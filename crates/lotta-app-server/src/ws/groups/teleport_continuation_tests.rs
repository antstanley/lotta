//! `ws::teleport::continuation` certificate selectors.
//!
//! Proves `input.kind = teleport_continue` takes the continuation branch of
//! the admission chain: the queue length is unchanged, the active lease (and
//! therefore the turn identity) is unchanged, and no new admission starts.

use std::sync::{Arc, Mutex};

use lotta_domain::{
    AgentId, ConversationId, NonEmptyString, RunId, RuntimeScope, Timestamp, TurnStateKind,
};
use lotta_runtime::{LifecycleOwner, ListenerRuntime, RuntimeHandle};
use serde_json::json;

use super::{
    TeleportBridge, TeleportContinuation, TeleportContinueKind, TeleportContinuePayload,
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
        continuation: Some(TeleportContinuation {
            approvals: vec![json!({"behavior": "allow"})],
        }),
    }
}

struct ActiveRuntime {
    runtime: ListenerRuntime,
    handle: RuntimeHandle,
    lease: lotta_domain::TurnLease,
}

fn active_runtime() -> ActiveRuntime {
    let mut runtime = ListenerRuntime::new();
    let handle = runtime
        .get_or_create(&scope(), uuid::Uuid::from_u128(1))
        .expect("runtime");
    let owner: &mut LifecycleOwner = runtime.lifecycle_mut(&handle).expect("owner");
    let lease = owner
        .begin_turn(
            "turn-1".to_owned(),
            RunId::generate_sequence(1).expect("run"),
        )
        .expect("turn");
    ActiveRuntime {
        runtime,
        handle,
        lease,
    }
}

#[test]
fn continues_on_active_lease_without_queue_or_new_turn() {
    let (bridge, frames) = recorder();
    let mut active = active_runtime();
    let before_state = active
        .runtime
        .lifecycle(&active.handle)
        .expect("owner")
        .projection()
        .state();
    assert_eq!(before_state, TurnStateKind::Active);

    let outcome = bridge
        .continue_input(
            &mut active.runtime,
            &active.handle,
            active.lease.clone(),
            &payload("teleport-7"),
            Timestamp::parse_persisted_rfc3339(ENQUEUED_AT).expect("timestamp"),
        )
        .expect("continuation");

    assert_eq!(
        outcome,
        super::TeleportContinuationOutcome::Continued,
        "continuation branch must admit directly on the lease"
    );
    let queue = active.runtime.queue(&active.handle).expect("queue");
    assert_eq!(
        queue.lock().expect("queue lock").len(),
        0,
        "no queue item may be created"
    );
    let owner = active.runtime.lifecycle(&active.handle).expect("owner");
    assert_eq!(
        owner.projection().state(),
        TurnStateKind::Active,
        "the same turn keeps ownership"
    );
    assert!(
        owner.is_current(&active.lease),
        "lease generation is unchanged: same turn ID"
    );
    assert!(
        frames.lock().expect("frames").is_empty(),
        "continuation admission itself emits nothing"
    );
}
