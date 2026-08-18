use super::*;
use crate::connections::store::{parse_for_test, serialize_for_test};
use lotta_domain::{AgentId, ConversationId, RunId, RuntimeScope, StopReason, TurnLease};
use lotta_runtime::{ListenerRuntime, RuntimeHandle};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use uuid::Uuid;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn root(label: &str) -> lotta_store::StorePaths {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "lotta-task52-{label}-{}-{sequence}",
        std::process::id()
    ));
    lotta_store::StorePaths::new(path).unwrap()
}

fn fixture() -> &'static [u8] {
    include_bytes!("../../../../fixtures/persistence/side_stores/providers/auth.json")
}

fn input(name: &str, revision: u64) -> ConnectProviderInput {
    ConnectProviderInput {
        provider_id: format!("local-provider-{name}"),
        provider_name: name.into(),
        provider_type: "openai".into(),
        auth_method: AuthMethod::Api,
        auth: ProviderAuth::Api {
            key: ProviderSecret::new(format!("secret-{name}")).unwrap(),
            extras: serde_json::Map::new(),
        },
        fields: BTreeMap::new(),
        expected_revision: revision,
        now: "2000-01-01T00:00:00.000Z".into(),
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for CaptureWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> MakeWriter<'a> for Capture {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter(Arc::clone(&self.0))
    }
}
impl Capture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

async fn captured<T>(future: impl Future<Output = T>, capture: &Capture) -> T {
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(capture.clone())
            .without_time(),
    );
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);
    future.await
}

struct Accept;
impl ConnectionAdapter for Accept {
    fn validate<'a>(
        &'a self,
        _: &'a str,
        _: &'a ProviderAuth,
        _: &'a BTreeMap<String, String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConnectionError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

mod auth_json {
    use super::*;

    #[test]
    fn round_trips_fixture() {
        let records = parse_for_test(fixture()).unwrap();
        let bytes = serialize_for_test(&records).unwrap();
        let reparsed = parse_for_test(&bytes).unwrap();
        assert_eq!(
            records.keys().collect::<Vec<_>>(),
            reparsed.keys().collect::<Vec<_>>()
        );
        assert_eq!(serialize_for_test(&reparsed).unwrap(), bytes);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("<redacted-fixture>"));
        assert!(!text.contains("credentials.enc"));
    }

