use super::host::ModHost;
use super::registrations::ModRegistrationSnapshot;
use super::registry::{ActiveGenerationTable, ModPublication, ModRegistries};
use super::test_support::*;
use super::types::{Generation, ModError, ModOwner};
use lotta_tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio_util::sync::CancellationToken;

fn registries() -> Arc<ModRegistries> {
    let tools = Arc::new(ToolRegistry::new([]).expect("empty builtins"));
    Arc::new(ModRegistries::new(tools))
}

fn publication(mod_owner: &ModOwner, prefix: &str, host: Arc<dyn ModHost>) -> ModPublication {
    let snapshot = ModRegistrationSnapshot::from_batch(mod_owner, batch(mod_owner, prefix))
        .expect("validated batch");
    ModPublication {
        owner: mod_owner.clone(),
        registrations: Arc::new(snapshot),
        host,
    }
}

/// Publishes generation one, captures its runtime view, then supersedes it with generation two.
fn superseded() -> (
    Arc<ModRegistries>,
    super::registry::ModRuntimeSnapshot,
    Arc<StubHost>,
) {
    let registries = registries();
    let first = owner("project:stale.ts", 1);
    let host = StubHost::new();
    registries
        .commit(&[publication(
            &first,
            "stale",
            Arc::clone(&host) as Arc<dyn ModHost>,
        )])
        .expect("first publication");
    let stale = registries.runtime().expect("runtime view");
    let second = owner("project:stale.ts", 2);
    registries
        .commit(&[publication(
            &second,
            "stale",
            Arc::clone(&host) as Arc<dyn ModHost>,
        )])
        .expect("second publication");
    (registries, stale, host)
}

#[tokio::test]
async fn all_six_paths_resolve_at_the_current_generation() {
    let registries = registries();
    let mod_owner = owner("project:live.ts", 1);
    let host = StubHost::new();
    registries
        .commit(&[publication(
            &mod_owner,
            "live",
            Arc::clone(&host) as Arc<dyn ModHost>,
        )])
        .unwrap();
    let snapshot = registries.runtime().unwrap();

    let tool = snapshot.tool(&name("live_tool")).unwrap();
    assert_eq!(tool.owner(), &mod_owner);
    assert_eq!(
        tool.call("call-1".into(), json!({}), CancellationToken::new())
            .await
            .unwrap(),
        json!("ok")
    );
    let command = snapshot.command(&name("live_command")).unwrap();
    assert_eq!(
        command
            .call(json!({}), CancellationToken::new())
            .await
            .unwrap(),
        json!("ok")
    );
    let lifecycle = snapshot.lifecycle(&name("live_event")).unwrap();
    assert_eq!(
        lifecycle
            .call(json!({}), CancellationToken::new())
            .await
            .unwrap(),
        json!("ok")
    );
    assert_eq!(
        snapshot.provider(&name("live_provider")).unwrap().owner,
        mod_owner
    );
    assert_eq!(
        snapshot.permission(&name("live_permission")).unwrap().owner,
        mod_owner
    );
    assert_eq!(
        snapshot.ui_metadata(&name("live_panel")).unwrap().owner,
        mod_owner
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 3);
}

#[test]
fn tool_path_refuses_a_stale_generation() {
    let (_registries, stale, _host) = superseded();
    assert_eq!(
        stale.tool(&name("stale_tool")).err(),
        Some(ModError::InvalidScope)
    );
}

#[test]
fn command_path_refuses_a_stale_generation() {
    let (_registries, stale, _host) = superseded();
    assert_eq!(
        stale.command(&name("stale_command")).err(),
        Some(ModError::InvalidScope)
    );
}

#[test]
fn lifecycle_path_refuses_a_stale_generation() {
    let (_registries, stale, _host) = superseded();
    assert_eq!(
        stale.lifecycle(&name("stale_event")).err(),
        Some(ModError::InvalidScope)
    );
}

