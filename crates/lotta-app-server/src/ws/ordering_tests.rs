use std::sync::{Arc, Mutex};

use lotta_domain::{BoundedJsonValue, BoundedVec, RunId, StopReason};
use lotta_runtime::{
    CancellationPolicy, LeaseEffect, LeaseGuard, ListenerRuntime, SuppressionReason,
};
use lotta_testkit::fixtures::traces::{
    FrameDirection, OrderingInvariant, Provenance, ReferenceTrace, TraceDriverProof, TraceFrame,
    assert_invariant,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::*;
use crate::ws::test_support::{
    RecordingService, bounded, decode_wire, router, scope, status_event,
};

fn frame(index: usize, ordinal: u64, actual: &impl Serialize) -> TraceFrame {
    TraceFrame {
        frame_index: index,
        direction: FrameDirection::ServerToClient,
        connection_ordinal: u32::try_from(ordinal)
            .unwrap_or_else(|error| panic!("ordinal: {error}")),
        caused_by: None,
        lease_id: None,
        lease_current: None,
        broadcast_emission: None,
        wire: BoundedJsonValue::new(
            serde_json::to_value(actual).unwrap_or_else(|error| panic!("wire: {error}")),
        )
        .unwrap_or_else(|error| panic!("bounded wire: {error}")),
    }
}

fn trace(frames: Vec<TraceFrame>) -> ReferenceTrace {
    let types = frames
        .iter()
        .filter_map(|frame| frame.wire.as_value().get("type").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    ReferenceTrace {
        schema_version: 1,
        name: "actual-router-observations".into(),
        kind: "vertical-slice".into(),
        provenance: Provenance {
            path: "crates/lotta-app-server/src/ws/ordering_tests.rs".into(),
            symbol: "actual observations".into(),
            capture_boundary: "owner-local router".into(),
        },
        supporting_provenance: BoundedVec::new(Vec::new()).unwrap(),
        driver_proof: TraceDriverProof {
            command_types: BoundedVec::new(Vec::new()).unwrap(),
            message_types: BoundedVec::new(types).unwrap(),
        },
        frames: BoundedVec::new(frames).unwrap(),
    }
}

fn subscribe(router: &Arc<Mutex<RuntimeRouter>>, id: ConnectionId) {
    lock_router(router)
        .unwrap()
        .connections
        .subscribe(id, scope(1))
        .unwrap();
}

fn recorded_frames(recorded: &Arc<Mutex<Vec<EventDeliveryBatch>>>) -> Vec<TraceFrame> {
    recorded
        .lock()
        .unwrap()
        .iter()
        .flat_map(lotta_domain::BoundedVec::as_slice)
        .enumerate()
        .map(|(index, delivery)| frame(index, delivery.ordinal, &delivery.frame))
        .collect()
}

fn recorder(
    router: Arc<Mutex<RuntimeRouter>>,
) -> (Arc<RouterEventSink>, Arc<Mutex<Vec<EventDeliveryBatch>>>) {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let output = recorded.clone();
    let sink = RouterEventSink::new(
        router,
        Arc::new(move |_, _, batch| {
            output.lock().unwrap().push(batch);
            Ok(())
        }),
    );
    (Arc::new(sink), recorded)
}

#[test]
fn increasing_event_seq_per_connection() {
    let (router, _, _, id) = router();
    subscribe(&router, id);
    let mut frames = Vec::new();
    for event in [
        status_event("RUNNING"),
        RuntimeEvent::TurnFinished {
            turn_id: crate::ws::test_support::text("turn-1"),
            run_id: Some(RunId::generate_sequence(1).unwrap()),
            stop_reason: crate::ws::test_support::text("done"),
            error: None,
        },
    ] {
        let deliveries = lock_router(&router)
            .unwrap()
            .broadcast(&scope(1), &event)
            .unwrap();
        let offset = frames.len();
        frames.extend(
            deliveries
                .as_slice()
                .iter()
                .enumerate()
                .map(|(index, item)| frame(offset + index, item.ordinal, &item.frame)),
        );
    }
    let seqs: Vec<_> = frames
        .iter()
        .map(|item| item.wire.as_value()["event_seq"].as_u64().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]));
    assert_invariant(
        &trace(frames),
        OrderingInvariant::IncreasingEventSeqPerConnection,
    )
    .unwrap();
}

