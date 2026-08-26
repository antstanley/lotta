use super::test_support::*;
use super::*;
use crate::ports::{ProviderEvent, StopReason};
use lotta_domain::{
    BoundedJsonValue, EntityExtras, NonEmptyString, QueueItem, QueueItemKind, QueueItemSource,
    RunId, Timestamp, TurnStateKind,
};

#[tokio::test]
async fn queued_message_pumps_and_runs_as_next_turn() {
    let (mut runtime, handle, occupied) = runtime();
    let item = QueueItem {
        id: NonEmptyString::new("queued-turn").unwrap(),
        client_message_id: NonEmptyString::new("queued-client").unwrap(),
        kind: QueueItemKind::Message,
        source: QueueItemSource::User,
        content: BoundedJsonValue::new(serde_json::json!({"text":"queued"})).unwrap(),
        enqueued_at: Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56Z").unwrap(),
        extras: EntityExtras::default(),
    };
    assert!(matches!(
        runtime
            .admit(
                &handle,
                crate::AdmissionRequest {
                    item: item.clone(),
                    route: crate::AdmissionRoute::Ordinary,
                },
            )
            .unwrap(),
        crate::AdmissionOutcome::Queued(_)
    ));
    runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .finish_turn(&occupied, lotta_domain::StopReason::new("done").unwrap())
        .unwrap();
    let pumped = runtime.pump_queue(&handle).unwrap().unwrap();
    let selected = match pumped.mutation().event() {
        crate::QueueMutationEvent::Pumped(items) => items[0].clone(),
        other => panic!("unexpected mutation: {other:?}"),
    };
    assert_eq!(selected.id, item.id);
    let lease = runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .begin_turn(
            selected.id.as_str().into(),
            RunId::generate_sequence(2).unwrap(),
        )
        .unwrap();
    let provider = ScriptedProvider::new(vec![vec![
        ProviderEvent::TextDelta {
            text: text("queued response"),
        },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert!(runtime.queue(&handle).unwrap().lock().unwrap().is_empty());
    assert_eq!(effects.projections.lock().unwrap().len(), 1);
    assert_eq!(effects.events.lock().unwrap().len(), 2);
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}
