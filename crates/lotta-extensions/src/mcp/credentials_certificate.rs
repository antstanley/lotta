use super::discovery::McpManager;
use super::{client::*, oauth::*, transport::*};
use lotta_domain::AgentId;
use lotta_tools::ToolRegistry;
use std::{
    fmt::{self, Write as _},
    future,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;
use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::prelude::*;

const MARKER: &[u8] = b"task43-marker-credential";

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);
struct Visitor(String);
impl tracing::field::Visit for Visitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.0.push_str(field.name());
        let _ = write!(self.0, "={value:?};");
    }
}
impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = Visitor(format!("{};", event.metadata().name()));
        event.record(&mut visitor);
        self.0.lock().unwrap().push(visitor.0);
    }
}

struct FailingStdio;
impl McpStdioFactoryPort for FailingStdio {
    fn launch<'a>(
        &'a self,
        _: &'a StdioConfig,
        _: Option<SecretValue>,
        _: CancellationToken,
    ) -> McpFuture<'a, Arc<dyn McpStdioPort>> {
        Box::pin(future::ready(Err(McpError::Unreachable)))
    }
}

struct CasStore(Mutex<(u64, Vec<u8>)>);
impl CredentialStorePort for CasStore {
    fn resolve(&self, _: &CredentialRef) -> CredentialFuture<'_, VersionedSecret> {
        let guard = self.0.lock().expect("test store lock");
        let result = SecretValue::new(guard.1.clone()).map(|value| VersionedSecret {
            value,
            revision: guard.0,
        });
        Box::pin(future::ready(result))
    }

    fn compare_replace(
        &self,
        _: &CredentialRef,
        expected_revision: u64,
        replacement: SecretValue,
    ) -> CredentialFuture<'_, u64> {
        let mut guard = self.0.lock().expect("test store lock");
        if guard.0 != expected_revision {
            return Box::pin(future::ready(Err(CredentialError::RevisionMismatch)));
        }
        guard.0 += 1;
        guard.1 = replacement.expose(<[u8]>::to_owned);
        Box::pin(future::ready(Ok(guard.0)))
    }
}

fn reference() -> CredentialRef {
    CredentialRef::new("credential-reference".into()).expect("valid reference")
}

#[test]
fn config_retains_only_reference_and_redacts_diagnostics() {
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "type": "http",
        "name": "server",
        "url": "https://example.test/mcp",
        "credential": "credential-reference"
    }))
    .expect("valid config");
    let debug = format!("{config:?}");
    let encoded = serde_json::to_string(&config).expect("serializable config");
    assert!(!debug.contains(std::str::from_utf8(MARKER).expect("marker utf8")));
    assert!(!encoded.contains(std::str::from_utf8(MARKER).expect("marker utf8")));
    assert!(encoded.contains("credential-reference"));
}

#[test]
fn secret_bounds_and_rendering_are_exact() {
    assert_eq!(
        SecretValue::new(Vec::new()).unwrap_err(),
        CredentialError::Invalid
    );
    let at = SecretValue::new(vec![b'x'; MCP_CREDENTIAL_BYTES_MAX]).expect("at bound");
    assert_eq!(format!("{at:?}"), "SecretValue([REDACTED])");
    assert_eq!(at.to_string(), "[REDACTED]");
    assert_eq!(
        SecretValue::new(vec![b'x'; MCP_CREDENTIAL_BYTES_MAX + 1]).unwrap_err(),
        CredentialError::Invalid
    );
}

#[tokio::test]
async fn refreshed_tokens_compare_and_replace_exact_revision() {
    let store = CasStore(Mutex::new((7, MARKER.to_vec())));
    let revision = store_refreshed_tokens(
        &store,
        &reference(),
        7,
        SecretValue::new(b"replacement".to_vec()).expect("valid secret"),
    )
    .await
    .expect("matching revision");
    assert_eq!(revision, 8);
    let mismatch = store_refreshed_tokens(
        &store,
        &reference(),
        7,
        SecretValue::new(b"discarded".to_vec()).expect("valid secret"),
    )
    .await
    .unwrap_err();
    assert_eq!(mismatch, CredentialError::RevisionMismatch);
    let stored = store
        .resolve(&reference())
        .await
        .expect("stored replacement");
    assert_eq!(stored.revision, 8);
    stored
        .value
        .expose(|value| assert_eq!(value, b"replacement"));
    assert!(
        !format!("{stored:?} {mismatch}")
            .contains(std::str::from_utf8(MARKER).expect("marker utf8"))
    );
}

#[tokio::test]
async fn actual_factory_discovery_failure_logs_never_capture_credential() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let store = Arc::new(CasStore(Mutex::new((1, MARKER.to_vec()))));
    let factory = McpClientFactory::new(
        Arc::new(super::transport_certificate::NoHttp),
        Arc::new(FailingStdio),
        store,
    );
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "type":"http","name":"server","url":"https://example.test/mcp",
        "credential":"credential-reference"
    }))
    .unwrap();
    let registry = Arc::new(ToolRegistry::new([]).unwrap());
    let manager = McpManager::new(AgentId::accept("agent").unwrap(), registry, vec![]);
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);
    tracing::debug!(url = "https://example.test/mcp", "factory connect path");
    let transport = factory
        .create(&config, CancellationToken::new())
        .await
        .unwrap();
    let Err(error) = manager
        .connect(
            ServerId::new("server".into()).unwrap(),
            SessionId::new("session".into()).unwrap(),
            transport,
            CancellationToken::new(),
        )
        .await
    else {
        panic!("transport failure expected");
    };
    let rendered = format!("{error:?} {error}");
    let logs = capture.0.lock().unwrap();
    assert!(!logs.is_empty());
    let marker = std::str::from_utf8(MARKER).unwrap();
    assert!(!rendered.contains(marker));
    assert!(logs.iter().all(|event| !event.contains(marker)));
}
