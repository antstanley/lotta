use super::test_support::*;
use super::*;
use crate::ports::{ProviderEvent, StopReason};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn two_calls_execute_sequentially_in_end_order() {
    let mut first = call_events("a", "one");
    first.extend(call_events("b", "two"));
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
    let tool = SequencingTool::new(vec![outcome("one"), outcome("two")]);
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    run_turn(
        &mut runtime,
        handle,
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&["one", "two"]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(*tool.log.lock().unwrap(), ["Eone", "Xone", "Etwo", "Xtwo"]);
    assert_eq!(tool.max_in_flight.load(Ordering::SeqCst), 1);
    let results = effects.results.lock().unwrap();
    assert_eq!(results[0].call_id, id("a"));
    assert_eq!(results[1].call_id, id("b"));
}
