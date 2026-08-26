//! `ws::schedules::trigger_enqueues` — a triggered schedule enqueues a
//! `cron_prompt` admission through the conversation queue instead of starting
//! a turn directly, mirroring the Task 61 scheduler certificate approach.

use lotta_domain::{
    BoundedJsonValue, EntityExtras, InputDisposition, NonEmptyString, QueueItem, QueueItemKind,
    QueueItemSource, RuntimeScope, TurnStateKind,
};
use lotta_runtime::admission::{AdmissionOutcome, AdmissionRequest, AdmissionRoute};
use lotta_runtime::registry::{RuntimeHandle, RuntimeKey};
use serde_json::json;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::support::{bridge, encoded, file_with, schedule_json, t0};

fn lock_listener(
    listener: &Arc<Mutex<lotta_runtime::registry::ListenerRuntime>>,
) -> MutexGuard<'_, lotta_runtime::registry::ListenerRuntime> {
    listener.lock().unwrap_or_else(PoisonError::into_inner)
}

fn scope_of(agent_id: &str, conversation_id: &str) -> RuntimeScope {
    RuntimeScope::new(
        lotta_domain::AgentId::accept(agent_id.to_owned()).expect("agent"),
        lotta_domain::ConversationId::accept(conversation_id.to_owned()).expect("conversation"),
        None,
    )
}

fn handle_for(
    listener: &Arc<Mutex<lotta_runtime::registry::ListenerRuntime>>,
    agent_id: &str,
    conversation_id: &str,
) -> RuntimeHandle {
    let locked = lock_listener(listener);
    locked
        .lookup(&RuntimeKey::from(&scope_of(agent_id, conversation_id)))
        .expect("target runtime resident after trigger")
}

/// One-shot scheduled exactly at the fixture instant: the intended occurrence
/// (and thus the dedup identity) is fully deterministic.
fn seeded_fixture() -> super::support::TestSchedules {
    let fixture = bridge();
    fixture.seed_file(&file_with(&[schedule_json(json!({
        "id": "schedule-1",
        "recurring": false,
        "scheduled_for": "2026-01-03T00:00:00Z",
    }))]));
    fixture
}

async fn trigger(fixture: &super::support::TestSchedules) {
    fixture
        .send(&json!({
            "type": "cron_trigger",
            "request_id": "t1",
            "task_id": "schedule-1",
        }))
        .await;
}

#[tokio::test]
async fn enqueues_cron_prompt_without_starting_a_turn() {
    let fixture = seeded_fixture();
    let before = fixture.messages().len();
    trigger(&fixture).await;

    let responses = fixture.messages();
    assert_eq!(responses.len(), before + 1);
    let value = encoded(responses.last().expect("trigger response"));
    assert_eq!(value["success"], true);
    assert_eq!(value["found"], true);
    assert_eq!(value["task"]["fire_count"], 1);

    // The fire landed as one queued cron_prompt on the target conversation,
    // and the lifecycle owner was never invoked directly.
    let listener = fixture.bridge.listener();
    let handle = handle_for(listener, "agent-local-fixture", "conversation");
    let locked = lock_listener(listener);
    let stored: Vec<(QueueItemKind, QueueItemSource)> = locked
        .queue(&handle)
        .expect("queue")
        .lock()
        .expect("queue lock")
        .items()
        .map(|item| (item.kind, item.source))
        .collect();
    assert_eq!(
        stored,
        [(QueueItemKind::CronPrompt, QueueItemSource::Cron)],
        "trigger must enqueue through the conversation queue"
    );
    assert_eq!(
        locked
            .lifecycle(&handle)
            .expect("owner")
            .projection()
            .state(),
        TurnStateKind::Idle,
        "lifecycle owner must not be invoked directly"
    );
}

#[tokio::test]
async fn admission_chain_records_identity_and_fired_history() {
    let fixture = seeded_fixture();
    trigger(&fixture).await;

    // The admission chain recorded the fire: replaying the identical
    // client_message_id resolves to the prior queued disposition.
    let listener = fixture.bridge.listener();
    let handle = handle_for(listener, "agent-local-fixture", "conversation");
    let replay = AdmissionRequest {
        item: QueueItem {
            id: NonEmptyString::new("replay".to_owned()).expect("id"),
            client_message_id: NonEmptyString::new(format!(
                "cron-schedule-1-{}",
                t0().as_utc().timestamp_millis()
            ))
            .expect("identity"),
            kind: QueueItemKind::CronPrompt,
            source: QueueItemSource::Cron,
            content: BoundedJsonValue::new(json!({})).expect("content"),
            enqueued_at: t0(),
            extras: EntityExtras::default(),
        },
        route: AdmissionRoute::Ordinary,
    };
    let outcome = lock_listener(listener)
        .admit(&handle, replay)
        .expect("admit");
    assert!(matches!(
        outcome,
        AdmissionOutcome::Duplicate(InputDisposition::Queued)
    ));

    // The fired run was persisted with its queue-item reference.
    let entries = lotta_store::schedule::RunLogStore::new(&fixture.paths)
        .read("schedule-1")
        .expect("run log");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].outcome,
        Some(lotta_domain::ScheduleRunOutcome::Queued),
        "trigger records one queued run"
    );
    assert_eq!(entries[0].reason.as_deref(), Some("one_off_due"));
    assert!(
        entries[0].queue_item_id.is_some(),
        "fired entry references its queue item"
    );
}
