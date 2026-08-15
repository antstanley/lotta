use super::test_support::*;
use super::*;
use crate::bounds::{TURN_RESOURCE_BOUNDS, TURN_STEPS_MAX, TURN_TOOL_CALLS_MAX};
use crate::ports::{ProviderEvent, StopReason};
use std::sync::atomic::Ordering;

#[test]
fn turn_bound_table_exact_values_and_boundaries() {
    assert_eq!(TURN_RESOURCE_BOUNDS.len(), 2);
    assert_eq!(TURN_TOOL_CALLS_MAX.name, "TURN_TOOL_CALLS_MAX");
    assert_eq!(TURN_STEPS_MAX.name, "TURN_STEPS_MAX");
    for bound in TURN_RESOURCE_BOUNDS {
        assert_eq!(bound.value, 256);
        assert!((bound.value - 1) <= bound.value);
        assert!(bound.value <= bound.value);
        assert!((bound.value + 1) > bound.value);
    }
}

fn tool_step(start: usize, count: usize) -> Vec<ProviderEvent> {
    let mut events = Vec::with_capacity(count * 3 + 1);
    for index in start..start + count {
        events.extend(call_events(&format!("call-{index}"), "tool"));
    }
    events.push(ProviderEvent::Stop {
        reason: StopReason::ToolUse,
    });
    events
}

#[tokio::test]
async fn global_tool_call_257_is_rejected_before_execute() {
    let scripts = vec![
        tool_step(0, 85),
        tool_step(85, 85),
        tool_step(170, 85),
        tool_step(255, 1),
        tool_step(256, 1),
    ];
    let provider = ScriptedProvider::new(scripts);
    let tool = SequencingTool::new((0..256).map(|_| outcome("ok")).collect());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let error = run_turn(
        &mut runtime,
        handle,
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&["tool"]), &effects),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        crate::RuntimeError::LimitExceeded {
            context: TURN_TOOL_CALLS_MAX.name.into()
        }
    );
    assert_eq!(tool.calls.load(Ordering::SeqCst), 256);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 5);
    assert_eq!(effects.results.lock().unwrap().len(), 256);
}

#[tokio::test]
async fn actual_step_257_is_blocked_after_256_tool_steps() {
    let scripts = (0..256).map(|index| tool_step(index, 1)).collect();
    let provider = ScriptedProvider::new(scripts);
    let tool = SequencingTool::new((0..256).map(|_| outcome("ok")).collect());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let error = run_turn(
        &mut runtime,
        handle,
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&["tool"]), &effects),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        crate::RuntimeError::LimitExceeded {
            context: TURN_STEPS_MAX.name.into()
        }
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 256);
    assert_eq!(tool.calls.load(Ordering::SeqCst), 256);
}
