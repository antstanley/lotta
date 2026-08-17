use super::*;
use crate::hooks::{
    pipeline_certificate_support::{ExecutorAction, PipelineCertificate},
    test_support::*,
};
use lotta_runtime::ports::{ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage};
use lotta_tools::{PermissionDecision, PipelineError};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn registration(event: HookEvent, hook_id: &str) -> HookRegistration {
    HookRegistration::new(owner("owner"), id(hook_id), event, None, command()).unwrap()
}

fn pipeline_runtime(
    registrations: Vec<HookRegistration>,
    sandbox: Arc<dyn lotta_runtime::ports::SandboxPort>,
) -> Arc<RegisteredHookRuntime> {
    Arc::new(runtime_with_sandbox(
        registrations,
        Arc::new(Model::default()),
        sandbox,
    ))
}

#[tokio::test]
async fn registered_all_allow_pipeline_executes_once_and_final_is_unchanged() {
    let sandbox = Arc::new(ScriptedSandboxPort::default());
    let runtime = pipeline_runtime(
        vec![
            registration(HookEvent::PreToolUse, "pre-allow"),
            registration(HookEvent::PostToolUse, "post-allow"),
        ],
        sandbox.clone(),
    );
    let pipeline = PipelineCertificate::new(
        PermissionDecision::Allow,
        ExecutorAction::Success("unchanged".into()),
    );
    let result = pipeline.run(runtime, json!({"n": 0})).await.unwrap();
    assert_eq!(pipeline.executor_calls(), 1);
    let ToolOutcome::Success { content } = &result else {
        panic!("expected success")
    };
    assert_eq!(content.as_str(), "unchanged");
    assert_eq!(pipeline.persisted(), vec![result.clone()]);
    assert_eq!(pipeline.emitted(), vec![result]);
    assert_eq!(sandbox.lifecycle().len(), 2);
}

#[tokio::test]
async fn block_pre_stops_executor_and_pipeline_error_contains_exact_attribution() {
    let runtime = pipeline_runtime(
        vec![registration(HookEvent::PreToolUse, "executor0")],
        Arc::new(ScriptedSandboxPort::blocking("denied")),
    );
    let pipeline = PipelineCertificate::new(
        PermissionDecision::Allow,
        ExecutorAction::Success("unreachable".into()),
    );
    let error = pipeline.run(runtime, json!({"n": 0})).await.unwrap_err();
    assert_eq!(pipeline.executor_calls(), 0);
    assert_eq!(error, PipelineError::HookBlocked);
}

#[derive(Clone)]
struct SequencedSandbox {
    ports: Arc<Mutex<std::collections::VecDeque<Arc<ScriptedSandboxPort>>>>,
}
impl SequencedSandbox {
    fn new(ports: Vec<Arc<ScriptedSandboxPort>>) -> Self {
        Self {
            ports: Arc::new(Mutex::new(ports.into())),
        }
    }
}
impl lotta_runtime::ports::SandboxPort for SequencedSandbox {
    fn execute(
        &self,
        request: lotta_runtime::ports::ProcessRequest,
        events: tokio::sync::mpsc::Sender<lotta_runtime::ports::ProcessEvent>,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, lotta_runtime::ports::ProcessOutcome> {
        let port = self.ports.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { port.execute(request, events, cancellation).await })
    }
}

#[tokio::test]
async fn two_pre_command_modifications_are_sequential_and_reach_pipeline_executor() {
    let first = Arc::new(ScriptedSandboxPort::with_json(json!({
        "hookSpecificOutput": {"updatedInput": {"n": 1}}
    })));
    let second = Arc::new(ScriptedSandboxPort::with_json(json!({
        "hookSpecificOutput": {"updatedInput": {"n": 2}}
    })));
    let runtime = pipeline_runtime(
        vec![
            registration(HookEvent::PreToolUse, "executor0"),
            registration(HookEvent::PreToolUse, "executor1"),
        ],
        Arc::new(SequencedSandbox::new(vec![first, second.clone()])),
    );
    let pipeline = PipelineCertificate::new(
        PermissionDecision::Allow,
        ExecutorAction::Success("ok".into()),
    );
    pipeline.run(runtime, json!({"n": 0})).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&second.lifecycle()[0].stdin).unwrap()["tool_input"],
        json!({"n": 1})
    );
    assert_eq!(pipeline.executor_input(), Some(json!({"n": 2})));
    assert_eq!(pipeline.executor_calls(), 1);
}

#[tokio::test]
async fn post_updated_result_changes_actual_final_persisted_and_emitted_output() {
    let runtime = pipeline_runtime(
        vec![registration(HookEvent::PostToolUse, "post")],
        Arc::new(ScriptedSandboxPort::with_json(json!({
            "hookSpecificOutput": {
                "updatedResult": {"status": "success", "output": "updated"}
            }
        }))),
    );
    let pipeline = PipelineCertificate::new(
        PermissionDecision::Allow,
        ExecutorAction::Success("raw".into()),
    );
    let result = pipeline.run(runtime, json!({"n": 0})).await.unwrap();
    let ToolOutcome::Success { content } = &result else {
        panic!("expected success")
    };
    assert_eq!(content.as_str(), "updated");
    assert_eq!(pipeline.persisted(), vec![result.clone()]);
    assert_eq!(pipeline.emitted(), vec![result]);
}

