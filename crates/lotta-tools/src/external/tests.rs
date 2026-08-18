use super::*;
use crate::{
    AllowAllPermissions, AllowAllSandbox, OutcomeSink, PipelineError, PipelineRequest,
    RawToolExecutionRequest, RegistrySnapshot, SecretResolver, ToolRegistry, ToolsetId, TraceEvent,
    TraceSink, execute,
};
use lotta_domain::BoundedJsonValue;
use lotta_runtime::ports::{ToolOutcome, ToolTimeout, ValidatedToolInput};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn member(name: impl Into<String>) -> ExternalToolMember {
    ExternalToolMember {
        name: name.into(),
        label: None,
        description: "controller tool".to_owned(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "tool_call_id": { "type": "string" } },
            "required": ["tool_call_id"]
        }),
    }
}
fn group(scope: Option<&str>, names: impl IntoIterator<Item = String>) -> ExternalToolGroup {
    ExternalToolGroup {
        scope_id: scope.map(ToOwned::to_owned),
        tools: names.into_iter().map(member).collect(),
    }
}
fn fixture() -> (
    ExternalToolManager,
    Arc<ToolRegistry>,
    RuntimeId,
    Arc<ControllerConnection>,
    ControllerReceiver,
) {
    let registry = Arc::new(ToolRegistry::new([]).unwrap());
    let runtime = RuntimeId::new("runtime-a".to_owned()).unwrap();
    let manager = ExternalToolManager::production(runtime.clone(), Arc::clone(&registry));
    let connection_id = ConnectionId::new("connection-a".to_owned()).unwrap();
    let (connection, receiver) = manager.connect(connection_id).unwrap();
    (manager, registry, runtime, connection, receiver)
}
fn selection(scope_id: Option<&ScopeId>) -> RegistrySelection<'_> {
    RegistrySelection {
        toolset: ToolsetId::None,
        scope_id,
        allowlist: None,
    }
}

pub mod registration {
    use super::*;
    #[tokio::test]
    async fn registers_unscoped() {
        let (manager, _, runtime, connection, mut receiver) = fixture();
        let groups = [group(None, ["alpha".to_owned()])];
        let revision = manager
            .runtime_start(
                &connection,
                &runtime,
                GroupRevision::new(0),
                &groups,
                selection(None),
            )
            .unwrap();
        assert_eq!(revision.value(), 1);
        let snapshot = manager.select(selection(None)).unwrap();
        let task = tokio::spawn(pipeline(snapshot, "alpha", CancellationToken::new()));
        let call = receiver.recv().await.unwrap();
        assert_eq!(call.runtime_id, runtime);
        assert_eq!(call.internal_name.as_str(), "alpha");
        connection.respond(success(&call, "ok")).unwrap();
        assert!(
            matches!(task.await.unwrap(), ToolOutcome::Success { content } if content.as_str() == "ok")
        );
    }

    #[test]
    fn registers_scoped() {
        let (manager, _, runtime, connection, _) = fixture();
        let groups = [
            group(None, ["common".to_owned()]),
            group(Some("scope-a"), ["selected".to_owned()]),
        ];
        manager
            .runtime_start(
                &connection,
                &runtime,
                GroupRevision::new(0),
                &groups,
                selection(None),
            )
            .unwrap();
        let plain = manager.select(selection(None)).unwrap();
        assert!(plain.by_model("common").is_some());
        assert!(plain.by_model("selected").is_none());
        let scope = ScopeId::new("scope-a".to_owned()).unwrap();
        let scoped = manager.select(selection(Some(&scope))).unwrap();
        assert!(scoped.by_model("common").is_some());
        assert!(scoped.by_model("selected").is_some());
        let replacement = [group(Some("scope-a"), ["replacement".to_owned()])];
        manager
            .update(
                &connection,
                &runtime,
                GroupRevision::new(1),
                &replacement,
                selection(Some(&scope)),
            )
            .unwrap();
        let updated = manager.select(selection(Some(&scope))).unwrap();
        assert!(updated.by_model("replacement").is_some());
        assert!(updated.by_model("common").is_none());
    }