#[tokio::test]
async fn input_accepted_before_caused_events() {
    let (router, _, _, id) = router();
    subscribe(&router, id);
    let service = Arc::new(RecordingService::default());
    let command = decode_wire(
        &json!({"type":"input","request_id":"r","runtime":scope(1),"payload":{"kind":"create_message","messages":[]}}),
    );
    let (output, deferred) = route_command(router.clone(), service.clone(), id, command)
        .await
        .unwrap();
    let mut frames = vec![frame(0, 1, &output.responses.as_slice()[0])];
    let offset = frames.len();
    frames.extend(
        output.event_batches.as_slice()[0]
            .deliveries
            .as_slice()
            .iter()
            .enumerate()
            .map(|(index, item)| frame(offset + index, item.ordinal, &item.frame)),
    );
    let (sink, recorded) = recorder(router);
    let deferred = deferred.unwrap();
    service
        .continue_input(deferred.scope, deferred.continuation, sink)
        .await
        .unwrap();
    let offset = frames.len();
    frames.extend(recorded_frames(&recorded).into_iter().map(|mut item| {
        item.frame_index += offset;
        item
    }));
    for item in &mut frames {
        item.caused_by = Some("r".into());
    }
    let types: Vec<_> = frames
        .iter()
        .map(|item| item.wire.as_value()["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["input_accepted", "update_queue", "stream_delta"]);
    assert_invariant(
        &trace(frames),
        OrderingInvariant::InputAcceptedBeforeCausedEvents,
    )
    .unwrap();
}

#[test]
fn client_tool_start_before_same_id_end() {
    let (router, _, _, id) = router();
    subscribe(&router, id);
    let (sink, recorded) = recorder(router);
    for message_type in ["client_tool_start", "client_tool_end"] {
        sink.emit(
            &scope(1),
            RuntimeEvent::StreamDelta {
                delta: crate::ws::event::StreamDelta::Other(bounded(
                    json!({"message_type":message_type,"tool_call_id":"tool-1"}),
                )),
                subagent_id: None,
            },
        )
        .unwrap();
    }
    let mut frames = recorded_frames(&recorded);
    for (index, item) in frames.iter_mut().enumerate() {
        item.broadcast_emission = Some(format!("tool-{index}"));
    }
    assert_eq!(
        frames[0].wire.as_value()["delta"]["tool_call_id"],
        frames[1].wire.as_value()["delta"]["tool_call_id"]
    );
    assert_invariant(
        &trace(frames),
        OrderingInvariant::ToolStartBeforeMatchingToolEnd,
    )
    .unwrap();
}

#[test]
fn exactly_one_finished_after_final_delta() {
    let (router, _, _, id) = router();
    subscribe(&router, id);
    let (sink, recorded) = recorder(router);
    sink.emit(
        &scope(1),
        RuntimeEvent::StreamDelta {
            delta: crate::ws::event::StreamDelta::Other(bounded(
                json!({"message_type":"status","message":"done"}),
            )),
            subagent_id: None,
        },
    )
    .unwrap();
    sink.emit(
        &scope(1),
        RuntimeEvent::TurnFinished {
            turn_id: crate::ws::test_support::text("turn-1"),
            run_id: Some(RunId::generate_sequence(1).unwrap()),
            stop_reason: crate::ws::test_support::text("done"),
            error: None,
        },
    )
    .unwrap();
    let mut frames = recorded_frames(&recorded);
    for (index, item) in frames.iter_mut().enumerate() {
        item.caused_by = Some("turn-1".into());
        item.broadcast_emission = Some(format!("turn-{index}"));
        item.frame_index += 1;
    }
    let mut start = frame(0, 1, &json!({"type":"turn_started"}));
    start.caused_by = Some("turn-1".into());
    frames.insert(0, start);
    let finished: Vec<_> = frames
        .iter()
        .filter(|item| item.wire.as_value()["type"] == "turn_finished")
        .collect();
    let last_delta = frames
        .iter()
        .rposition(|item| item.wire.as_value()["type"] == "stream_delta")
        .unwrap();
    assert_eq!(finished.len(), 1);
    assert!(finished[0].frame_index > last_delta);
    assert_invariant(
        &trace(frames),
        OrderingInvariant::TurnFinishedExactlyOnceAfterFinalStreamDelta,
    )
    .unwrap();
}

#[test]
fn stale_lease_replacement_suppresses_event_sink() {
    let (router, _, _, id) = router();
    subscribe(&router, id);
    let (sink, recorded) = recorder(router);
    let mut runtime = ListenerRuntime::new();
    let handle = runtime
        .get_or_create(&scope(1), Uuid::from_u128(20))
        .unwrap();
    let old = runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .begin_turn("turn-1".into(), RunId::generate_sequence(1).unwrap())
        .unwrap();
    let guard = LeaseGuard::new(
        handle.clone(),
        old.clone(),
        CancellationToken::new(),
        CancellationPolicy::SuppressWhenCancelled,
    );
    let finishing = LeaseGuard::new(
        handle.clone(),
        old.clone(),
        CancellationToken::new(),
        CancellationPolicy::SuppressWhenCancelled,
    );
    assert_eq!(
        finishing.finish_turn_after_await(&mut runtime, StopReason::new("done").unwrap()),
        LeaseEffect::Applied(())
    );
    let current = runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .begin_turn("turn-2".into(), RunId::generate_sequence(2).unwrap())
        .unwrap();
    let effect = guard.apply_after_await(&runtime, || sink.emit(&scope(1), status_event("STALE")));
    assert!(matches!(
        effect,
        LeaseEffect::Suppressed(SuppressionReason::StaleLease)
    ));
    assert!(current.generation() > old.generation());
    assert!(recorded.lock().unwrap().is_empty());
    let mut replaced = frame(
        0,
        1,
        &json!({"type":"lease_replaced","old_lease_id":old.generation().to_string()}),
    );
    replaced.direction = FrameDirection::Lifecycle;
    replaced.lease_id = Some(old.generation().to_string());
    replaced.lease_current = Some(false);
    assert_invariant(
        &trace(vec![replaced]),
        OrderingInvariant::NoServerEventFromStaleLeaseAfterReplacement,
    )
    .unwrap();
}

#[test]
fn fanout_ordinals_ascending() {
    let (router, _, _, first) = router();
    let second = {
        let mut guard = lock_router(&router).unwrap();
        let id = guard.connections.open().unwrap();
        guard.connections.initialize(id).unwrap();
        id
    };
    subscribe(&router, second);
    subscribe(&router, first);
    let deliveries = lock_router(&router)
        .unwrap()
        .broadcast(&scope(1), &status_event("FANOUT"))
        .unwrap();
    let ordinals: Vec<_> = deliveries
        .as_slice()
        .iter()
        .map(|item| item.ordinal)
        .collect();
    assert_eq!(ordinals, [1, 2]);
    let mut frames: Vec<_> = deliveries
        .as_slice()
        .iter()
        .enumerate()
        .map(|(index, item)| frame(index, item.ordinal, &item.frame))
        .collect();
    for item in &mut frames {
        item.broadcast_emission = Some("fanout-1".into());
    }
    assert_invariant(
        &trace(frames),
        OrderingInvariant::BroadcastDeliveryStableAscendingConnectionOrdinal,
    )
    .unwrap();
}
