use super::client::{HostClient, HostConfig, materialize_host_script};
use super::protocol::{HostAuth, HostOptions};
use super::provider::{FixtureInjection, HostProvider};
use lotta_extensions::sidecar::SidecarOwnerIdentity;
use lotta_testkit::fixtures::FixtureLoader;
use lotta_testkit::fixtures::providers::{load_case, load_index, replay_provider};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) fn source_root() -> PathBuf {
    if let Some(configured) = std::env::var_os("LOTTA_LETTA_CODE_CHECKOUT") {
        return PathBuf::from(configured)
            .canonicalize()
            .expect("LOTTA_LETTA_CODE_CHECKOUT must identify an available canonical checkout");
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../letta-code")
        .canonicalize()
        .expect("real-process tests require ../../letta-code sibling or LOTTA_LETTA_CODE_CHECKOUT")
}

pub(super) fn bun() -> PathBuf {
    let configured = std::env::var_os("LOTTA_BUN_EXECUTABLE")
        .map_or_else(|| PathBuf::from("/Users/stan/.bun/bin/bun"), PathBuf::from);
    configured
        .canonicalize()
        .expect("real-process tests require Bun or LOTTA_BUN_EXECUTABLE")
}

pub(super) fn owner() -> SidecarOwnerIdentity {
    SidecarOwnerIdentity::new("host-test", "runtime", "conversation").unwrap()
}

#[derive(Debug, Default)]
struct BrowserPort;
impl super::oauth::OAuthBrowser for BrowserPort {
    fn open(&self, _: &url::Url) -> Result<(), super::oauth::OAuthError> {
        Ok(())
    }
}

#[test]
fn host_script_materialization_is_restart_idempotent_and_tamper_evident() {
    let fixture = std::env::temp_dir().join(format!(
        "lotta-host-restart-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&fixture);
    std::fs::create_dir(&fixture).expect("create host restart fixture");
    let script = fixture.join("pi-ai-host.mjs");
    materialize_host_script(&script).expect("first materialization");
    materialize_host_script(&script).expect("restart materialization");
    std::fs::write(&script, "tampered").expect("tamper fixture");
    assert!(materialize_host_script(&script).is_err());
    let _ = std::fs::remove_dir_all(fixture);
}

pub(super) fn config() -> (HostConfig, PathBuf) {
    let source = source_root();
    let bun = bun();
    let package = source
        .join("node_modules/@earendil-works/pi-ai")
        .canonicalize()
        .expect("pinned @earendil-works/pi-ai 0.82.1 must be installed");
    let fixture = std::env::temp_dir().join(format!(
        "lotta-host-script-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&fixture);
    std::fs::create_dir(&fixture).expect("create isolated fixture directory");
    let script = fixture.join("pi-ai-host.mjs");
    materialize_host_script(&script).expect("materialize pinned host script");
    let script = script.canonicalize().expect("canonical host script");
    let scratch = std::env::temp_dir().join(format!(
        "lotta-host-scratch-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir(&scratch).expect("create empty scratch directory");
    let scratch = scratch.canonicalize().expect("canonical scratch directory");
    (
        HostConfig {
            bun_executable: bun,
            host_script: script,
            package_root: package,
            scratch_cwd: scratch.clone(),
            test_mode: false,
        },
        scratch,
    )
}

pub(super) fn test_config() -> (HostConfig, PathBuf) {
    let (mut config, scratch) = config();
    config.test_mode = true;
    (config, scratch)
}

pub(super) fn load_host_case(id: &str) -> lotta_testkit::fixtures::providers::ProviderCase {
    let loader = FixtureLoader::new();
    let index = load_index(&loader).expect("provider index");
    let indexed: Vec<_> = index
        .cases
        .iter()
        .filter(|case| {
            matches!(
                case.dialect,
                lotta_testkit::fixtures::providers::Dialect::OpenaiCompatible
                    | lotta_testkit::fixtures::providers::Dialect::Anthropic
            )
        })
        .map(|case| case.id.as_str())
        .collect();
    assert_eq!(indexed, host_replay_ids(), "canonical host manifest");
    let record = index
        .cases
        .into_iter()
        .find(|record| record.id == id)
        .unwrap_or_else(|| panic!("{id} is indexed"));
    load_case(&loader, &record).unwrap_or_else(|error| panic!("{id}: {error}"))
}

pub(super) async fn replay_case(id: &str) {
    let mut case = load_host_case(id);
    let fixture = fixture_for(&case);
    let (provider, scratch) = fixture_provider(&case, fixture).await;
    case.request.model.provider_id = lotta_domain::NonEmptyString::new("host-fixture").unwrap();
    case.request.model.handle =
        lotta_domain::NonEmptyString::new("host-fixture/fixture-model").unwrap();
    if let Some(host_trace) = case.host_expected_trace.take() {
        case.expected_trace = host_trace;
    }
    replay_provider(&provider, case)
        .await
        .unwrap_or_else(|error| panic!("{id}: {error}"));
    provider.shutdown().expect("shutdown");
    std::fs::remove_dir_all(scratch).unwrap();
}

fn fixture_for(case: &lotta_testkit::fixtures::providers::ProviderCase) -> FixtureInjection {
    let response = case
        .record
        .response
        .as_ref()
        .expect("host response fixture");
    let headers = Value::Object(
        response
            .headers
            .as_slice()
            .iter()
            .map(|entry| (entry.name.clone(), Value::String(entry.value.clone())))
            .collect(),
    );
    FixtureInjection {
        dialect: format!("{:?}", case.record.dialect),
        status: response.status,
        headers,
        bytes: case.raw_stream.as_bytes().to_vec(),
        expected_request: case.expected_request.as_value().clone(),
    }
}

async fn fixture_provider(
    case: &lotta_testkit::fixtures::providers::ProviderCase,
    fixture: FixtureInjection,
) -> (HostProvider, PathBuf) {
    let (config, scratch) = test_config();
    let api = if case.record.dialect == lotta_testkit::fixtures::providers::Dialect::Anthropic {
        "anthropic-messages"
    } else {
        "openai-completions"
    };
    let descriptor = json!({"id":"host-fixture","name":"Fixture",
        "owner":"host-replay","adapter":{"type":"pi_ai_api","api":api},"models":[{
        "id":"fixture-model","name":"Fixture Model","api":api,
        "provider":"host-fixture","reasoning":case.request.reasoning.enabled,
        "input":["text","image"],"contextWindow":case.request.model.context_window,
        "maxTokens":case.request.output_tokens_max.get(),
        "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"tiers":[]}}]});
    let provider = HostProvider::spawn(
        config,
        owner(),
        HostAuth::ApiKey {
            value: "fixture-credential".into(),
        },
        HostOptions::default(),
    )
    .await
    .expect("provider")
    .with_fixture(fixture);
    provider
        .register_adapter(descriptor)
        .await
        .expect("register fixture adapter");
    (provider, scratch)
}

fn host_replay_ids() -> [&'static str; 7] {
    [
        "openai-compatible/happy-tool",
        "anthropic/reasoning-redacted",
        "openai-compatible/retry-after",
        "anthropic/authentication",
        "openai-compatible/authorization",
        "anthropic/quota",
        "openai-compatible/protocol-error",
    ]
}

#[tokio::test]
async fn production_oauth_api_compiles_and_routes_metadata_through_host() {
    let manager = Arc::new(
        super::oauth::OAuthManager::production(Arc::new(BrowserPort)).expect("production OAuth"),
    );
    let (config, scratch) = config();
    let mut client = HostClient::spawn(config, owner())
        .await
        .unwrap()
        .with_oauth(manager);
    assert_eq!(
        client.oauth_metadata().await.unwrap(),
        super::oauth::OAuthMetadata::openai_codex()
    );
    client.shutdown().await.unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn oauth_protocol_commands_are_consumed_and_never_logged() {
    let host = include_str!("../../assets/pi-ai-host.mjs");
    for command in [
        "oauth.metadata",
        "oauth.begin",
        "oauth.exchange",
        "oauth.device.begin",
        "oauth.device.poll",
        "oauth.cancel",
    ] {
        assert!(host.contains(command));
    }
    assert!(host.contains("oauthDispatch(command, params)"));
    assert!(!host.contains("console.log"));
    assert!(!host.contains("console.error"));
}

#[test]
fn protocol_inference_is_correlated_bounded_and_single_attempt() {
    let host = include_str!("../../assets/pi-ai-host.mjs");
    for command in ["inference.start", "inference.event", "inference.cancel"] {
        assert!(host.contains(command));
    }
    assert!(host.contains("maxRetries: 0"));
    assert!(host.contains("params.sequence !== stream.cursor"));
    assert!(host.contains("EVENTS_MAX"));
    assert!(host.contains("process.stdout.once(\"drain\""));
}

#[test]
fn security_secrets_are_scoped_and_debug_redacted() {
    let provider = include_str!("provider.rs");
    let host = include_str!("../../assets/pi-ai-host.mjs");
    assert!(provider.contains("[REDACTED]"));
    assert!(!provider.contains("derive(Clone, Debug)\npub enum HostAuth"));
    assert!(host.contains("env: params.options?.env ?? {}"));
    assert!(!host.contains("console.log"));
    assert!(!host.contains("console.error"));
}

#[test]
fn bounds_apply_before_fixture_and_event_retention() {
    let host = include_str!("../../assets/pi-ai-host.mjs");
    assert!(host.contains("bytes.length > FIXTURE_BYTES_MAX"));
    assert!(host.contains("Buffer.byteLength(JSON.stringify(item)) > EVENT_BYTES_MAX"));
    assert!(host.contains("streams.size >= STREAMS_MAX"));
}

#[test]
fn mod_provider_registration_is_inference_routable_and_owner_checked() {
    let host = include_str!("../../assets/pi-ai-host.mjs");
    assert!(host.contains("registrations.get(request.model.provider)"));
    assert!(host.contains("prior.owner !== next.owner"));
    assert!(host.contains("prior.owner !== registrationOwner"));
}

#[test]
fn lifecycle_cancellation_deadline_receiver_close_and_terminal_are_enforced() {
    let provider = include_str!("provider.rs");
    assert!(provider.contains("HostClient::spawn(self.config.clone(), self.owner.clone())"));
    assert!(provider.contains("tokio::time::timeout(deadline, operation)"));
    assert!(provider.contains("inference_cancel"));
    assert!(provider.contains("ProviderEvent::Stop { .. } | ProviderEvent::Error { .. }"));
    assert!(provider.contains("events.send(event).await"));
}

#[test]
fn provider_host_frame_profile_covers_request_and_bounded_envelope() {
    let limit = lotta_extensions::sidecar::SidecarFrameLimit::provider_host().bytes_max();
    assert!(limit >= lotta_runtime::bounds::PROVIDER_REQUEST_BYTES_MAX.value + 1024);
    assert!(limit <= lotta_runtime::bounds::PROVIDER_REQUEST_BYTES_MAX.value + 4 * 1024 * 1024);
}

#[test]
fn timeout_and_error_mapping_cover_canonical_semantics() {
    let host = include_str!("../../assets/pi-ai-host.mjs");
    assert!(host.contains("TIMEOUT_MAX_MS = 300_000"));
    assert!(host.contains("Math.min(TIMEOUT_MAX_MS, params.request.deadline_ms)"));
    for kind in [
        "authentication",
        "authorization",
        "invalid_request",
        "rate_limit",
        "quota",
        "timeout",
        "context_overflow",
        "overloaded",
        "unavailable",
        "protocol",
        "cancelled",
        "unknown",
    ] {
        assert!(host.contains(kind), "missing {kind}");
    }
    assert!(host.contains("retry-after-ms"));
    assert!(host.contains("retry-after"));
}

#[test]
fn stderr_is_drained_after_diagnostic_cap_and_base_url_is_test_only() {
    let client = include_str!("client.rs");
    let host = include_str!("../../assets/pi-ai-host.mjs");
    assert!(client.contains("reader.read(&mut buffer).await"));
    assert!(client.contains("read.min(remaining)"));
    assert!(host.contains("else if (TEST_MODE && params.options?.base_url)"));
    assert!(!host.contains("if (params.options?.base_url || fixture)"));
}

#[test]
fn fixture_stream_is_explicit_test_only_capability() {
    let host = include_str!("../../assets/pi-ai-host.mjs");
    assert!(host.contains("LOTTA_HOST_TEST_MODE"));
    assert!(host.contains("fixture capability disabled"));
    assert!(host.contains("fixtureLoopback"));
    assert!(host.contains("provider.streamSimple"));
    assert!(!host.contains("fixture_events"));
    assert!(!host.contains("fixture_error"));
}

#[cfg(unix)]
fn stalled_handshake_config() -> (HostConfig, PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;
    let root = std::env::temp_dir().join(format!(
        "lotta-host-cancel-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("package")).expect("package root");
    std::fs::create_dir(root.join("scratch")).expect("scratch root");
    std::fs::write(
        root.join("package/package.json"),
        r#"{"name":"@earendil-works/pi-ai","version":"0.82.1"}"#,
    )
    .expect("package manifest");
    let pid = root.join("host.pid");
    let executable = root.join("stalled-host");
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nread blocked\n",
            pid.display()
        ),
    )
    .expect("host executable");
    let mut permissions = std::fs::metadata(&executable)
        .expect("host metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions).expect("host permissions");
    let canonical = root.canonicalize().expect("fixture root");
    let executable = canonical.join("stalled-host");
    let config = HostConfig {
        bun_executable: executable.clone(),
        host_script: executable,
        package_root: canonical.join("package"),
        scratch_cwd: canonical.join("scratch"),
        test_mode: false,
    };
    (config, canonical, pid)
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_handshake_reaps_the_owned_child() {
    let (config, root, pid_path) = stalled_handshake_config();
    let spawning = tokio::spawn(HostClient::spawn(config, owner()));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !pid_path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("host started");
    let pid = std::fs::read_to_string(&pid_path).expect("host pid");
    spawning.abort();
    let _ = spawning.await;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while std::process::Command::new("kill")
            .args(["-0", pid.as_str()])
            .status()
            .is_ok_and(|status| status.success())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned child reaped after cancellation");
    std::fs::remove_dir_all(root).expect("remove cancellation fixture");
}
