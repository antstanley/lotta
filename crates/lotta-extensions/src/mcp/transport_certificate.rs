use super::{client::*, oauth::*, transport::*};
use crate::sidecar::{
    SIDECAR_PROTOCOL_VERSION, SidecarCapability, SidecarEnvelope, SidecarEnvelopeKind,
    SidecarFrameLimit, SidecarOwnerIdentity,
    framing::{read_frame, write_frame},
};
use serde_json::{Value, json};
use std::{
    future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

struct Store;
impl CredentialStorePort for Store {
    fn resolve(&self, _: &CredentialRef) -> CredentialFuture<'_, VersionedSecret> {
        Box::pin(future::ready(Err(CredentialError::Unknown)))
    }
    fn compare_replace(
        &self,
        _: &CredentialRef,
        _: u64,
        _: SecretValue,
    ) -> CredentialFuture<'_, u64> {
        Box::pin(future::ready(Err(CredentialError::RevisionMismatch)))
    }
}
pub(super) struct NoHttp;
impl McpHttpPort for NoHttp {
    fn request(&self, _: McpHttpRequest, _: CancellationToken) -> HttpFuture<'_> {
        Box::pin(future::ready(Err(McpError::Unreachable)))
    }
    fn open_sse(
        &self,
        _: McpHttpRequest,
        _: CancellationToken,
    ) -> SseFuture<'_, (McpHttpResponse, Arc<dyn McpSseStream>)> {
        Box::pin(future::ready(Err(McpError::Unreachable)))
    }
}
#[derive(Default)]
struct Counts {
    admitted: AtomicUsize,
    launched: AtomicUsize,
    stopped: AtomicUsize,
    joined: AtomicUsize,
}
struct Child(Arc<Counts>);
impl McpStdioChild for Child {
    fn stop(&mut self) -> McpChildFuture<'_> {
        self.0.stopped.fetch_add(1, Ordering::SeqCst);
        Box::pin(future::ready(Ok(())))
    }
    fn join(&mut self) -> McpChildFuture<'_> {
        self.0.joined.fetch_add(1, Ordering::SeqCst);
        Box::pin(future::ready(Ok(())))
    }
}
struct Launcher {
    owner: SidecarOwnerIdentity,
    counts: Arc<Counts>,
    outbound: Arc<Mutex<Vec<SidecarEnvelope>>>,
    unsupported: bool,
}
impl McpStdioChildLauncher for Launcher {
    fn admit(&self) -> Result<(), crate::sidecar::supervisor::RestartLimitError> {
        self.counts.admitted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn launch(&self, _: McpStdioLaunch<'_>) -> McpLaunchFuture<'_> {
        self.counts.launched.fetch_add(1, Ordering::SeqCst);
        let owner = self.owner.clone();
        let counts = Arc::clone(&self.counts);
        let outbound = Arc::clone(&self.outbound);
        let unsupported = self.unsupported;
        Box::pin(async move { Ok(test_child(owner, counts, outbound, unsupported)) })
    }
}

fn test_child(
    owner: SidecarOwnerIdentity,
    counts: Arc<Counts>,
    outbound: Arc<Mutex<Vec<SidecarEnvelope>>>,
    unsupported: bool,
) -> McpStdioChildConnection {
    let (host_read, child_write) = duplex(65_536);
    let (child_read, host_write) = duplex(65_536);
    tokio::spawn(child_script(
        child_read,
        child_write,
        owner.clone(),
        outbound,
        unsupported,
    ));
    McpStdioChildConnection {
        reader: Box::new(host_read),
        writer: Box::new(host_write),
        child: Box::new(Child(counts)),
        owner,
    }
}

async fn child_script(
    mut child_read: tokio::io::DuplexStream,
    mut child_write: tokio::io::DuplexStream,
    owner: SidecarOwnerIdentity,
    outbound: Arc<Mutex<Vec<SidecarEnvelope>>>,
    unsupported: bool,
) {
    let version = if unsupported {
        crate::sidecar::SidecarProtocolVersion(2)
    } else {
        SIDECAR_PROTOCOL_VERSION
    };
    let hello = envelope(
        owner.clone(),
        version,
        SidecarEnvelopeKind::Hello,
        json!({}),
        "hello",
    );
    write_frame(
        &mut child_write,
        SidecarFrameLimit::bounded(MCP_MESSAGE_BYTES_MAX),
        &hello,
    )
    .await
    .unwrap();
    if unsupported {
        return;
    }
    while let Ok(request) = read_frame::<_, SidecarEnvelope>(
        &mut child_read,
        SidecarFrameLimit::bounded(MCP_MESSAGE_BYTES_MAX),
    )
    .await
    {
        outbound.lock().unwrap().push(request.clone());
        respond_to_request(&mut child_write, &owner, &request).await;
    }
}

async fn respond_to_request(
    child_write: &mut tokio::io::DuplexStream,
    owner: &SidecarOwnerIdentity,
    request: &SidecarEnvelope,
) {
    let Some(id) = request.payload.get("id").cloned() else {
        return;
    };
    let result = if request.payload["method"] == "initialize" {
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": {}
        })
    } else {
        json!({"tools":[]})
    };
    let response = envelope(
        owner.clone(),
        SIDECAR_PROTOCOL_VERSION,
        SidecarEnvelopeKind::Response,
        json!({"jsonrpc":"2.0","id":id,"result":result}),
        "response",
    );
    write_frame(
        child_write,
        SidecarFrameLimit::bounded(MCP_MESSAGE_BYTES_MAX),
        &response,
    )
    .await
    .unwrap();
}
fn envelope(
    owner: SidecarOwnerIdentity,
    version: crate::sidecar::SidecarProtocolVersion,
    kind: SidecarEnvelopeKind,
    payload: Value,
    request_id: &str,
) -> SidecarEnvelope {
    SidecarEnvelope {
        version,
        owner,
        capability: SidecarCapability::Mcp,
        timeout_ms: 1_000,
        request_id: request_id.into(),
        correlation_id: None,
        kind,
        payload,
    }
}
fn config(value: Value) -> McpServerConfig {
    serde_json::from_value(value).unwrap()
}
fn launcher(unsupported: bool) -> (Arc<Launcher>, Arc<Counts>, Arc<Mutex<Vec<SidecarEnvelope>>>) {
    let counts = Arc::new(Counts::default());
    let outbound = Arc::new(Mutex::new(Vec::new()));
    (
        Arc::new(Launcher {
            owner: SidecarOwnerIdentity::new("agent", "runtime", "conversation").unwrap(),
            counts: Arc::clone(&counts),
            outbound: Arc::clone(&outbound),
            unsupported,
        }),
        counts,
        outbound,
    )
}

