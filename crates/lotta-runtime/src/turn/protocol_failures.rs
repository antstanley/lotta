use super::test_support::*;
use super::*;
use crate::boundary::{ProviderEventText, ProviderName};
use crate::ports::{ProviderError, ProviderErrorContext, ProviderEvent, StopReason};
use lotta_domain::TurnStateKind;

async fn assert_failure(
    scripts: Vec<ProviderScript>,
    outcomes: Vec<Result<crate::ports::ToolOutcome, crate::RuntimeError>>,
    catalog_names: &[&str],
    expected: crate::RuntimeError,
) {
    let provider = ScriptedProvider::configured(scripts);
    let tool = SequencingTool::configured(outcomes, None);
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let error = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        request(),
        TurnPorts::direct(&provider, &tool, &catalog(catalog_names), &effects),
    )
    .await
    .unwrap_err();
    assert_eq!(error, expected);
    assert!(
        !effects
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, TurnEvent::Finished { .. }))
    );
    assert_ne!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

#[tokio::test]
async fn clean_close_without_terminal() {
    assert_failure(
        vec![ProviderScript::Events(vec![])],
        vec![],
        &[],
        crate::RuntimeError::InvalidData {
            context: "provider stream closed without terminal".into(),
        },
    )
    .await;
}

#[tokio::test]
async fn two_stops() {
    assert_failure(
        vec![ProviderScript::Events(vec![
            ProviderEvent::Stop {
                reason: StopReason::EndTurn,
            },
            ProviderEvent::Stop {
                reason: StopReason::EndTurn,
            },
        ])],
        vec![],
        &[],
        crate::RuntimeError::InvalidData {
            context: "provider event after terminal".into(),
        },
    )
    .await;
}

#[tokio::test]
async fn event_after_stop() {
    assert_failure(
        vec![ProviderScript::Events(vec![
            ProviderEvent::Stop {
                reason: StopReason::EndTurn,
            },
            ProviderEvent::TextDelta { text: text("late") },
        ])],
        vec![],
        &[],
        crate::RuntimeError::InvalidData {
            context: "provider event after terminal".into(),
        },
    )
    .await;
}

#[tokio::test]
async fn incomplete_call_at_stop() {
    let call_id = id("incomplete");
    assert_failure(
        vec![ProviderScript::Events(vec![
            ProviderEvent::ToolCallStart {
                call_id,
                name: text("tool"),
            },
            ProviderEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ])],
        vec![],
        &["tool"],
        crate::RuntimeError::InvalidData {
            context: "provider tool call incomplete at stop".into(),
        },
    )
    .await;
}

#[tokio::test]
async fn provider_event_error_finishes_with_typed_reason() {
    let error = ProviderError::Unknown(ProviderErrorContext {
        retry_after: None,
        code: ProviderName::new("fake".into()).unwrap(),
        context: ProviderEventText::new("safe".into()).unwrap(),
    });
    let provider =
        ScriptedProvider::configured(vec![ProviderScript::Events(vec![ProviderEvent::Error {
            error,
        }])]);
    let tool = SequencingTool::configured(vec![], None);
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        request(),
        TurnPorts::direct(&provider, &tool, &catalog(&[]), &effects),
    )
    .await;
    assert_eq!(outcome, Ok(TurnRunOutcome::Completed));
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
    assert!(effects.events.lock().unwrap().iter().any(|event| matches!(
        event,
        TurnEvent::Failed {
            reason: TurnStopReason::TransportFailure
        }
    )));
}

#[tokio::test]
async fn configured_provider_port_failure() {
    let expected = crate::RuntimeError::Timeout {
        context: "provider port".into(),
    };
    assert_failure(
        vec![ProviderScript::Error(expected.clone())],
        vec![],
        &[],
        expected,
    )
    .await;
}

#[tokio::test]
async fn tool_result_with_end_turn_is_protocol_failure() {
    let mut script = call_events("call", "tool");
    script.push(ProviderEvent::Stop {
        reason: StopReason::EndTurn,
    });
    assert_failure(
        vec![ProviderScript::Events(script)],
        vec![Ok(outcome("ok"))],
        &["tool"],
        crate::RuntimeError::InvalidData {
            context: "provider tool result with non-tool stop".into(),
        },
    )
    .await;
}
