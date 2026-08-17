use super::controller::{ModHostController, ModStartRequest};
use super::registry::ModRegistries;
use super::safe_mode::{ModTrust, ModsStartupMode};
use super::test_support::*;
use super::types::{Capability, ModId, ModOwner};
use lotta_runtime::ports::ToolExecutionOwner;
use lotta_testkit::clock::FakeClock;
use lotta_tools::registry::ToolRegistry;
use lotta_tools::toolset::ToolsetId;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio_util::sync::CancellationToken;

fn stores() -> (Arc<ToolRegistry>, Arc<ModRegistries>) {
    let tools = Arc::new(ToolRegistry::new([]).expect("empty builtins"));
    let registries = Arc::new(ModRegistries::new(Arc::clone(&tools)));
    (tools, registries)
}

fn controller(
    registries: &Arc<ModRegistries>,
    port: &Arc<RecordingPort>,
) -> ModHostController<FakeClock> {
    ModHostController::new(
        clock(),
        Arc::clone(registries),
        Arc::clone(port) as Arc<dyn super::capabilities::CapabilityPort>,
    )
}

fn scripted(owner: &ModOwner, prefix: &str, diagnostics: Vec<String>) -> ScriptedLauncher {
    let sidecar = sidecar_owner(owner.id.as_str());
    let responder = standard_responder(sidecar.clone(), batch(owner, prefix), diagnostics);
    ScriptedLauncher::new(sidecar, vec![ChildPlan::new(responder)])
}

fn request(
    owner: &ModOwner,
    prefix: &str,
    trust: ModTrust,
    diagnostics: Vec<String>,
) -> ModStartRequest<ScriptedLauncher> {
    ModStartRequest {
        spec: start_spec(owner.clone(), trust, &[Capability::Tools]),
        launcher: scripted(owner, prefix, diagnostics),
    }
}

#[tokio::test(start_paused = true)]
async fn start_publishes_all_six_kinds_for_every_retained_host() {
    let (tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let left = owner("project:left.ts", 1);
    let right = owner("project:right.ts", 1);
    let requests = vec![
        request(&left, "left", ModTrust::BuiltIn, Vec::new()),
        request(&right, "right", ModTrust::Trusted, Vec::new()),
    ];
    assert!(host.start(enabled(), requests).await.unwrap().is_empty());

    let snapshot = host.snapshot().unwrap();
    let registrations = snapshot.registrations();
    assert_eq!(registrations.tools.len(), 2);
    assert_eq!(registrations.commands.len(), 2);
    assert_eq!(registrations.providers.len(), 2);
    assert_eq!(registrations.permissions.len(), 2);
    assert_eq!(registrations.lifecycle_events.len(), 2);
    assert_eq!(registrations.ui_metadata.len(), 2);
    assert_eq!(registrations.len(), 12);
    assert_eq!(tools.snapshot().unwrap().len(), 2);
    assert_eq!(host.active_owners().await.len(), 2);
    host.dispose_all().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn safe_mode_skips_third_party_and_retains_trusted() {
    let (_tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let trusted = owner("project:trusted.ts", 1);
    let third = owner("project:third.ts", 1);
    let third_launcher = scripted(&third, "third", Vec::new());
    let skipped = third_launcher.launched();
    let requests = vec![
        request(&trusted, "trusted", ModTrust::Trusted, Vec::new()),
        ModStartRequest {
            spec: start_spec(third.clone(), ModTrust::ThirdParty, &[Capability::Tools]),
            launcher: third_launcher,
        },
    ];
    let diagnostics = host
        .start(ModsStartupMode::SafeMode, requests)
        .await
        .unwrap();

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].owner, third);
    assert_eq!(diagnostics[0].reason, "safe_mode");
    assert_eq!(skipped.load(Ordering::SeqCst), 0);
    assert_eq!(host.snapshot().unwrap().registrations().len(), 6);
    host.dispose_all().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn no_mods_starts_without_host_and_removes_every_mod_registration() {
    let (tools, registries) = stores();
    tools
        .update(
            ToolsetId::None,
            &[
                runtime_tool("native", ToolExecutionOwner::Rust),
                runtime_tool("mcp", ToolExecutionOwner::Mcp),
            ],
            None,
        )
        .unwrap();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let first = owner("project:first.ts", 1);
    let enabled_requests = vec![request(&first, "first", ModTrust::BuiltIn, Vec::new())];
    host.start(enabled(), enabled_requests).await.unwrap();
    assert_eq!(host.snapshot().unwrap().registrations().len(), 6);

    let second = owner("project:second.ts", 1);
    let disabled = scripted(&second, "second", Vec::new());
    let launched = disabled.launched();
    let requests = vec![ModStartRequest {
        spec: start_spec(second.clone(), ModTrust::BuiltIn, &[Capability::Tools]),
        launcher: disabled,
    }];
    let diagnostics = host.start(ModsStartupMode::NoMods, requests).await.unwrap();

    assert_eq!(launched.load(Ordering::SeqCst), 0);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].reason, "mods_disabled");
    assert!(host.snapshot().unwrap().registrations().is_empty());
    assert!(host.active_owners().await.is_empty());
    assert!(host.brokers().is_empty().unwrap());
    let published = tools.snapshot().unwrap();
    assert_eq!(published.len(), 2);
    assert!(published.by_internal("native").is_some());
    assert!(published.by_internal("mcp").is_some());
}

