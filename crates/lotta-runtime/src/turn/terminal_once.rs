use super::test_support::*;
use super::*;
use crate::ports::{ProviderEvent, StopReason};
use lotta_domain::{RunId, TurnStateKind};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn exactly_one_terminal_after_final_delta_and_release() {
    let provider = ScriptedProvider::new(vec![vec![
        ProviderEvent::TextDelta { text: text("last") },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(
        *effects.events.lock().unwrap(),
        [
            TurnEvent::StreamDelta(TurnProjection::new(ProjectionKind::Text, text("last"))),
            TurnEvent::Finished {
                reason: StopReason::EndTurn
            }
        ]
    );
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

#[tokio::test]
async fn stale_real_replacement_is_suppressed_before_dispatch() {
    let (mut runtime, handle, stale) = runtime();
    runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .finish_turn(&stale, lotta_domain::StopReason::new("done").unwrap())
        .unwrap();
    let current = runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .begin_turn("next".into(), RunId::generate_sequence(2).unwrap())
        .unwrap();
    let provider = ScriptedProvider::new(vec![vec![ProviderEvent::Stop {
        reason: StopReason::EndTurn,
    }]]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        stale,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Suppressed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(tool.calls.load(Ordering::SeqCst), 0);
    assert!(effects.events.lock().unwrap().is_empty());
    assert!(runtime.lifecycle(&handle).unwrap().is_current(&current));
}

#[tokio::test]
async fn cancellation_during_tool_execute_suppresses_all_later_effects() {
    let req = request();
    let token = req.cancellation.clone();
    let mut script = call_events("cancel", "tool");
    script.push(ProviderEvent::Stop {
        reason: StopReason::ToolUse,
    });
    let provider = ScriptedProvider::new(vec![script]);
    let tool = SequencingTool::configured(vec![Ok(outcome("ignored"))], Some(token));
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        req,
        TurnPorts::new(&provider, &tool, &catalog(&["tool"]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Suppressed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(effects.events.lock().unwrap().is_empty());
    assert!(effects.results.lock().unwrap().is_empty());
    assert_ne!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

#[tokio::test]
async fn cancellation_during_finished_effect_still_linearizes_completion() {
    let req = request();
    let effects = RecordingEffects::configured(Some(req.cancellation.clone()), false);
    let provider = ScriptedProvider::new(vec![vec![ProviderEvent::Stop {
        reason: StopReason::EndTurn,
    }]]);
    let tool = SequencingTool::new(Vec::new());
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        req,
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(effects.events.lock().unwrap().len(), 1);
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

#[tokio::test]
async fn cancellation_before_terminal_helper_suppresses_without_release() {
    let req = request();
    let provider = ScriptedProvider::configured(vec![ProviderScript::CancelBefore(
        vec![ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        }],
        req.cancellation.clone(),
    )]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        req,
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Suppressed);
    assert!(effects.events.lock().unwrap().is_empty());
    assert_ne!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

#[tokio::test]
async fn finished_effect_failure_retains_active_owner() {
    let provider = ScriptedProvider::new(vec![vec![ProviderEvent::Stop {
        reason: StopReason::EndTurn,
    }]]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::configured(None, true);
    let (mut runtime, handle, lease) = runtime();
    assert!(
        run_turn(
            &mut runtime,
            handle.clone(),
            lease,
            request(),
            TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
        )
        .await
        .is_err()
    );
    assert_ne!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}