#[test]
fn provider_path_refuses_a_stale_generation() {
    let (_registries, stale, _host) = superseded();
    assert_eq!(
        stale.provider(&name("stale_provider")).err(),
        Some(ModError::InvalidScope)
    );
}

#[test]
fn permission_path_refuses_a_stale_generation() {
    let (_registries, stale, _host) = superseded();
    assert_eq!(
        stale.permission(&name("stale_permission")).err(),
        Some(ModError::InvalidScope)
    );
}

#[test]
fn ui_metadata_path_refuses_a_stale_generation() {
    let (_registries, stale, _host) = superseded();
    assert_eq!(
        stale.ui_metadata(&name("stale_panel")).err(),
        Some(ModError::InvalidScope)
    );
}

#[test]
fn a_stale_generation_never_reaches_the_host() {
    let (_registries, stale, host) = superseded();
    assert!(stale.tool(&name("stale_tool")).is_err());
    assert!(stale.command(&name("stale_command")).is_err());
    assert!(stale.lifecycle(&name("stale_event")).is_err());
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_absent_registration_is_refused_on_every_path() {
    let registries = registries();
    let snapshot = registries.runtime().unwrap();
    let absent = name("absent");
    assert_eq!(
        snapshot.tool(&absent).err(),
        Some(ModError::InvalidRegistration)
    );
    assert_eq!(
        snapshot.command(&absent).err(),
        Some(ModError::InvalidRegistration)
    );
    assert_eq!(
        snapshot.lifecycle(&absent).err(),
        Some(ModError::InvalidRegistration)
    );
    assert_eq!(
        snapshot.provider(&absent).err(),
        Some(ModError::InvalidRegistration)
    );
    assert_eq!(
        snapshot.permission(&absent).err(),
        Some(ModError::InvalidRegistration)
    );
    assert_eq!(
        snapshot.ui_metadata(&absent).err(),
        Some(ModError::InvalidRegistration)
    );
}

#[test]
fn one_shared_generation_table_backs_every_path() {
    let registries = registries();
    let table = Arc::clone(registries.generations());
    let mod_owner = owner("project:shared.ts", 1);
    assert!(table.is_empty().unwrap());

    registries
        .commit(&[publication(
            &mod_owner,
            "shared",
            StubHost::new() as Arc<dyn ModHost>,
        )])
        .unwrap();

    // The captured handle observes the commit, so publication mutates one table rather than
    // replacing it; every access and invoke path therefore sees the same generation.
    assert_eq!(table.current(&mod_owner.id).unwrap(), Some(Generation(1)));
    assert_eq!(table.len().unwrap(), 1);
    registries.clear_mods().unwrap();
    assert!(table.is_empty().unwrap());
}

#[tokio::test]
async fn a_handle_captured_before_invalidation_refuses_at_call_time() {
    let registries = registries();
    let first = owner("project:late.ts", 1);
    let host = StubHost::new();
    registries
        .commit(&[publication(
            &first,
            "late",
            Arc::clone(&host) as Arc<dyn ModHost>,
        )])
        .unwrap();
    let command = registries
        .runtime()
        .unwrap()
        .command(&name("late_command"))
        .unwrap();

    let second = owner("project:late.ts", 2);
    registries
        .commit(&[publication(
            &second,
            "late",
            Arc::clone(&host) as Arc<dyn ModHost>,
        )])
        .unwrap();

    assert_eq!(
        command.call(json!({}), CancellationToken::new()).await,
        Err(ModError::InvalidScope)
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_activated_table_validates_only_its_exact_generations() {
    let table = ActiveGenerationTable::activated([owner("project:exact.ts", 4)]);
    assert!(table.validate(&owner("project:exact.ts", 4)).is_ok());
    assert_eq!(
        table.validate(&owner("project:exact.ts", 3)).err(),
        Some(ModError::InvalidScope)
    );
    assert_eq!(
        table.validate(&owner("project:other.ts", 4)).err(),
        Some(ModError::InvalidScope)
    );
}