#[tokio::test]
async fn stdio() {
    let (launcher, counts, outbound) = launcher(false);
    let stdio = Arc::new(
        Task42McpStdioFactory::new(launcher)
            .with_timeouts(Duration::from_secs(1), Duration::from_secs(1)),
    );
    let factory = McpClientFactory::new(Arc::new(NoHttp), stdio, Arc::new(Store));
    let transport = factory
        .create(
            &config(json!({"type":"stdio","name":"local","command":"/bin/server"})),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    transport
        .initialize(
            ClientInfo {
                name: "lotta".into(),
                version: "1".into(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    transport.shutdown().await.unwrap();
    let sent = outbound.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].version, SIDECAR_PROTOCOL_VERSION);
    assert_eq!(sent[0].capability, SidecarCapability::Mcp);
    assert_eq!(sent[0].timeout_ms, 1_000);
    assert_eq!(sent[0].kind, SidecarEnvelopeKind::Request);
    assert_eq!(sent[1].payload["method"], "notifications/initialized");
    assert_eq!(counts.admitted.load(Ordering::SeqCst), 1);
    assert_eq!(counts.launched.load(Ordering::SeqCst), 1);
    assert_eq!(counts.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(counts.joined.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn omitted_type_defaults_to_stdio() {
    let (good_launcher, counts, _) = launcher(false);
    let factory = McpClientFactory::new(
        Arc::new(NoHttp),
        Arc::new(Task42McpStdioFactory::new(good_launcher)),
        Arc::new(Store),
    );
    let transport = factory
        .create(
            &config(json!({"name":"local","command":"/bin/server"})),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    transport.shutdown().await.unwrap();
    assert_eq!(counts.launched.load(Ordering::SeqCst), 1);
    let (launcher, counts, outbound) = launcher(true);
    let factory = McpClientFactory::new(
        Arc::new(NoHttp),
        Arc::new(Task42McpStdioFactory::new(launcher)),
        Arc::new(Store),
    );
    assert!(
        factory
            .create(
                &config(json!({"name":"bad","command":"/bin/server"})),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(outbound.lock().unwrap().is_empty());
    assert_eq!(counts.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(counts.joined.load(Ordering::SeqCst), 1);
}

#[test]
fn sse() {
    super::tests::sse();
}

#[test]
fn http() {
    super::tests::http();
}

#[test]
fn one_codec() {
    let source = include_str!("transport/stdio.rs");
    assert_eq!(source.matches("write_frame(").count(), 2);
    assert!(!source.contains("to_be_bytes()"));
}
