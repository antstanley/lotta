use crate::scope_context::*;
use lotta_domain::{
    AgentId, BoundedJsonValue, ConversationId, NonEmptyString, PermissionMode, RuntimeScope,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn text(value: &str) -> NonEmptyString {
    NonEmptyString::new(value).unwrap_or_else(|error| panic!("text: {error}"))
}

fn snapshot(agent: &str, lease: u64, sandbox: bool) -> ScopeContextSnapshot {
    ScopeContextSnapshot::new(ScopeContextInput {
        connection_id: text("connection-id"),
        device_id: text("device-id"),
        runtime_scope: RuntimeScope::new(
            AgentId::accept(agent).unwrap_or_else(|error| panic!("agent: {error}")),
            ConversationId::default_for_agent(),
            None,
        ),
        cwd: PathBuf::from("/secret/cwd"),
        workspace_sandbox: sandbox.then(|| {
            WorkspaceSandbox::new(
                PathBuf::from("/secret/root"),
                PathBuf::from("/secret/isolation"),
            )
        }),
        permission_mode: PermissionMode::Strict,
        selected_skills: Arc::from([text("secret-skill")]),
        tool_context: BoundedJsonValue::new(json!({"secret-token":"secret-content"}))
            .unwrap_or_else(|error| panic!("tool context: {error}")),
        cancellation_token: CancellationToken::new(),
        lease_generation: lease,
    })
    .unwrap_or_else(|error| panic!("snapshot: {error}"))
}

#[tokio::test]
async fn all_fields_round_trip_with_sandbox() {
    let expected = snapshot("agent-a", 7, true);
    scope_operation(expected, async {
        let current = try_current().unwrap_or_else(|| panic!("context missing"));
        assert_eq!(current.connection_id().as_str(), "connection-id");
        assert_eq!(current.device_id().as_str(), "device-id");
        assert_eq!(current.runtime_scope().agent_id.as_str(), "agent-a");
        assert_eq!(current.cwd(), Path::new("/secret/cwd"));
        let sandbox = current
            .workspace_sandbox()
            .unwrap_or_else(|| panic!("sandbox missing"));
        assert_eq!(sandbox.isolation_root(), Path::new("/secret/isolation"));
        assert_eq!(current.permission_mode(), PermissionMode::Strict);
        assert_eq!(current.selected_skills()[0].as_str(), "secret-skill");
        assert_eq!(
            current.tool_context().as_value(),
            &json!({"secret-token":"secret-content"})
        );
        assert_eq!(current.lease_generation(), 7);
    })
    .await;
}

#[tokio::test]
async fn optional_sandbox_can_be_absent() {
    scope_operation(snapshot("agent", 1, false), async {
        assert!(
            try_current()
                .and_then(|context| context.workspace_sandbox().cloned())
                .is_none()
        );
    })
    .await;
}

#[tokio::test]
async fn parent_context_not_inherited_by_raw_spawn() {
    scope_operation(snapshot("agent", 1, false), async {
        let child = tokio::spawn(async { try_current().is_none() });
        assert!(child.await.unwrap_or_else(|error| panic!("join: {error}")));
    })
    .await;
}

#[tokio::test]
async fn explicit_spawn_receives_same_token_and_lease() {
    let parent = snapshot("agent", 9, false);
    let token = parent.cancellation_token().clone();
    let child = spawn_scoped(parent, |explicit| async move {
        let current = try_current().unwrap_or_else(|| panic!("context missing"));
        assert_eq!(current.lease_generation(), explicit.lease_generation());
        current.cancellation_token().cancel();
    });
    child.await.unwrap_or_else(|error| panic!("join: {error}"));
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn different_tasks_are_isolated() {
    let first = spawn_scoped(snapshot("agent-a", 1, false), |context| async move {
        context.runtime_scope().agent_id.as_str().to_owned()
    });
    let second = spawn_scoped(snapshot("agent-b", 2, false), |context| async move {
        context.runtime_scope().agent_id.as_str().to_owned()
    });
    assert_eq!(
        first.await.unwrap_or_else(|error| panic!("join: {error}")),
        "agent-a"
    );
    assert_eq!(
        second.await.unwrap_or_else(|error| panic!("join: {error}")),
        "agent-b"
    );
}

#[tokio::test]
async fn outside_scope_has_no_conversation() {
    assert!(try_current().is_none());
}

#[test]
fn debug_is_redacted() {
    let output = format!("{:?}", snapshot("agent", 5, true));
    for secret in [
        "/secret/cwd",
        "/secret/root",
        "/secret/isolation",
        "secret-skill",
        "secret-token",
        "secret-content",
    ] {
        assert!(!output.contains(secret), "debug leaked {secret}: {output}");
    }
    assert!(output.contains("selected_skill_count: 1"));
    assert!(output.contains("workspace_sandbox_present: true"));
}
