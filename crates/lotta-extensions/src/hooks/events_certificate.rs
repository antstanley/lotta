use super::{HookEvent, HookFuture, HookOutcome, HookPayload, HookRuntime};
use crate::hooks::pipeline_certificate_support::{ExecutorAction, PipelineCertificate};
use lotta_runtime::hooks::HookLifecycleHost;
use lotta_tools::{PermissionDecision, PipelineError, PipelineStage, PreflightEvent, TraceEvent};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn test_payload(event: HookEvent) -> HookPayload {
    HookPayload::new(event, json!({"event_type": event})).unwrap()
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<HookEvent>>,
    payloads: Mutex<Vec<serde_json::Value>>,
    modify_pre: bool,
}
impl HookRuntime for Recorder {
    fn fire(&self, payload: HookPayload, _: CancellationToken) -> HookFuture<'_> {
        self.events.lock().unwrap().push(payload.event());
        self.payloads.lock().unwrap().push(payload.value().clone());
        let outcome = if self.modify_pre && payload.event() == HookEvent::PreToolUse {
            let mut value = payload.value().clone();
            value["tool_input"] = json!({"n": 2});
            HookOutcome::Modify(HookPayload::new(HookEvent::PreToolUse, value).unwrap())
        } else {
            HookOutcome::Allow
        };
        Box::pin(std::future::ready(Ok(outcome)))
    }
}

async fn before(
    event: HookEvent,
    invoke: impl for<'a> FnOnce(
        &'a HookLifecycleHost<'a>,
        HookPayload,
        Box<dyn FnOnce() -> std::future::Ready<Result<(), ()>> + Send>,
    ) -> lotta_runtime::hooks::LifecycleFuture<
        'a,
        (),
        lotta_runtime::hooks::LifecycleError<()>,
    >,
) {
    let recorder = Recorder::default();
    let host = HookLifecycleHost::new(&recorder);
    let trace = Arc::new(Mutex::new(Vec::new()));
    let operation_trace = Arc::clone(&trace);
    invoke(
        &host,
        test_payload(event),
        Box::new(move || {
            operation_trace.lock().unwrap().push("operation");
            std::future::ready(Ok(()))
        }),
    )
    .await
    .unwrap();
    assert_eq!(*recorder.events.lock().unwrap(), [event]);
    assert_eq!(*trace.lock().unwrap(), ["operation"]);
}
macro_rules! before_case {
    ($name:ident, $event:expr, $method:ident) => {
        #[tokio::test]
        async fn $name() {
            before($event, |host, payload, operation| {
                Box::pin(host.$method(payload, CancellationToken::new(), operation))
            })
            .await;
        }
    };
}
before_case!(
    user_prompt_submit_before_operation_once,
    HookEvent::UserPromptSubmit,
    user_prompt_submit
);
before_case!(
    notification_before_operation_once,
    HookEvent::Notification,
    notification
);
before_case!(
    pre_compact_before_operation_once,
    HookEvent::PreCompact,
    pre_compact
);
before_case!(
    session_end_before_operation_once,
    HookEvent::SessionEnd,
    session_end
);
#[tokio::test]
async fn stop_after_operation_once() {
    let recorder = Recorder::default();
    HookLifecycleHost::new(&recorder)
        .stop(
            test_payload(HookEvent::Stop),
            CancellationToken::new(),
            || async { Ok::<_, ()>(()) },
        )
        .await
        .unwrap();
    assert_eq!(*recorder.events.lock().unwrap(), [HookEvent::Stop]);
}
#[tokio::test]
async fn subagent_stop_after_operation_once() {
    let recorder = Recorder::default();
    HookLifecycleHost::new(&recorder)
        .subagent_stop(
            test_payload(HookEvent::SubagentStop),
            CancellationToken::new(),
            || async { Ok::<_, ()>(()) },
        )
        .await
        .unwrap();
    assert_eq!(*recorder.events.lock().unwrap(), [HookEvent::SubagentStop]);
}
#[tokio::test]
async fn session_start_after_operation_once() {
    let recorder = Recorder::default();
    HookLifecycleHost::new(&recorder)
        .session_start(
            test_payload(HookEvent::SessionStart),
            CancellationToken::new(),
            || async { Ok::<_, ()>(()) },
        )
        .await
        .unwrap();
    assert_eq!(*recorder.events.lock().unwrap(), [HookEvent::SessionStart]);
}

