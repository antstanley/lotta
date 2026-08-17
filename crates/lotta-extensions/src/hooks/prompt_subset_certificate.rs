use super::test_support::*;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
async fn accepted(event: HookEvent) {
    let model = Arc::new(Model::default());
    let registration = HookRegistration::new(owner("o"), id("h"), event, None, prompt()).unwrap();
    let runtime = runtime(vec![registration], Arc::clone(&model));
    assert_eq!(
        runtime
            .fire(payload(event), CancellationToken::new())
            .await
            .unwrap(),
        HookOutcome::Allow
    );
    assert_eq!(*model.calls.lock().unwrap(), 1);
}
macro_rules! accepts {
    ($name:ident,$event:expr) => {
        #[tokio::test]
        async fn $name() {
            accepted($event).await;
        }
    };
}
accepts!(
    pre_tool_registered_executes_model_once,
    HookEvent::PreToolUse
);
accepts!(
    post_tool_registered_executes_model_once,
    HookEvent::PostToolUse
);
accepts!(
    failure_registered_executes_model_once,
    HookEvent::PostToolUseFailure
);
accepts!(
    permission_registered_executes_model_once,
    HookEvent::PermissionRequest
);
accepts!(
    prompt_registered_executes_model_once,
    HookEvent::UserPromptSubmit
);
accepts!(stop_registered_executes_model_once, HookEvent::Stop);
accepts!(
    subagent_registered_executes_model_once,
    HookEvent::SubagentStop
);
fn rejected(event: HookEvent) {
    let model = Arc::new(Model::default());
    let registry = HookRegistry::new();
    assert_eq!(
        HookRegistration::new(owner("o"), id("h"), event, None, prompt()).unwrap_err(),
        HookLoadError::PromptUnsupported
    );
    assert!(registry.snapshot().unwrap().event(event).is_empty());
    assert_eq!(*model.calls.lock().unwrap(), 0);
}
macro_rules! rejects {
    ($name:ident,$event:expr) => {
        #[test]
        fn $name() {
            rejected($event);
        }
    };
}
rejects!(notification_rejected_before_model, HookEvent::Notification);
rejects!(pre_compact_rejected_before_model, HookEvent::PreCompact);
rejects!(session_start_rejected_before_model, HookEvent::SessionStart);
rejects!(session_end_rejected_before_model, HookEvent::SessionEnd);
