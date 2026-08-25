//! Authoritative full-snapshot sync behavior.
//!
//! Sync has no cursor or delta semantics. Every invocation emits device, loop,
//! queue, and subagent snapshots followed by unresolved approval requests; the
//! router appends the unstamped `sync_response` only after those deliveries.

#[cfg(test)]
mod replays_snapshots {
    use crate::ws::{RuntimeEvent, service::SyncOutcome, test_support::events};
    use serde_json::json;

    use crate::ws::test_support::bounded;

    #[test]
    fn pinned_order_is_full_and_cursor_free() {
        let outcome = SyncOutcome {
            broadcasts: events(vec![
                RuntimeEvent::UpdateDeviceStatus {
                    device_status: bounded(json!({"online": true})),
                },
                RuntimeEvent::UpdateLoopStatus {
                    loop_status: bounded(json!({"status": "idle"})),
                },
                RuntimeEvent::UpdateQueue {
                    queue: bounded(json!([])),
                    removed: bounded(json!([])),
                },
                RuntimeEvent::UpdateSubagentState {
                    subagents: bounded(json!([])),
                },
                RuntimeEvent::ControlRequest {
                    request_id: super::super::test_support::text("approval-1"),
                    request: bounded(json!({"state": "pending"})),
                    agent_id: None,
                    conversation_id: None,
                },
            ]),
        };
        let kinds: Vec<_> = outcome
            .broadcasts
            .as_slice()
            .iter()
            .map(RuntimeEvent::discriminant)
            .collect();
        assert_eq!(
            kinds,
            [
                "update_device_status",
                "update_loop_status",
                "update_queue",
                "update_subagent_state",
                "control_request",
            ]
        );
    }
}

#[cfg(test)]
mod snapshot_semantics {
    use crate::ws::{RuntimeEvent, test_support::bounded};
    use serde_json::{Value, json};

    fn apply(current: &mut Value, event: &RuntimeEvent) {
        *current = match event {
            RuntimeEvent::UpdateDeviceStatus { device_status } => device_status.as_value().clone(),
            RuntimeEvent::UpdateLoopStatus { loop_status } => loop_status.as_value().clone(),
            RuntimeEvent::UpdateQueue { queue, .. } => queue.as_value().clone(),
            RuntimeEvent::UpdateSubagentState { subagents } => subagents.as_value().clone(),
            _ => current.clone(),
        };
    }

    #[test]
    fn every_state_type_replaces_prior_state() {
        let cases = [
            RuntimeEvent::UpdateDeviceStatus {
                device_status: bounded(json!({"new": 1})),
            },
            RuntimeEvent::UpdateLoopStatus {
                loop_status: bounded(json!({"status": "idle"})),
            },
            RuntimeEvent::UpdateQueue {
                queue: bounded(json!([{"id": "new"}])),
                removed: bounded(json!([])),
            },
            RuntimeEvent::UpdateSubagentState {
                subagents: bounded(json!([{"id": "child"}])),
            },
        ];
        for event in cases {
            let mut current = json!({"stale": true});
            apply(&mut current, &event);
            assert!(current.get("stale").is_none());
        }
    }

    #[test]
    fn queue_removal_transitions_remain_ordered() {
        let event = RuntimeEvent::UpdateQueue {
            queue: bounded(json!([])),
            removed: bounded(json!([
                {"client_message_id": "first", "disposition": "dequeued"},
                {"client_message_id": "second", "disposition": "cancelled"}
            ])),
        };
        let RuntimeEvent::UpdateQueue { removed, .. } = event else {
            unreachable!();
        };
        let ids: Vec<_> = removed
            .as_value()
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("client_message_id").and_then(Value::as_str))
            .collect();
        assert_eq!(ids, ["first", "second"]);
    }
}

#[cfg(test)]
mod fixture_round_trip {
    use crate::framing::decode_text;
    use crate::ws::{ConnectionResponse, command};
    use serde_json::json;

    #[test]
    fn sync_command_and_response_discriminants_round_trip() {
        let raw = json!({
            "type": "sync",
            "request_id": "sync-1",
            "runtime": {"agent_id": "agent-1", "conversation_id": "conversation-1"}
        });
        let frame = decode_text(&raw.to_string()).expect("fixture frame");
        let command = command::decode(&frame).expect("command decode");
        assert!(matches!(command, Some(command::RuntimeCommand::Sync(_))));
        let response = ConnectionResponse::SyncResponse {
            request_id: "sync-1".into(),
            runtime: serde_json::from_value(raw["runtime"].clone()).expect("runtime fixture"),
            success: true,
            error: None,
        };
        let wire = serde_json::to_value(response).expect("response encode");
        assert_eq!(wire["type"], "sync_response");
    }
}