#[tokio::test]
async fn pre_tool_use_once_before_permission_and_modification_reaches_executor() {
    let runtime = Arc::new(Recorder {
        modify_pre: true,
        ..Recorder::default()
    });
    let pipeline = PipelineCertificate::new(
        PermissionDecision::Allow,
        ExecutorAction::Success("raw".into()),
    );
    pipeline
        .run(runtime.clone(), json!({"n": 1}))
        .await
        .unwrap();
    assert_eq!(
        *runtime.events.lock().unwrap(),
        [HookEvent::PreToolUse, HookEvent::PostToolUse]
    );
    assert_eq!(pipeline.executor_input(), Some(json!({"n": 2})));
    assert_eq!(pipeline.executor_calls(), 1);
    assert_eq!(pipeline.trace(), success_trace());
}

#[tokio::test]
async fn permission_request_fires_once_only_for_ask_before_approval() {
    for (decision, expected_events, expected_trace, expected_error) in [
        (
            PermissionDecision::Allow,
            vec![HookEvent::PreToolUse, HookEvent::PostToolUse],
            success_trace(),
            None,
        ),
        (
            PermissionDecision::Deny,
            vec![HookEvent::PreToolUse],
            vec![
                TraceEvent::Preflight(PreflightEvent::NameResolution),
                TraceEvent::Preflight(PreflightEvent::SchemaValidation),
                TraceEvent::Stage(PipelineStage::PreHook),
                TraceEvent::Stage(PipelineStage::Permission),
            ],
            Some(PipelineError::PermissionDenied),
        ),
        (
            PermissionDecision::Ask,
            vec![HookEvent::PreToolUse, HookEvent::PermissionRequest],
            vec![
                TraceEvent::Preflight(PreflightEvent::NameResolution),
                TraceEvent::Preflight(PreflightEvent::SchemaValidation),
                TraceEvent::Stage(PipelineStage::PreHook),
                TraceEvent::Stage(PipelineStage::Permission),
                TraceEvent::Stage(PipelineStage::PermissionHook),
            ],
            Some(PipelineError::ApprovalRequired),
        ),
    ] {
        let runtime = Arc::new(Recorder::default());
        let pipeline = PipelineCertificate::new(decision, ExecutorAction::Success("raw".into()));
        let result = pipeline.run(runtime.clone(), json!({"n": 1})).await;
        assert_eq!(*runtime.events.lock().unwrap(), expected_events);
        assert_eq!(pipeline.trace(), expected_trace);
        assert_eq!(result.err(), expected_error);
    }
}

#[tokio::test]
async fn post_tool_use_once_only_after_executor_success() {
    let runtime = Arc::new(Recorder::default());
    let pipeline = PipelineCertificate::new(
        PermissionDecision::Allow,
        ExecutorAction::Success("raw".into()),
    );
    pipeline
        .run(runtime.clone(), json!({"n": 1}))
        .await
        .unwrap();
    assert_eq!(
        *runtime.events.lock().unwrap(),
        [HookEvent::PreToolUse, HookEvent::PostToolUse]
    );
    assert_eq!(pipeline.executor_calls(), 1);
    assert_eq!(pipeline.trace(), success_trace());
}

#[tokio::test]
async fn post_tool_use_failure_once_for_executor_error_and_raw_failure() {
    let failure = lotta_runtime::ports::ToolOutcome::ToolDefinedError {
        code: lotta_runtime::ports::ToolOutcomeCode::new("failed".into()).unwrap(),
        message: lotta_runtime::ports::ToolOutcomeMessage::new("failed".into()).unwrap(),
    };
    for (action, expected_trace) in [
        (
            ExecutorAction::ExecutorError,
            success_trace().into_iter().take(8).collect::<Vec<_>>(),
        ),
        (ExecutorAction::RawFailure(failure), success_trace()),
    ] {
        let runtime = Arc::new(Recorder::default());
        let pipeline = PipelineCertificate::new(PermissionDecision::Allow, action);
        let _ = pipeline.run(runtime.clone(), json!({"n": 1})).await;
        assert_eq!(
            *runtime.events.lock().unwrap(),
            [HookEvent::PreToolUse, HookEvent::PostToolUseFailure]
        );
        assert_eq!(pipeline.executor_calls(), 1);
        assert_eq!(pipeline.trace(), expected_trace);
    }
}

fn success_trace() -> Vec<TraceEvent> {
    vec![
        TraceEvent::Preflight(PreflightEvent::NameResolution),
        TraceEvent::Preflight(PreflightEvent::SchemaValidation),
        TraceEvent::Stage(PipelineStage::PreHook),
        TraceEvent::Stage(PipelineStage::Permission),
        TraceEvent::Stage(PipelineStage::Sandbox),
        TraceEvent::Stage(PipelineStage::SecretSubstitution),
        TraceEvent::Stage(PipelineStage::Executor),
        TraceEvent::Stage(PipelineStage::PostHook),
        TraceEvent::Stage(PipelineStage::Scrub),
        TraceEvent::Stage(PipelineStage::Clamp),
        TraceEvent::Stage(PipelineStage::Persist),
        TraceEvent::Stage(PipelineStage::Emit),
    ]
}
