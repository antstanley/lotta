use super::{client::*, discovery::*, transport::*};
use lotta_domain::AgentId;
use lotta_runtime::ports::ToolCallId;
use lotta_tools::ToolRegistry;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::Barrier;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct FakeTransport {
    pages: Mutex<VecDeque<Result<ToolsPage, McpError>>>,
    shutdowns: AtomicUsize,
}
impl McpTransport for FakeTransport {
    fn initialize(&self, _: ClientInfo, _: CancellationToken) -> McpFuture<'_, InitializeResult> {
        Box::pin(future::ready(Ok(InitializeResult {
            protocol_version: MCP_PROTOCOL_VERSION.into(),
            capabilities: json!({}),
            server_info: json!({}),
        })))
    }
    fn tools_list(&self, _: Option<&str>, _: CancellationToken) -> McpFuture<'_, ToolsPage> {
        Box::pin(future::ready(
            self.pages
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(ToolsPage {
                    tools: vec![],
                    next_cursor: None,
                })),
        ))
    }
    fn tools_call(
        &self,
        _: &ToolCallId,
        _: &str,
        _: Value,
        _: CancellationToken,
    ) -> McpFuture<'_, ToolCallResult> {
        Box::pin(future::ready(Ok(ToolCallResult {
            content: vec![],
            is_error: false,
            structured_content: None,
        })))
    }
    fn shutdown(&self) -> McpFuture<'_, ()> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        Box::pin(future::ready(Ok(())))
    }
}
fn manager() -> (McpManager, Arc<ToolRegistry>) {
    let registry = Arc::new(ToolRegistry::new([]).unwrap());
    (
        McpManager::new(
            AgentId::accept("agent").unwrap(),
            Arc::clone(&registry),
            vec![],
        ),
        registry,
    )
}
fn server(value: &str) -> ServerId {
    ServerId::new(value.into()).unwrap()
}
fn session(value: &str) -> SessionId {
    SessionId::new(value.into()).unwrap()
}
fn tool(name: &str) -> RemoteTool {
    RemoteTool {
        name: name.into(),
        title: None,
        description: Some("remote".into()),
        input_schema: json!({"type":"object"}),
    }
}

#[tokio::test]
async fn namespaces_per_server() {
    let (manager, registry) = manager();
    for name in ["space value", "literal_20", "雪"] {
        let transport = Arc::new(FakeTransport {
            pages: Mutex::new(VecDeque::from([Ok(ToolsPage {
                tools: vec![tool("same")],
                next_cursor: None,
            })])),
            ..Default::default()
        });
        manager
            .connect(
                server(name),
                session(name),
                transport,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let encoded = namespace(name, "same").unwrap();
        assert_eq!(
            decode_namespace(&encoded).unwrap(),
            (name.into(), "same".into())
        );
    }
    assert_eq!(registry.snapshot().unwrap().len(), 3);
}

#[tokio::test]
async fn server_limits_and_candidate_shutdown() {
    let (manager, registry) = manager();
    for index in 0..63 {
        manager
            .connect(
                server(&format!("s{index}")),
                session(&format!("x{index}")),
                Arc::new(FakeTransport::default()),
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    assert_eq!(manager.server_count().unwrap(), 63);
    assert_eq!(registry.snapshot().unwrap().len(), 0);
    manager
        .connect(
            server("s63"),
            session("x63"),
            Arc::new(FakeTransport::default()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(manager.server_count().unwrap(), 64);
    assert_eq!(registry.snapshot().unwrap().len(), 0);
    let revision = registry.revision().unwrap();
    let rejected = Arc::new(FakeTransport::default());
    assert!(matches!(
        manager
            .connect(
                server("above"),
                session("above"),
                rejected.clone(),
                CancellationToken::new(),
            )
            .await,
        Err(DiscoveryError::ServerLimit)
    ));
    assert_eq!(rejected.shutdowns.load(Ordering::SeqCst), 1);
    assert!(manager.group(&server("above")).unwrap().is_none());
    assert_eq!(manager.server_count().unwrap(), 64);
    assert_eq!(registry.snapshot().unwrap().len(), 0);
    assert_eq!(registry.revision().unwrap(), revision);
}

#[tokio::test]
async fn concurrent_server_boundary_installs_exactly_one() {
    let (manager, _) = manager();
    for index in 0..63 {
        manager
            .connect(
                server(&format!("base-{index}")),
                session(&format!("base-{index}")),
                Arc::new(FakeTransport::default()),
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    let manager = Arc::new(manager);
    let barrier = Arc::new(Barrier::new(3));
    let first = Arc::new(FakeTransport::default());
    let second = Arc::new(FakeTransport::default());
    let tasks = [
        ("concurrent-a", Arc::clone(&first)),
        ("concurrent-b", Arc::clone(&second)),
    ]
    .map(|(name, transport)| {
        let manager = Arc::clone(&manager);
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            manager
                .connect(
                    server(name),
                    session(name),
                    transport,
                    CancellationToken::new(),
                )
                .await
        })
    });
    barrier.wait().await;
    let [first_result, second_result] = tasks;
    let results = tokio::join!(first_result, second_result);
    assert_eq!(
        [results.0, results.1]
            .iter()
            .filter(|result| result.as_ref().unwrap().is_ok())
            .count(),
        1
    );
    assert_eq!(manager.server_count().unwrap(), 64);
    assert_eq!(
        first.shutdowns.load(Ordering::SeqCst) + second.shutdowns.load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn tool_boundaries_are_discovered() {
    for count in [511, 512] {
        let (manager, registry) = manager();
        let transport = Arc::new(FakeTransport {
            pages: Mutex::new(VecDeque::from([Ok(ToolsPage {
                tools: (0..count).map(|index| tool(&format!("t{index}"))).collect(),
                next_cursor: None,
            })])),
            ..Default::default()
        });
        let registrations = manager
            .connect(
                server("tools"),
                session("tools"),
                transport,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(registrations.len(), count);
        assert_eq!(registry.snapshot().unwrap().len(), count);
    }
    let (manager, registry) = manager();
    let rejected = Arc::new(FakeTransport {
        pages: Mutex::new(VecDeque::from([Ok(ToolsPage {
            tools: (0..513).map(|index| tool(&format!("t{index}"))).collect(),
            next_cursor: None,
        })])),
        ..Default::default()
    });
    let revision = registry.revision().unwrap();
    assert!(matches!(
        manager
            .connect(
                server("above"),
                session("above"),
                rejected.clone(),
                CancellationToken::new(),
            )
            .await,
        Err(DiscoveryError::ToolLimit)
    ));
    assert_eq!(rejected.shutdowns.load(Ordering::SeqCst), 1);
    assert!(manager.group(&server("above")).unwrap().is_none());
    assert_eq!(registry.revision().unwrap(), revision);
}

#[tokio::test]
async fn invalid_definition_shutdowns_candidate() {
    let (manager, _registry) = manager();
    let rejected = Arc::new(FakeTransport {
        pages: Mutex::new(VecDeque::from([Ok(ToolsPage {
            tools: vec![RemoteTool {
                name: "bad".into(),
                title: None,
                description: None,
                input_schema: json!({"type":"definitely-not-a-json-schema-type"}),
            }],
            next_cursor: None,
        })])),
        ..Default::default()
    });
    assert!(matches!(
        manager
            .connect(
                server("bad"),
                session("bad"),
                rejected.clone(),
                CancellationToken::new(),
            )
            .await,
        Err(DiscoveryError::Definition)
    ));
    assert_eq!(rejected.shutdowns.load(Ordering::SeqCst), 1);
}
