use super::test_support::*;
use super::*;
use crate::bounds::TOOL_ARGUMENT_BYTES_MAX;
use crate::ports::{ProviderEvent, ProviderMessageRole, StopReason, ToolCallAccumulator};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn malformed_partial_json_is_rejected_only_at_end() {
    let call_id = id("partial");
    let script = vec![
        ProviderEvent::ToolCallStart {
            call_id: call_id.clone(),
            name: text("tool"),
        },
        ProviderEvent::ToolCallArgumentsDelta {
            call_id: call_id.clone(),
            chunk: chunk(b"{"),
        },
        ProviderEvent::ToolCallEnd { call_id },
    ];
    let (mut runtime, handle, lease) = runtime();
    let provider = ScriptedProvider::new(vec![script]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
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
        crate::RuntimeError::InvalidData {
            context: "provider tool arguments JSON".into()
        }
    );
    assert_eq!(tool.calls.load(Ordering::SeqCst), 0);
    assert!(effects.events.lock().unwrap().is_empty());
    assert!(effects.results.lock().unwrap().is_empty());

    let source = include_str!("loop.rs");
    let delta = source
        .split("ProviderEvent::ToolCallArgumentsDelta")
        .nth(1)
        .unwrap()
        .split("ProviderEvent::ToolCallEnd")
        .next()
        .unwrap();
    assert!(delta.contains("accumulator.append"));
    assert!(!delta.contains("serde"));
    assert!(!delta.contains(".end("));
    assert!(!delta.contains("ValidatedToolInput"));
    let end = source.split("ProviderEvent::ToolCallEnd").nth(1).unwrap();
    assert!(end.contains("execute_call"));
    assert!(source.contains("state.accumulator.end(&call_id)"));
}

#[tokio::test]
async fn stable_id_reaches_result_and_continuation_request() {
    let call_id = id("stable");
    let mut first = call_events("stable", "tool");
    first.push(ProviderEvent::Stop {
        reason: StopReason::ToolUse,
    });
    let provider = ScriptedProvider::new(vec![
        first,
        vec![
            ProviderEvent::TextDelta { text: text("done") },
            ProviderEvent::Stop {
                reason: StopReason::EndTurn,
            },
        ],
    ]);
    let tool = SequencingTool::new(vec![outcome("ok")]);
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    run_turn(
        &mut runtime,
        handle,
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&["tool"]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(effects.results.lock().unwrap()[0].call_id, call_id);
    let requests = provider.requests.lock().unwrap();
    let message = requests[1].messages.as_slice().last().unwrap();
    assert_eq!(message.role, ProviderMessageRole::Tool);
    assert_eq!(message.tool_call_id.as_ref(), Some(&call_id));
}

#[test]
fn accumulator_accepts_exact_bound_then_rejects_one_byte() {
    let mut accumulator = ToolCallAccumulator::default();
    let call_id = id("bound");
    accumulator.start(call_id.clone()).unwrap();
    let bytes = vec![b' '; TOOL_ARGUMENT_BYTES_MAX.value];
    accumulator.append(&call_id, bytes.as_slice()).unwrap();
    assert_eq!(bytes.len(), TOOL_ARGUMENT_BYTES_MAX.value);
    assert_eq!(
        accumulator.append(&call_id, b"x"),
        Err(crate::RuntimeError::LimitExceeded {
            context: TOOL_ARGUMENT_BYTES_MAX.name.into()
        })
    );
    assert!(include_str!("loop.rs").contains("ToolCallAccumulator"));
}