    #[test]
    fn bedrock_profile_empty_key_round_trips_exactly() {
        let source = br#"{
  "version": 1,
  "providers": {
    "aws-profile": {
      "id": "local-provider-aws-profile",
      "name": "aws-profile",
      "provider_type": "amazon-bedrock",
      "provider_category": "byok",
      "auth": {"type": "api", "key": ""},
      "region": "us-east-1",
      "profile": "default",
      "created_at": "2026-08-14T00:00:00.000Z",
      "updated_at": "2026-08-14T00:00:00.000Z"
    }
  }
}"#;
        let records = parse_for_test(source).unwrap();
        assert!(matches!(
            records["aws-profile"].auth,
            ProviderAuth::BedrockProfile { .. }
        ));
        let bytes = serialize_for_test(&records).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["providers"]["aws-profile"]["auth"]["key"], "");
        assert!(!String::from_utf8_lossy(&bytes).contains("not-needed"));

        let paths = root("bedrock-profile-empty");
        let store = ProviderAuthStore::new(paths.clone());
        let revision = store.replace(&records, 0).unwrap();
        let (loaded, loaded_revision) = store.load().unwrap();
        assert_eq!(revision, loaded_revision);
        assert!(matches!(
            loaded["aws-profile"].auth,
            ProviderAuth::BedrockProfile { .. }
        ));
        store.replace(&loaded, loaded_revision).unwrap();
        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(paths.provider_auth()).unwrap()).unwrap();
        assert_eq!(persisted["providers"]["aws-profile"]["auth"]["key"], "");
    }

    #[cfg(unix)]
    #[test]
    fn enforces_modes() {
        use std::os::unix::fs::PermissionsExt as _;
        let paths = root("modes");
        std::fs::create_dir_all(paths.providers()).unwrap();
        std::fs::set_permissions(paths.providers(), std::fs::Permissions::from_mode(0o777))
            .unwrap();
        std::fs::write(paths.provider_auth(), fixture()).unwrap();
        std::fs::set_permissions(
            paths.provider_auth(),
            std::fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        ProviderAuthStore::new(paths.clone())
            .enforce_modes()
            .unwrap();
        assert_eq!(
            std::fs::metadata(paths.providers())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(paths.provider_auth())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn rejects_unsupported_malformed_duplicate_and_nonregular() {
        assert_eq!(
            parse_for_test(br#"{"version":2,"providers":{}}"#).unwrap_err(),
            ConnectionError::Unsupported
        );
        assert!(parse_for_test(b"{").is_err());
        assert!(parse_for_test(br#"{"version":1,"version":1,"providers":{}}"#).is_err());
        assert!(parse_for_test(br#"{"version":1,"providers":{"a":{},"a":{}}}"#).is_err());
        let paths = root("nonregular");
        std::fs::create_dir_all(paths.provider_auth()).unwrap();
        assert_eq!(
            ProviderAuthStore::new(paths).load().unwrap_err(),
            ConnectionError::Store
        );
    }
}

mod catalogs {
    use super::*;

    #[test]
    fn exact_public_declarative_catalogs() {
        let cases = [
            ("default", default_api_key_fields()),
            ("openai-compatible", openai_compatible_fields()),
            ("ollama", ollama_fields()),
            ("lmstudio", lmstudio_fields()),
            ("llama-cpp", llama_cpp_fields()),
            ("google-vertex", google_vertex_fields()),
        ];
        let expected = [
            serde_json::json!([{"key":"apiKey","label":"API Key","secret":true,"required":true}]),
            serde_json::json!([
                {"key":"apiKey","label":"API Key","secret":true,"required":false},
                {"key":"baseUrl","label":"Base URL","required":true}
            ]),
            serde_json::json!([
                {"key":"baseUrl","label":"Base URL","placeholder":"http://localhost:11434/v1","required":false},
                {"key":"apiKey","label":"API Key","secret":true,"required":false}
            ]),
            serde_json::json!([
                {"key":"baseUrl","label":"Base URL","placeholder":"http://127.0.0.1:1234/v1","required":false},
                {"key":"apiKey","label":"API Key","secret":true,"required":false}
            ]),
            serde_json::json!([
                {"key":"baseUrl","label":"Base URL","placeholder":"http://localhost:8080/v1","required":false},
                {"key":"apiKey","label":"API Key","secret":true,"required":false}
            ]),
            serde_json::json!([{"key":"apiKey","label":"API Key","secret":true,"required":true}]),
        ];
        for ((name, actual), expected) in cases.into_iter().zip(expected) {
            assert_eq!(serde_json::to_value(actual).unwrap(), expected, "{name}");
        }

        assert_eq!(
            serde_json::to_value(baseline_bedrock_auth_methods()).unwrap(),
            serde_json::json!([
                {"id":"iam","label":"AWS Access Keys","description":"Enter access key and secret key manually","fields":[
                    {"key":"accessKey","label":"AWS Access Key ID","placeholder":"AKIA...","required":true},
                    {"key":"apiKey","label":"AWS Secret Access Key","secret":true,"required":true},
                    {"key":"region","label":"AWS Region","placeholder":"us-east-1","required":true}
                ]},
                {"id":"profile","label":"AWS Profile","description":"Load credentials from ~/.aws/credentials","fields":[
                    {"key":"profile","label":"Profile Name","placeholder":"default","required":true},
                    {"key":"region","label":"AWS Region","placeholder":"us-east-1","required":true}
                ]}
            ])
        );
    }
}

mod redaction {
    use super::*;

    #[tokio::test]
    async fn connect_diagnostics_redact_secret() {
        let capture = Capture::default();
        let mut manager =
            ConnectionManager::load(ProviderAuthStore::new(root("success-redaction"))).unwrap();
        let value = input("redaction", 0);
        let debug = format!("{value:?}");
        captured(manager.connect(value, &Accept), &capture)
            .await
            .unwrap();
        let output = capture.text();
        assert!(output.contains("provider.connect.persisted"));
        assert!(!output.contains("secret-redaction"));
        assert!(!debug.contains("secret-redaction"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn failed_connect_error_redacts_secret() {
        struct Reject;
        impl ConnectionAdapter for Reject {
            fn validate<'a>(
                &'a self,
                _: &'a str,
                _: &'a ProviderAuth,
                _: &'a BTreeMap<String, String>,
            ) -> Pin<Box<dyn Future<Output = Result<(), ConnectionError>> + Send + 'a>>
            {
                Box::pin(async { Err(ConnectionError::InvalidInput("secret-redaction")) })
            }
        }
        let mut manager =
            ConnectionManager::load(ProviderAuthStore::new(root("failed-redaction"))).unwrap();
        let capture = Capture::default();
        let error = captured(manager.connect(input("redaction", 0), &Reject), &capture)
            .await
            .unwrap_err();
        let output = capture.text();
        assert!(output.contains("provider.connect.validate"));
        assert!(!output.contains("secret-redaction"));
        assert_eq!(error, ConnectionError::Adapter);
        assert!(!error.to_string().contains("secret-redaction"));
    }

    #[tokio::test]
    async fn snapshot_json_omits_secret() {
        let mut manager =
            ConnectionManager::load(ProviderAuthStore::new(root("snapshot-redaction"))).unwrap();
        let capture = Capture::default();
        captured(manager.connect(input("redaction", 0), &Accept), &capture)
            .await
            .unwrap();
        let json = serde_json::to_string(&manager.snapshots()).unwrap();
        tracing::dispatcher::with_default(
            &tracing::Dispatch::new(
                tracing_subscriber::registry().with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(capture.clone())
                        .without_time(),
                ),
            ),
            || tracing::info!(snapshot = %json, marker = "provider.snapshot", "provider snapshot"),
        );
        let output = capture.text();
        assert!(output.contains("provider.snapshot"));
        assert!(!output.contains("secret-redaction"));
        assert!(!json.contains("secret-redaction"));
        assert!(!json.contains("auth\""));
        manager
            .with_auth("local-provider-redaction", |auth| match auth {
                ProviderAuth::Api { key, .. } => {
                    assert_eq!(key.expose(), "secret-redaction");
                }
                ProviderAuth::OAuth { .. } | ProviderAuth::BedrockProfile { .. } => panic!(),
            })
            .unwrap();
    }
}

mod bounds {
    use super::*;

    async fn fill(count: usize) -> Result<ConnectionManager, ConnectionError> {
        let mut manager =
            ConnectionManager::load(ProviderAuthStore::new(root(&format!("bounds-{count}"))))
                .unwrap();
        for index in 0..count {
            let revision = manager.revision();
            manager
                .connect(input(&format!("provider-{index}"), revision), &Accept)
                .await?;
        }
        Ok(manager)
    }

    #[tokio::test]
    async fn below() {
        assert_eq!(
            fill(PROVIDERS_MAX - 1).await.unwrap().snapshots().len(),
            127
        );
    }
    #[tokio::test]
    async fn at() {
        assert_eq!(fill(PROVIDERS_MAX).await.unwrap().snapshots().len(), 128);
    }
    #[tokio::test]
    async fn above() {
        let mut manager = fill(PROVIDERS_MAX).await.unwrap();
        let before = serde_json::to_string(&manager.snapshots()).unwrap();
        let error = manager
            .connect(input("provider-129", manager.revision()), &Accept)
            .await
            .unwrap_err();
        assert_eq!(error, ConnectionError::Capacity);
        assert_eq!(serde_json::to_string(&manager.snapshots()).unwrap(), before);
    }
}

struct Cancellation {
    order: Arc<Mutex<Vec<&'static str>>>,
    fail: bool,
    lease: TurnLease,
}
impl TurnCancellation for Cancellation {
    fn cancel_and_wait<'a>(
        &'a self,
        runtime: &'a mut ListenerRuntime,
        handle: &'a RuntimeHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConnectionError>> + Send + 'a>> {
        Box::pin(async move {
            self.order.lock().unwrap().push("cancel");
            if self.fail {
                return Err(ConnectionError::Cancellation);
            }
            let owner = runtime.lifecycle_mut(handle)?;
            owner.request_cancellation(&self.lease)?;
            owner.finish_turn(&self.lease, StopReason::new("cancelled").unwrap())?;
            self.order.lock().unwrap().push("ack");
            Ok(())
        })
    }
}

mod disconnect {
    use super::*;

    fn active() -> (ListenerRuntime, RuntimeHandle, TurnLease) {
        let scope = RuntimeScope {
            agent_id: AgentId::accept("agent").unwrap(),
            conversation_id: ConversationId::accept("conversation").unwrap(),
            acting_user_id: None,
        };
        let mut runtime = ListenerRuntime::new();
        let handle = runtime.get_or_create(&scope, Uuid::from_u128(1)).unwrap();
        let lease = runtime
            .lifecycle_mut(&handle)
            .unwrap()
            .begin_turn("turn".into(), RunId::accept("run").unwrap())
            .unwrap();
        (runtime, handle, lease)
    }

    #[tokio::test]
    async fn refuses_while_active() {
        let mut manager = ConnectionManager::load(ProviderAuthStore::new(root("refuse"))).unwrap();
        manager.connect(input("openai", 0), &Accept).await.unwrap();
        let (mut runtime, handle, lease) = active();
        let mut turns = ActiveTurnRegistry::default();
        turns.attach("local-provider-openai".into(), handle);
        let before = manager.revision();
        let error = manager
            .disconnect(
                DisconnectProviderInput {
                    provider_id: "local-provider-openai".into(),
                    provider_name: None,
                    force: false,
                    expected_revision: before,
                },
                &mut turns,
                &mut runtime,
                &Cancellation {
                    order: Arc::default(),
                    fail: false,
                    lease,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, ConnectionError::ActiveTurns);
        assert_eq!(manager.revision(), before);
    }

    #[tokio::test]
    async fn force_cancels_then_disconnects() {
        let mut manager = ConnectionManager::load(ProviderAuthStore::new(root("force"))).unwrap();
        manager.connect(input("openai", 0), &Accept).await.unwrap();
        let (mut runtime, handle, lease) = active();
        let mut turns = ActiveTurnRegistry::default();
        turns.attach("local-provider-openai".into(), handle);
        let order = Arc::new(Mutex::new(Vec::new()));
        manager
            .disconnect(
                DisconnectProviderInput {
                    provider_id: "local-provider-openai".into(),
                    provider_name: Some("openai".into()),
                    force: true,
                    expected_revision: manager.revision(),
                },
                &mut turns,
                &mut runtime,
                &Cancellation {
                    order: order.clone(),
                    fail: false,
                    lease,
                },
            )
            .await
            .unwrap();
        order.lock().unwrap().push("remove");
        assert_eq!(*order.lock().unwrap(), ["cancel", "ack", "remove"]);
        assert!(manager.snapshots().is_empty());
    }

    #[tokio::test]
    async fn cancellation_failure_leaves_connection() {
        let mut manager =
            ConnectionManager::load(ProviderAuthStore::new(root("cancel-fail"))).unwrap();
        manager.connect(input("openai", 0), &Accept).await.unwrap();
        let (mut runtime, handle, lease) = active();
        let mut turns = ActiveTurnRegistry::default();
        turns.attach("local-provider-openai".into(), handle);
        let revision = manager.revision();
        assert_eq!(
            manager
                .disconnect(
                    DisconnectProviderInput {
                        provider_id: "local-provider-openai".into(),
                        provider_name: None,
                        force: true,
                        expected_revision: revision
                    },
                    &mut turns,
                    &mut runtime,
                    &Cancellation {
                        order: Arc::default(),
                        fail: true,
                        lease
                    }
                )
                .await
                .unwrap_err(),
            ConnectionError::Cancellation
        );
        assert_eq!(manager.revision(), revision);
        assert_eq!(manager.snapshots().len(), 1);
    }
}
