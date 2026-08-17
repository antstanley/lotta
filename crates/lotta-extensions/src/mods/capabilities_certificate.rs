use super::capabilities::*;
use super::types::*;
use lotta_domain::{AgentId, ConversationId, RuntimeScope};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

struct CountingPort(AtomicUsize);
impl CapabilityPort for CountingPort {
    fn call(
        &self,
        _: &ModOwner,
        _: &ModRuntimeScope,
        _: Capability,
        _: &str,
        params: Value,
        _: CancellationToken,
    ) -> CapabilityFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(params) })
    }
}
fn scope(agent: &str, conversation: &str) -> ModRuntimeScope {
    ModRuntimeScope::new(
        RuntimeScope::new(
            AgentId::accept(agent).unwrap(),
            ConversationId::accept(conversation).unwrap(),
            None,
        ),
        "/workspace".into(),
    )
}
fn owner() -> ModOwner {
    ModOwner {
        id: ModId::new("project:mod.ts".into()).unwrap(),
        generation: Generation(1),
    }
}
fn handle() -> ConversationHandle {
    ConversationHandle::new("opaque-7".into()).unwrap()
}

#[tokio::test]
async fn undeclared_capability_refused() {
    let port = Arc::new(CountingPort(AtomicUsize::new(0)));
    let broker = CapabilityBroker::new(
        CapabilityContext::new(
            [Capability::Tools],
            owner(),
            scope("agent-a", "conversation-a"),
            handle(),
        ),
        port.clone(),
    );
    let result = broker
        .call(CapabilityCall {
            owner: owner(),
            handle: handle(),
            capability: Capability::Providers,
            operation: "register".into(),
            params: json!({}),
            cancellation: CancellationToken::new(),
        })
        .await;
    assert_eq!(result, Err(ModError::UndeclaredCapability));
    assert_eq!(port.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn forged_handle_is_refused_before_port() {
    let port = Arc::new(CountingPort(AtomicUsize::new(0)));
    let broker = CapabilityBroker::new(
        CapabilityContext::new(
            [Capability::Tools],
            owner(),
            scope("agent-a", "conversation-a"),
            handle(),
        ),
        port.clone(),
    );
    let result = broker
        .call(CapabilityCall {
            owner: owner(),
            handle: ConversationHandle::new("forged".into()).unwrap(),
            capability: Capability::Tools,
            operation: "call".into(),
            params: json!({}),
            cancellation: CancellationToken::new(),
        })
        .await;
    assert_eq!(result, Err(ModError::InvalidScope));
    assert_eq!(port.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn handle_is_scoped_to_one_conversation() {
    let table = CapabilityBrokerTable::new();
    let port = Arc::new(RecordingScopePort(std::sync::Mutex::new(Vec::new())));
    let first = table
        .mint(
            [Capability::Tools],
            owner(),
            scope("agent-a", "conversation-a"),
            port.clone(),
        )
        .unwrap();
    let second = table
        .mint(
            [Capability::Tools],
            owner(),
            scope("agent-a", "conversation-b"),
            port.clone(),
        )
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(table.len().unwrap(), 2);

    for handle in [&first, &second] {
        table
            .call(CapabilityCall {
                owner: owner(),
                handle: handle.clone(),
                capability: Capability::Tools,
                operation: "call".into(),
                params: json!({}),
                cancellation: CancellationToken::new(),
            })
            .await
            .unwrap();
    }
    // Each handle resolved to exactly the scope it was minted with, never a registry-wide accessor.
    assert_eq!(port.observed(), vec!["conversation-a", "conversation-b"]);

    table.revoke(&first).unwrap();
    let refused = table
        .call(CapabilityCall {
            owner: owner(),
            handle: first,
            capability: Capability::Tools,
            operation: "call".into(),
            params: json!({}),
            cancellation: CancellationToken::new(),
        })
        .await;
    assert_eq!(refused, Err(ModError::InvalidScope));
    assert_eq!(table.len().unwrap(), 1);
}

struct RecordingScopePort(std::sync::Mutex<Vec<String>>);
impl RecordingScopePort {
    fn observed(&self) -> Vec<String> {
        self.0.lock().expect("scope lock").clone()
    }
}
impl CapabilityPort for RecordingScopePort {
    fn call(
        &self,
        _: &ModOwner,
        scope: &ModRuntimeScope,
        _: Capability,
        _: &str,
        params: Value,
        _: CancellationToken,
    ) -> CapabilityFuture<'_> {
        self.0
            .lock()
            .expect("scope lock")
            .push(scope.runtime().conversation_id.as_str().to_owned());
        Box::pin(async move { Ok(params) })
    }
}

#[tokio::test]
async fn declared_capability_succeeds() {
    let port = Arc::new(CountingPort(AtomicUsize::new(0)));
    let broker = CapabilityBroker::new(
        CapabilityContext::new(
            [Capability::Tools],
            owner(),
            scope("agent-a", "conversation-a"),
            handle(),
        ),
        port.clone(),
    );
    let result = broker
        .call(CapabilityCall {
            owner: owner(),
            handle: handle(),
            capability: Capability::Tools,
            operation: "call".into(),
            params: json!({"ok":true}),
            cancellation: CancellationToken::new(),
        })
        .await
        .unwrap();
    assert_eq!(result, json!({"ok":true}));
    assert_eq!(port.0.load(Ordering::SeqCst), 1);
}