    #[test]
    fn invalid_member_rejects_group() {
        let (manager, registry, runtime, connection, _) = fixture();
        let initial = [group(None, ["stable".to_owned()])];
        manager
            .runtime_start(
                &connection,
                &runtime,
                GroupRevision::new(0),
                &initial,
                selection(None),
            )
            .unwrap();
        let before = registry.snapshot().unwrap();
        let bad = [ExternalToolGroup {
            scope_id: None,
            tools: vec![
                member("candidate"),
                ExternalToolMember {
                    name: String::new(),
                    label: None,
                    description: "bad".to_owned(),
                    parameters: serde_json::json!({"type":"object"}),
                },
            ],
        }];
        assert_eq!(
            manager.update(
                &connection,
                &runtime,
                GroupRevision::new(1),
                &bad,
                selection(None)
            ),
            Err(ExternalRegistrationError::InvalidName)
        );
        assert_eq!(manager.revision().unwrap().value(), 1);
        assert!(Arc::ptr_eq(&before, &registry.snapshot().unwrap()));
        let malformed = [ExternalToolGroup {
            scope_id: None,
            tools: vec![ExternalToolMember {
                name: "schema".to_owned(),
                label: None,
                description: "bad".to_owned(),
                parameters: serde_json::json!({"type":"not-real"}),
            }],
        }];
        assert_eq!(
            manager.update(
                &connection,
                &runtime,
                GroupRevision::new(1),
                &malformed,
                selection(None)
            ),
            Err(ExternalRegistrationError::InvalidSchema)
        );
        assert!(Arc::ptr_eq(&before, &registry.snapshot().unwrap()));
        let duplicate = [group(None, ["same".to_owned(), "same".to_owned()])];
        assert!(
            matches!(manager.update(&connection, &runtime, GroupRevision::new(1), &duplicate, selection(None)), Err(ExternalRegistrationError::DuplicateTool(name)) if name == "same")
        );
        assert!(Arc::ptr_eq(&before, &registry.snapshot().unwrap()));
    }
}

