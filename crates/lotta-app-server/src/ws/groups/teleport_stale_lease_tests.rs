//! `ws::teleport::stale_lease` certificate selectors.
//!
//! A teleport continuation carrying a superseded lease generation must be
//! rejected with zero state change and zero emissions.

use std::sync::{Arc, Mutex};

use lotta_domain::{
    AgentId, ConversationId, NonEmptyString, RunId, RuntimeScope, Timestamp, TurnStateKind,
};
use lotta_runtime::ListenerRuntime;

use super::{
    TeleportBridge, TeleportContinuationOutcome, TeleportContinueKind, TeleportContinuePayload,
    TeleportForwarder, TeleportMessage, TeleportSource,
};

const ENQUEUED_AT: &str = "2026-08-14T12:34:56Z";

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent-1").expect("agent"),
        ConversationId::accept("conversation-1").expect("conversation"),
        None,
    )
}

fn recorder() -> (TeleportBridge, Arc<Mutex<Vec<TeleportMessage>>>) {
    let frames: Arc<Mutex<Vec<TeleportMessage>>> = Arc::default();
    let sink = Arc::clone(&frames);
    let forward: TeleportForwarder = Arc::new(move |_, message| {
        sink.lock().expect("frames").push(message);
        Ok(())
    });
    (TeleportBridge::new(forward), frames)
}

fn payload(teleport_id: &str) -> TeleportContinuePayload {
    TeleportContinuePayload {
        kind: TeleportContinueKind::TeleportContinue,
        teleport_id: NonEmptyString::new(teleport_id).expect("teleport id"),
        source: TeleportSource {
            device_id: "device-1".to_owned(),
            connection_name: "origin".to_owned(),
        },
        continuation: None,
    }
}

#[test]
fn superseded_generation_is_rejected_with_zero_emissions() {
    let (bridge, frames) = recorder();
    let mut runtime = ListenerRuntime::new();
    let handle = runtime
        .get_or_create(&scope(), uuid::Uuid::from_u128(1))
        .expect("runtime");
    let owner = runtime.lifecycle_mut(&handle).expect("owner");
    let stale = owner
        .begin_turn(
            "turn-1".to_owned(),
            RunId::generate_sequence(1).expect("run"),
        )
        .expect("first turn");
    owner
        .finish_turn(
            &stale,
            lotta_domain::StopReason::new("completed").expect("reason"),
        )
        .expect("settle first turn");
    let current = owner
        .begin_turn(
            "turn-2".to_owned(),
            RunId::generate_sequence(2).expect("run"),
        )
        .expect("second turn");
    assert_ne!(stale, current, "a replacement turn owns a new generation");

    let outcome = bridge
        .continue_input(
            &mut runtime,
            &handle,
            stale,
            &payload("teleport-7"),
            Timestamp::parse_persisted_rfc3339(ENQUEUED_AT).expect("timestamp"),
        )
        .expect("classification");

    assert_eq!(outcome, TeleportContinuationOutcome::StaleLease);
    assert!(
        frames.lock().expect("frames").is_empty(),
        "a stale continuation emits nothing"
    );
    assert!(
        runtime
            .queue(&handle)
            .expect("queue")
            .lock()
            .expect("queue lock")
            .is_empty()
    );
    let owner = runtime.lifecycle(&handle).expect("owner");
    let projection = owner.projection();
    assert_eq!(
        projection.state(),
        TurnStateKind::Active,
        "the replacement turn is untouched"
    );
    assert!(owner.is_current(&current), "only the current lease remains");
}
