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
use tokio_util::sync::CancellationToken;

struct FakeTransport {
    pages: Mutex<VecDeque<Result<ToolsPage, McpError>>>,
    shutdowns: AtomicUsize,
    shutdown_error: bool,
}
impl FakeTransport {
    fn with_tools(names: &[&str]) -> Self {
        Self {
            pages: Mutex::new(VecDeque::from([Ok(ToolsPage {
                tools: names
                    .iter()
                    .map(|name| RemoteTool {
                        name: (*name).into(),
                        title: None,
                        description: Some("remote".into()),
                        input_schema: json!({"type":"object"}),
                    })
                    .collect(),
                next_cursor: None,
            })])),
            shutdowns: AtomicUsize::new(0),
            shutdown_error: false,
        }
    }
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
        Box::pin(future::ready(if self.shutdown_error {
            Err(McpError::Protocol)
        } else {
            Ok(())
        }))
    }
}
fn server() -> ServerId {
    ServerId::new("server".into()).unwrap()
}
fn session(value: &str) -> SessionId {
    SessionId::new(value.into()).unwrap()
}

async fn connect_transport(manager: &McpManager, session_id: &str, transport: Arc<FakeTransport>) {
    manager
        .connect(
            server(),
            session(session_id),
            transport,
            CancellationToken::new(),
        )
        .await
        .unwrap();
}

fn observe_snapshots(
    registry: Arc<ToolRegistry>,
    barrier: Arc<tokio::sync::Barrier>,
    observed: Arc<Mutex<Vec<Vec<String>>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        barrier.wait().await;
        for _ in 0..1_000 {
            let names = registry
                .snapshot()
                .unwrap()
                .model_names()
                .into_iter()
                .map(str::to_owned)
                .collect();
            observed.lock().unwrap().push(names);
            tokio::task::yield_now().await;
        }
    })
}

#[tokio::test]
async fn atomic_group_swap() {
    let registry = Arc::new(ToolRegistry::new([]).unwrap());
    let manager = McpManager::new(
        AgentId::accept("agent").unwrap(),
        Arc::clone(&registry),
        vec![],
    );
    let old = Arc::new(FakeTransport::with_tools(&["old-a", "old-b"]));
    connect_transport(&manager, "old", old.clone()).await;
    let mut failing_old = FakeTransport::with_tools(&["old-a", "old-b"]);
    failing_old.shutdown_error = true;
    let failing_old = Arc::new(failing_old);
    connect_transport(&manager, "middle", failing_old.clone()).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let reader = observe_snapshots(
        Arc::clone(&registry),
        Arc::clone(&barrier),
        Arc::clone(&observed),
    );
    barrier.wait().await;
    let result = manager
        .connect(
            server(),
            session("new"),
            Arc::new(FakeTransport::with_tools(&["new-a", "new-b"])),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    reader.await.unwrap();
    assert_eq!(
        result[0].definition.internal_name.as_str(),
        namespace("server", "new-a").unwrap()
    );
    assert_eq!(manager.group(&server()).unwrap().unwrap().1, session("new"));
    assert_eq!(old.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(failing_old.shutdowns.load(Ordering::SeqCst), 1);
    assert_atomic_snapshots(&registry, &observed);
}

fn assert_atomic_snapshots(registry: &ToolRegistry, observed: &Mutex<Vec<Vec<String>>>) {
    let old_names = vec![
        namespace("server", "old-a").unwrap(),
        namespace("server", "old-b").unwrap(),
    ];
    let new_names = vec![
        namespace("server", "new-a").unwrap(),
        namespace("server", "new-b").unwrap(),
    ];
    for names in observed.lock().unwrap().iter() {
        assert!(
            names == &old_names || names == &new_names,
            "mixed group: {names:?}"
        );
    }
    assert_eq!(registry.snapshot().unwrap().model_names(), new_names);
}

#[tokio::test]
async fn failed_refresh_is_noop() {
    let registry = Arc::new(ToolRegistry::new([]).unwrap());
    let manager = McpManager::new(
        AgentId::accept("agent").unwrap(),
        Arc::clone(&registry),
        vec![],
    );
    let old = Arc::new(FakeTransport::with_tools(&["old"]));
    manager
        .connect(server(), session("old"), old, CancellationToken::new())
        .await
        .unwrap();
    let before_group = manager.group(&server()).unwrap().unwrap();
    let before_snapshot = registry.snapshot().unwrap();
    let before_revision = registry.revision().unwrap();
    let candidate = Arc::new(FakeTransport {
        pages: Mutex::new(VecDeque::from([Err(McpError::Protocol)])),
        shutdowns: AtomicUsize::new(0),
        shutdown_error: false,
    });
    assert!(matches!(
        manager
            .connect(
                server(),
                session("new"),
                candidate.clone(),
                CancellationToken::new(),
            )
            .await,
        Err(DiscoveryError::Transport)
    ));
    let after_group = manager.group(&server()).unwrap().unwrap();
    assert!(Arc::ptr_eq(&before_group.0, &after_group.0));
    assert_eq!(before_group.1, after_group.1);
    assert!(Arc::ptr_eq(&before_snapshot, &registry.snapshot().unwrap()));
    assert_eq!(before_revision, registry.revision().unwrap());
    assert_eq!(candidate.shutdowns.load(Ordering::SeqCst), 1);
}