#[tokio::test(start_paused = true)]
async fn private_handles_are_minted_and_revoked_around_accept() {
    let (_tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let left = owner("project:left.ts", 1);
    let right = owner("project:right.ts", 1);
    let requests = vec![
        request(&left, "left", ModTrust::BuiltIn, Vec::new()),
        request(&right, "right", ModTrust::BuiltIn, Vec::new()),
    ];
    host.start(enabled(), requests).await.unwrap();
    assert_eq!(host.brokers().len().unwrap(), 2);

    host.dispose(&left.id).await.unwrap();
    assert_eq!(host.brokers().len().unwrap(), 1);
    host.dispose_all().await.unwrap();
    assert_eq!(host.brokers().len().unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn failed_candidate_revokes_its_handle_and_leaves_no_registrations() {
    let (_tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let broken = owner("project:broken.ts", 1);
    let sidecar = sidecar_owner(broken.id.as_str());
    // A child that answers nothing after hello fails introduction before publication.
    let plan = ChildPlan::new(Box::new(|_| Vec::new()));
    let requests = vec![ModStartRequest {
        spec: start_spec(broken.clone(), ModTrust::BuiltIn, &[Capability::Tools]),
        launcher: ScriptedLauncher::new(sidecar, vec![plan]),
    }];
    let diagnostics = host.start(enabled(), requests).await.unwrap();

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].reason, "load_failed");
    assert!(host.snapshot().unwrap().registrations().is_empty());
    assert!(host.brokers().is_empty().unwrap());
}

#[tokio::test(start_paused = true)]
async fn declared_capability_reaches_the_injected_port_with_private_scope() {
    let (_tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let mod_owner = owner("project:probe.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let probe = CapabilityProbe::new();
    let responder = capability_probe_responder(
        mod_owner.clone(),
        sidecar.clone(),
        batch(&mod_owner, "probe"),
        Capability::Tools,
        Arc::clone(&probe),
    );
    let requests = vec![ModStartRequest {
        spec: start_spec(mod_owner.clone(), ModTrust::BuiltIn, &[Capability::Tools]),
        launcher: ScriptedLauncher::new(sidecar, vec![ChildPlan::new(responder)]),
    }];
    host.start(enabled(), requests).await.unwrap();

    assert_eq!(probe.observed(), vec![Ok(json!({"probe": true}))]);
    let observed = port.observed();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].0, mod_owner);
    assert_eq!(observed[0].1, Capability::Tools);
    assert_eq!(observed[0].2, "probe");
    assert_eq!(port.observed_scopes(), vec!["conversation-a".to_owned()]);
    host.dispose_all().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn undeclared_capability_is_refused_over_the_wire() {
    let (_tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let mod_owner = owner("project:refused.ts", 1);
    let sidecar = sidecar_owner(mod_owner.id.as_str());
    let probe = CapabilityProbe::new();
    let responder = capability_probe_responder(
        mod_owner.clone(),
        sidecar.clone(),
        batch(&mod_owner, "refused"),
        Capability::Providers,
        Arc::clone(&probe),
    );
    let requests = vec![ModStartRequest {
        spec: start_spec(mod_owner.clone(), ModTrust::BuiltIn, &[Capability::Tools]),
        launcher: ScriptedLauncher::new(sidecar, vec![ChildPlan::new(responder)]),
    }];
    host.start(enabled(), requests).await.unwrap();

    assert_eq!(probe.observed(), vec![Err(-32010)]);
    assert!(port.observed().is_empty());
    host.dispose_all().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn reload_advances_only_the_selected_owner() {
    let (_tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let left = owner("project:left.ts", 1);
    let right = owner("project:right.ts", 1);
    let requests = vec![
        request(&left, "left", ModTrust::BuiltIn, Vec::new()),
        request(&right, "right", ModTrust::BuiltIn, Vec::new()),
    ];
    host.start(enabled(), requests).await.unwrap();

    let reloaded = owner("project:left.ts", 2);
    let mut launcher = scripted(&reloaded, "left", Vec::new());
    host.reload(&mut launcher, &left.id).await.unwrap();

    let generations = registries.generations();
    assert_eq!(
        generations.current(&left.id).unwrap(),
        Some(reloaded.generation)
    );
    assert_eq!(
        generations.current(&right.id).unwrap(),
        Some(right.generation)
    );
    let snapshot = host.snapshot().unwrap();
    assert_eq!(snapshot.registrations().len(), 12);
    assert_eq!(
        snapshot.tool(&name("left_tool")).unwrap().owner(),
        &reloaded
    );
    let survivor = snapshot.command(&name("right_command")).unwrap();
    assert_eq!(
        survivor
            .call(json!({}), CancellationToken::new())
            .await
            .unwrap(),
        json!("ok")
    );
    host.dispose_all().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn failed_reload_leaves_the_previous_generation_callable() {
    let (tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let only = owner("project:only.ts", 1);
    let requests = vec![request(&only, "only", ModTrust::BuiltIn, Vec::new())];
    host.start(enabled(), requests).await.unwrap();
    let revision = tools.revision().unwrap();

    // An exhausted launcher fails before any candidate can be introduced.
    let mut launcher = ScriptedLauncher::new(sidecar_owner(only.id.as_str()), Vec::new());
    assert!(host.reload(&mut launcher, &only.id).await.is_err());

    assert_eq!(tools.revision().unwrap(), revision);
    assert_eq!(
        registries.generations().current(&only.id).unwrap(),
        Some(only.generation)
    );
    let snapshot = host.snapshot().unwrap();
    assert_eq!(snapshot.registrations().len(), 6);
    let command = snapshot.command(&name("only_command")).unwrap();
    assert_eq!(
        command
            .call(json!({}), CancellationToken::new())
            .await
            .unwrap(),
        json!("ok")
    );
    assert_eq!(host.brokers().len().unwrap(), 1);
    host.dispose_all().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn dispose_removes_only_the_selected_owner() {
    let (tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let left = owner("project:left.ts", 1);
    let right = owner("project:right.ts", 1);
    let requests = vec![
        request(&left, "left", ModTrust::BuiltIn, Vec::new()),
        request(&right, "right", ModTrust::BuiltIn, Vec::new()),
    ];
    host.start(enabled(), requests).await.unwrap();

    host.dispose(&left.id).await.unwrap();

    let snapshot = host.snapshot().unwrap();
    assert_eq!(snapshot.registrations().len(), 6);
    assert!(snapshot.tool(&name("left_tool")).is_err());
    assert!(snapshot.tool(&name("right_tool")).is_ok());
    assert!(
        registries
            .generations()
            .current(&left.id)
            .unwrap()
            .is_none()
    );
    let published = tools.snapshot().unwrap();
    assert_eq!(published.len(), 1);
    assert!(published.by_internal("right_tool").is_some());
    assert!(host.dispose(&left.id).await.is_err());
    host.dispose_all().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn diagnostics_are_owner_attributed_over_real_rpc() {
    let (_tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let left = owner("project:left.ts", 1);
    let right = owner("project:right.ts", 1);
    let requests = vec![
        request(&left, "left", ModTrust::BuiltIn, vec!["left warned".into()]),
        request(
            &right,
            "right",
            ModTrust::BuiltIn,
            vec!["right warned".into(), "right again".into()],
        ),
    ];
    host.start(enabled(), requests).await.unwrap();

    let collected = host.diagnostics().await;
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0].owner, left);
    assert_eq!(collected[0].entries, vec!["left warned".to_owned()]);
    assert_eq!(collected[1].owner, right);
    assert_eq!(collected[1].entries.len(), 2);
    let single = host.diagnostics_for(&right.id).await.unwrap();
    assert_eq!(single.owner, right);
    assert_eq!(single.entries[0], "right warned");
    assert!(
        host.diagnostics_for(&ModId::new("project:absent.ts".into()).unwrap())
            .await
            .is_err()
    );
    host.dispose_all().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn disposal_stops_and_joins_every_child_exactly_once() {
    let (_tools, registries) = stores();
    let port = RecordingPort::new();
    let host = controller(&registries, &port);
    let only = owner("project:only.ts", 1);
    let launcher = scripted(&only, "only", Vec::new());
    let counters = launcher.counters();
    let requests = vec![ModStartRequest {
        spec: start_spec(only.clone(), ModTrust::BuiltIn, &[Capability::Tools]),
        launcher,
    }];
    host.start(enabled(), requests).await.unwrap();
    host.dispose_all().await.unwrap();

    let observed = counters.lock().expect("counter lock");
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].stops.load(Ordering::SeqCst), 1);
    assert_eq!(observed[0].joins.load(Ordering::SeqCst), 1);
    assert_eq!(observed[0].kills.load(Ordering::SeqCst), 0);
}