#[tokio::test]
async fn raw_failure_allow_preserves_original_outcome() {
    let original = ToolOutcome::ToolDefinedError {
        code: ToolOutcomeCode::new("raw_failure".into()).unwrap(),
        message: ToolOutcomeMessage::new("original".into()).unwrap(),
    };
    let runtime = pipeline_runtime(
        vec![registration(HookEvent::PostToolUseFailure, "failure-allow")],
        Arc::new(ScriptedSandboxPort::default()),
    );
    let pipeline = PipelineCertificate::new(
        PermissionDecision::Allow,
        ExecutorAction::RawFailure(original.clone()),
    );
    let result = pipeline.run(runtime, json!({"n": 0})).await.unwrap();
    assert_eq!(result, original);
    assert_eq!(pipeline.executor_calls(), 1);
    assert_eq!(pipeline.emitted(), vec![original]);
}

#[tokio::test]
async fn failure_hook_block_fires_once_without_recursion() {
    let sandbox = Arc::new(ScriptedSandboxPort::blocking("failure blocked"));
    let runtime = pipeline_runtime(
        vec![registration(HookEvent::PostToolUseFailure, "failure-block")],
        sandbox.clone(),
    );
    let pipeline =
        PipelineCertificate::new(PermissionDecision::Allow, ExecutorAction::ExecutorError);
    let error = pipeline.run(runtime, json!({"n": 0})).await.unwrap_err();
    assert_eq!(error, PipelineError::HookBlocked);
    assert_eq!(pipeline.executor_calls(), 1);
    assert_eq!(sandbox.lifecycle().len(), 1);
}

#[tokio::test]
async fn failure_hook_executor_error_has_exact_attribution_and_fires_once() {
    let sandbox = Arc::new(ScriptedSandboxPort::with_stdout(b"not-json".to_vec()));
    let runtime = pipeline_runtime(
        vec![registration(
            HookEvent::PostToolUseFailure,
            "failure-executor",
        )],
        sandbox.clone(),
    );
    let pipeline =
        PipelineCertificate::new(PermissionDecision::Allow, ExecutorAction::ExecutorError);
    let error = pipeline.run(runtime, json!({"n": 0})).await.unwrap_err();
    let PipelineError::Hook(failure) = error else {
        panic!("expected attributed hook failure")
    };
    assert_eq!(failure.owner.as_str(), "owner");
    assert_eq!(failure.hook_id.as_str(), "failure-executor");
    assert_eq!(failure.code, "command_hook_failed");
    assert_eq!(sandbox.lifecycle().len(), 1);
}

#[tokio::test]
async fn illegal_failure_modification_has_exact_pipeline_attribution() {
    let runtime = Arc::new(IllegalFailureModify);
    let pipeline =
        PipelineCertificate::new(PermissionDecision::Allow, ExecutorAction::ExecutorError);
    let error = pipeline.run(runtime, json!({"n": 0})).await.unwrap_err();
    let PipelineError::Hook(failure) = error else {
        panic!("expected pipeline contract failure")
    };
    assert_eq!(failure.owner.as_str(), "runtime");
    assert_eq!(failure.hook_id.as_str(), "tool-pipeline");
    assert_eq!(failure.code, "illegal_modification");
}

struct IllegalFailureModify;
impl HookRuntime for IllegalFailureModify {
    fn fire(&self, payload: HookPayload, _: CancellationToken) -> HookFuture<'_> {
        let outcome = if payload.event() == HookEvent::PostToolUseFailure {
            HookOutcome::Modify(payload)
        } else {
            HookOutcome::Allow
        };
        Box::pin(std::future::ready(Ok(outcome)))
    }
}

#[tokio::test]
async fn strict_command_parser_accepts_pinned_shapes_and_rejects_invalid_shapes() {
    for (stdout, expected) in [
        (r#"{"ok":true}"#, "allow"),
        (r#"{"ok":false,"reason":"denied"}"#, "block"),
        (
            r#"{"hookSpecificOutput":{"updatedInput":{"arbitrary":true,"updatedResult":"input-key"}}}"#,
            "modify",
        ),
    ] {
        let runtime = pipeline_runtime(
            vec![registration(HookEvent::PreToolUse, "pinned")],
            Arc::new(ScriptedSandboxPort::with_stdout(stdout)),
        );
        let outcome = runtime
            .fire(
                HookPayload::new(
                    HookEvent::PreToolUse,
                    json!({
                        "event_type": HookEvent::PreToolUse,
                        "tool_name": "Read",
                        "tool_input": {"original": true}
                    }),
                )
                .unwrap(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(
            (expected, outcome),
            ("allow", HookOutcome::Allow)
                | ("block", HookOutcome::Block(_))
                | ("modify", HookOutcome::Modify(_))
        ));
    }

    for (event, stdout) in [
        (HookEvent::PreToolUse, r#"{"ok":true"#),
        (
            HookEvent::PreToolUse,
            r#"{"hookSpecificOutput":{"updatedResult":{"bad":true}}}"#,
        ),
        (
            HookEvent::PostToolUse,
            r#"{"hookSpecificOutput":{"updatedInput":{"bad":true}}}"#,
        ),
        (HookEvent::PreToolUse, r#"{"ok":true,"unknown":1}"#),
    ] {
        let runtime = pipeline_runtime(
            vec![registration(event, "strict-parser")],
            Arc::new(ScriptedSandboxPort::with_stdout(stdout)),
        );
        let field = if event == HookEvent::PreToolUse {
            "tool_input"
        } else {
            "tool_result"
        };
        let error = runtime
            .fire(
                HookPayload::new(
                    event,
                    json!({"event_type": event, "tool_name": "Read", field: {"original": true}}),
                )
                .unwrap(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.owner.as_str(), "owner");
        assert_eq!(error.hook_id.as_str(), "strict-parser");
        assert_eq!(error.code, "command_hook_failed");
    }
}