pub mod timeout {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn raw_request_deadline_cannot_shorten_fixed_five_minutes() {
        let (manager, _, runtime, connection, mut receiver) = fixture();
        manager
            .runtime_start(
                &connection,
                &runtime,
                GroupRevision::new(0),
                &[group(None, ["slow".to_owned()])],
                selection(None),
            )
            .unwrap();
        let snapshot = manager.select(selection(None)).unwrap();
        let tool = snapshot.by_model("slow").unwrap();
        let executor = Arc::clone(&tool.executor);
        let request = RawToolExecutionRequest {
            tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
                lotta_runtime::boundary::ProviderName::new("raw-call".to_owned()).unwrap(),
            ),
            input: ValidatedToolInput::new(
                BoundedJsonValue::new(serde_json::json!({"tool_call_id":"call-1"})).unwrap(),
            )
            .unwrap(),
            cancellation: CancellationToken::new(),
            deadline: ToolTimeout::new(std::time::Duration::from_millis(1)).unwrap(),
            definition: Arc::clone(&tool.definition),
            model_name: tool.model_name.clone(),
            secrets: crate::pipeline::test_empty_secret_delivery(),
        };
        let task = tokio::spawn(async move { executor.execute(request).await });
        let _request = receiver.recv().await.unwrap();
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        assert!(!task.is_finished());
        tokio::time::advance(std::time::Duration::from_millis(299_998)).await;
        assert!(!task.is_finished());
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        assert!(matches!(
            task.await.unwrap().unwrap(),
            crate::RawToolOutcome::Failure(ToolOutcome::Timeout { .. })
        ));
        assert_eq!(manager.pending_calls().unwrap(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn exact_selector_timeout_remains_fixed_five_minutes() {
        let (manager, _, runtime, connection, mut receiver) = fixture();
        manager
            .runtime_start(
                &connection,
                &runtime,
                GroupRevision::new(0),
                &[group(None, ["slow".to_owned()])],
                selection(None),
            )
            .unwrap();
        let snapshot = manager.select(selection(None)).unwrap();
        let task = tokio::spawn(pipeline(snapshot, "slow", CancellationToken::new()));
        let _request = receiver.recv().await.unwrap();
        tokio::time::advance(std::time::Duration::from_millis(299_999)).await;
        assert!(!task.is_finished());
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        assert!(matches!(task.await.unwrap(), ToolOutcome::Timeout { .. }));
        assert_eq!(manager.pending_calls().unwrap(), 0);
    }
}

pub mod owner_disconnect {
    use super::*;
    #[tokio::test]
    async fn pending_rejected_on_disconnect() {
        let (manager, _, runtime, connection, mut receiver) = fixture();
        manager
            .runtime_start(
                &connection,
                &runtime,
                GroupRevision::new(0),
                &[group(None, ["owned".to_owned()])],
                selection(None),
            )
            .unwrap();
        let task = tokio::spawn(pipeline(
            manager.select(selection(None)).unwrap(),
            "owned",
            CancellationToken::new(),
        ));
        let _request = receiver.recv().await.unwrap();
        drop(connection);
        assert!(
            matches!(task.await.unwrap(), ToolOutcome::ToolDefinedError { code, .. } if code.as_str() == "external_owner_disconnected")
        );
        assert_eq!(manager.pending_calls().unwrap(), 0);
    }

    #[tokio::test]
    async fn other_connection_cannot_answer() {
        let (manager, _, runtime, owner, mut receiver) = fixture();
        let (other, _) = manager
            .connect(ConnectionId::new("connection-b".to_owned()).unwrap())
            .unwrap();
        manager
            .runtime_start(
                &owner,
                &runtime,
                GroupRevision::new(0),
                &[group(None, ["owned".to_owned()])],
                selection(None),
            )
            .unwrap();
        let task = tokio::spawn(pipeline(
            manager.select(selection(None)).unwrap(),
            "owned",
            CancellationToken::new(),
        ));
        let call = receiver.recv().await.unwrap();
        assert_eq!(
            other.respond(success(&call, "wrong")).unwrap(),
            ResponseDisposition::OwnerMismatch
        );
        assert!(!task.is_finished());
        assert_eq!(
            owner.respond(success(&call, "right")).unwrap(),
            ResponseDisposition::Resolved
        );
        assert!(
            matches!(task.await.unwrap(), ToolOutcome::Success { content } if content.as_str() == "right")
        );
        assert_eq!(
            owner.respond(success(&call, "late")).unwrap(),
            ResponseDisposition::UnknownIgnored
        );
        let mut unknown = success(&call, "unknown");
        unknown.request_id = ExternalRequestId::new("external-tool-unknown".to_owned()).unwrap();
        assert_eq!(
            owner.respond(unknown).unwrap(),
            ResponseDisposition::UnknownIgnored
        );
    }
}

pub mod bounds {
    use super::*;
    #[test]
    fn below_at_above_total_is_atomic() {
        let (manager, registry, runtime, connection, _) = fixture();
        let below = [group(None, (0..255).map(|index| format!("below-{index}")))];
        manager
            .runtime_start(
                &connection,
                &runtime,
                GroupRevision::new(0),
                &below,
                selection(None),
            )
            .unwrap();
        let exact = [group(None, (0..256).map(|index| format!("exact-{index}")))];
        manager
            .update(
                &connection,
                &runtime,
                GroupRevision::new(1),
                &exact,
                selection(None),
            )
            .unwrap();
        let before = registry.snapshot().unwrap();
        let above = [group(None, (0..257).map(|index| format!("above-{index}")))];
        assert_eq!(
            manager.update(
                &connection,
                &runtime,
                GroupRevision::new(2),
                &above,
                selection(None)
            ),
            Err(ExternalRegistrationError::TooManyTools)
        );
        assert_eq!(manager.revision().unwrap().value(), 2);
        assert!(Arc::ptr_eq(&before, &registry.snapshot().unwrap()));
    }

    #[test]
    fn scopes_share_runtime_total_and_member_bounds() {
        let (manager, registry, runtime, connection, _) = fixture();
        let groups = [
            group(None, (0..128).map(|index| format!("common-{index}"))),
            group(
                Some("scope-a"),
                (0..128).map(|index| format!("scoped-{index}")),
            ),
        ];
        let scope = ScopeId::new("scope-a".to_owned()).unwrap();
        manager
            .runtime_start(
                &connection,
                &runtime,
                GroupRevision::new(0),
                &groups,
                selection(Some(&scope)),
            )
            .unwrap();
        assert_eq!(registry.snapshot().unwrap().len(), 256);
        let before = registry.snapshot().unwrap();
        let invalid = [group(Some(""), ["invalid".to_owned()])];
        assert_eq!(
            manager.update(
                &connection,
                &runtime,
                GroupRevision::new(1),
                &invalid,
                selection(None)
            ),
            Err(ExternalRegistrationError::InvalidScope)
        );
        assert!(Arc::ptr_eq(&before, &registry.snapshot().unwrap()));
    }
}

#[tokio::test]
async fn cancellation_is_distinct_and_removes_pending() {
    let (manager, _, runtime, connection, mut receiver) = fixture();
    manager
        .runtime_start(
            &connection,
            &runtime,
            GroupRevision::new(0),
            &[group(None, ["cancel".to_owned()])],
            selection(None),
        )
        .unwrap();
    let cancellation = CancellationToken::new();
    let task = tokio::spawn(pipeline(
        manager.select(selection(None)).unwrap(),
        "cancel",
        cancellation.clone(),
    ));
    let _request = receiver.recv().await.unwrap();
    cancellation.cancel();
    assert!(matches!(
        task.await.unwrap(),
        ToolOutcome::Interrupted { .. }
    ));
    assert_eq!(manager.pending_calls().unwrap(), 0);
}

#[test]
fn select_isolated_from_shared_registry_and_other_manager() {
    let registry = Arc::new(ToolRegistry::new([]).unwrap());
    let runtime_a = RuntimeId::new("runtime-a".to_owned()).unwrap();
    let runtime_b = RuntimeId::new("runtime-b".to_owned()).unwrap();
    let manager_a = ExternalToolManager::production(runtime_a.clone(), Arc::clone(&registry));
    let manager_b = ExternalToolManager::production(runtime_b.clone(), Arc::clone(&registry));
    let (owner_a, _) = manager_a
        .connect(ConnectionId::new("a".to_owned()).unwrap())
        .unwrap();
    let (owner_b, _) = manager_b
        .connect(ConnectionId::new("b".to_owned()).unwrap())
        .unwrap();
    manager_a
        .runtime_start(
            &owner_a,
            &runtime_a,
            GroupRevision::new(0),
            &[group(None, ["only-a".to_owned()])],
            selection(None),
        )
        .unwrap();
    manager_b
        .runtime_start(
            &owner_b,
            &runtime_b,
            GroupRevision::new(0),
            &[group(None, ["only-b".to_owned()])],
            selection(None),
        )
        .unwrap();
    let revision = registry.revision().unwrap();
    let a = manager_a.select(selection(None)).unwrap();
    let b = manager_b.select(selection(None)).unwrap();
    assert!(a.by_model("only-a").is_some() && a.by_model("only-b").is_none());
    assert!(b.by_model("only-b").is_some() && b.by_model("only-a").is_none());
    assert_eq!(registry.revision().unwrap(), revision);
}

#[test]
fn revision_exhaustion_is_atomic() {
    let (manager, registry, runtime, connection, _) = fixture();
    let before = registry.snapshot().unwrap();
    manager.set_group_revision_for_test(u64::MAX);
    assert_eq!(
        manager.runtime_start(
            &connection,
            &runtime,
            GroupRevision::new(u64::MAX),
            &[group(None, ["never".to_owned()])],
            selection(None)
        ),
        Err(ExternalRegistrationError::Allocation)
    );
    assert_eq!(manager.revision().unwrap().value(), u64::MAX);
    assert!(Arc::ptr_eq(&before, &registry.snapshot().unwrap()));
}

#[test]
fn registry_conflict_is_atomic() {
    let (manager, registry, runtime, connection, _) = fixture();
    registry.set_revision_for_test(u64::MAX);
    let before = registry.snapshot().unwrap();
    assert_eq!(
        manager.runtime_start(
            &connection,
            &runtime,
            GroupRevision::new(0),
            &[group(None, ["never".to_owned()])],
            selection(None)
        ),
        Err(ExternalRegistrationError::Allocation)
    );
    assert_eq!(manager.revision().unwrap().value(), 0);
    assert!(Arc::ptr_eq(&before, &registry.snapshot().unwrap()));
}

#[tokio::test]
async fn malformed_response_does_not_settle_legitimate_call() {
    let (manager, _, runtime, connection, mut receiver) = fixture();
    manager
        .runtime_start(
            &connection,
            &runtime,
            GroupRevision::new(0),
            &[group(None, ["correlated".to_owned()])],
            selection(None),
        )
        .unwrap();
    let task = tokio::spawn(pipeline(
        manager.select(selection(None)).unwrap(),
        "correlated",
        CancellationToken::new(),
    ));
    let call = receiver.recv().await.unwrap();
    let mut wrong = success(&call, "wrong");
    wrong.tool_call_id = ToolCallId::new("wrong-call".to_owned()).unwrap();
    assert_eq!(
        connection.respond(wrong).unwrap(),
        ResponseDisposition::InvalidResponse
    );
    assert_eq!(manager.pending_calls().unwrap(), 1);
    assert_eq!(
        connection.respond(success(&call, "right")).unwrap(),
        ResponseDisposition::Resolved
    );
    assert!(
        matches!(task.await.unwrap(), ToolOutcome::Success { content } if content.as_str() == "right")
    );
}

#[tokio::test]
async fn cloned_handle_and_explicit_close_have_exact_lifecycle() {
    let (manager, _, runtime, connection, mut receiver) = fixture();
    manager
        .runtime_start(
            &connection,
            &runtime,
            GroupRevision::new(0),
            &[group(None, ["owned".to_owned()])],
            selection(None),
        )
        .unwrap();
    let clone = Arc::clone(&connection);
    drop(connection);
    let task = tokio::spawn(pipeline(
        manager.select(selection(None)).unwrap(),
        "owned",
        CancellationToken::new(),
    ));
    let _ = receiver.recv().await.unwrap();
    assert_eq!(manager.pending_calls().unwrap(), 1);
    clone.close();
    clone.close();
    assert!(
        matches!(task.await.unwrap(), ToolOutcome::ToolDefinedError { code, .. } if code.as_str() == "external_owner_disconnected")
    );
    assert_eq!(manager.pending_calls().unwrap(), 0);
}

#[tokio::test]
async fn caller_metadata_produces_distinct_calls() {
    let (manager, _, runtime, connection, mut receiver) = fixture();
    manager
        .runtime_start(
            &connection,
            &runtime,
            GroupRevision::new(0),
            &[group(None, ["ids".to_owned()])],
            selection(None),
        )
        .unwrap();
    let snapshot = manager.select(selection(None)).unwrap();
    let one = tokio::spawn(pipeline(
        Arc::clone(&snapshot),
        "ids",
        CancellationToken::new(),
    ));
    let two = tokio::spawn(pipeline(snapshot, "ids", CancellationToken::new()));
    let first = receiver.recv().await.unwrap();
    let second = receiver.recv().await.unwrap();
    assert_ne!(first.request_id, second.request_id);
    connection.respond(success(&first, "one")).unwrap();
    connection.respond(success(&second, "two")).unwrap();
    let _ = (one.await.unwrap(), two.await.unwrap());
}

#[tokio::test]
async fn request_id_exhaustion_is_typed_and_leaves_no_pending() {
    let (manager, _, runtime, connection, _) = fixture();
    manager
        .runtime_start(
            &connection,
            &runtime,
            GroupRevision::new(0),
            &[group(None, ["ids".to_owned()])],
            selection(None),
        )
        .unwrap();
    manager.set_request_sequence_for_test(u64::MAX);
    let outcome = pipeline(
        manager.select(selection(None)).unwrap(),
        "ids",
        CancellationToken::new(),
    )
    .await;
    assert!(matches!(outcome, ToolOutcome::ToolDefinedError { .. }));
    assert_eq!(manager.pending_calls().unwrap(), 0);
}

fn success(request: &ExternalCallRequest, value: &str) -> ExternalCallResponse {
    ExternalCallResponse {
        runtime_id: request.runtime_id.clone(),
        request_id: request.request_id.clone(),
        tool_call_id: request.tool_call_id.clone(),
        internal_name: request.internal_name.clone(),
        model_name: request.model_name.clone(),
        scope_id: request.scope_id.clone(),
        result: Some(serde_json::Value::String(value.to_owned())),
        error: None,
    }
}

async fn pipeline(
    registry: Arc<RegistrySnapshot>,
    name: &'static str,
    cancellation: CancellationToken,
) -> ToolOutcome {
    let trace = RecordingTrace(Mutex::new(Vec::new()));
    let sink = Sink;
    let result = execute(PipelineRequest {
        approval_grant: lotta_runtime::ports::ToolApprovalGrant::None,
        tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
            lotta_runtime::boundary::ProviderName::new("pipeline-call".to_owned()).unwrap(),
        ),
        registry,
        model_name: name,
        input: BoundedJsonValue::new(serde_json::json!({"tool_call_id":"call-1"})).unwrap(),
        cancellation,
        hook_runtime: &lotta_runtime::hooks::NoopHookRuntime,
        permissions: &AllowAllPermissions,
        sandbox: &AllowAllSandbox,
        secrets: &NoSecrets,
        trace: &trace,
        overflow: &NoOverflow,
        persistence: &sink,
        emit: &sink,
    })
    .await
    .unwrap();
    assert_eq!(trace.0.lock().unwrap().len(), 12);
    result
}
struct RecordingTrace(Mutex<Vec<TraceEvent>>);
impl TraceSink for RecordingTrace {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}
struct Sink;
impl OutcomeSink for Sink {
    fn record(&self, _: &str, _: &ToolOutcome) -> Result<(), PipelineError> {
        Ok(())
    }
}
struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn resolve(&self, _: &str) -> Result<Option<String>, PipelineError> {
        Ok(None)
    }
}
struct NoOverflow;
impl crate::clamp::OverflowWriter for NoOverflow {
    fn write(&self, _: &str, _: &str) -> Result<String, crate::clamp::ClampError> {
        Ok("unused".to_owned())
    }
}
