//! Full-state reconnect synchronization.
//!
//! Sync has no cursor or delta semantics. Every invocation emits device, loop,
//! queue, and subagent snapshots followed by unresolved approval requests; the
//! router appends the unstamped `sync_response` only after those deliveries.

#[cfg(test)]
mod replays_snapshots {
    use crate::ws::{RuntimeEvent, service::SyncOutcome, test_support::*};

    #[test]
    fn pinned_order_is_full_and_cursor_free() {
        let outcome = SyncOutcome {
            broadcasts: events(vec![
                RuntimeEvent::UpdateDeviceStatus {
                    device_status: Box::new(device_status()),
                },
                status_event("WAITING_ON_INPUT"),
                RuntimeEvent::UpdateQueue {
                    queue: Vec::new(),
                    removed: Vec::new(),
                },
                RuntimeEvent::UpdateSubagentState {
                    subagents: Vec::new(),
                },
                RuntimeEvent::ControlRequest {
                    request_id: text("approval-1"),
                    request: approval_request(),
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
    use crate::ws::{RuntimeEvent, event::*, test_support::*};
    use lotta_domain::{QueueItem, QueueRemovalDisposition};
    use serde_json::{Value, json};

    fn apply(current: &mut Value, event: &RuntimeEvent) {
        *current = match event {
            RuntimeEvent::UpdateDeviceStatus { device_status } => {
                serde_json::to_value(device_status)
            }
            RuntimeEvent::UpdateLoopStatus { loop_status } => serde_json::to_value(loop_status),
            RuntimeEvent::UpdateQueue { queue, .. } => serde_json::to_value(queue),
            RuntimeEvent::UpdateSubagentState { subagents } => serde_json::to_value(subagents),
            _ => Ok(current.clone()),
        }
        .expect("typed DTO encodes");
    }

    fn subagent() -> SubagentState {
        SubagentState {
            subagent_id: "child".into(),
            subagent_type: "worker".into(),
            description: "fixture".into(),
            prompt: None,
            status: SubagentStatus::Pending,
            agent_url: None,
            conversation_id: None,
            model: None,
            is_background: None,
            silent: None,
            tool_call_id: None,
            parent_agent_id: None,
            parent_conversation_id: None,
            start_time: 0,
            tool_calls: Vec::new(),
            total_tokens: 0,
            duration_ms: 0,
            error: None,
        }
    }

    #[test]
    fn every_state_type_replaces_prior_state() {
        let cases = [
            RuntimeEvent::UpdateDeviceStatus {
                device_status: Box::new(device_status()),
            },
            RuntimeEvent::UpdateLoopStatus {
                loop_status: loop_state(LoopStatus::WaitingOnInput),
            },
            RuntimeEvent::UpdateQueue {
                queue: Vec::<QueueItem>::new(),
                removed: Vec::new(),
            },
            RuntimeEvent::UpdateSubagentState {
                subagents: vec![subagent()],
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
            queue: Vec::new(),
            removed: vec![
                QueueRemovalTransition {
                    client_message_id: text("first"),
                    disposition: QueueRemovalDisposition::Dequeued,
                },
                QueueRemovalTransition {
                    client_message_id: text("second"),
                    disposition: QueueRemovalDisposition::Cancelled,
                },
            ],
        };
        let removed = match event {
            RuntimeEvent::UpdateQueue { removed, .. } => removed,
            _ => Vec::new(),
        };
        let ids: Vec<_> = removed
            .iter()
            .map(|row| row.client_message_id.as_str())
            .collect();
        assert_eq!(ids, ["first", "second"]);
    }
}

#[cfg(test)]
mod recovery_repairs {
    mod ws {
        mod recovery {
            mod repairs_missing_tool_end {
                use crate::ws::{RuntimeEvent, event::*, test_support::*};

                #[derive(Default)]
                struct ToolReducer {
                    executing: std::collections::BTreeSet<String>,
                }

                impl ToolReducer {
                    fn apply(&mut self, event: &RuntimeEvent) {
                        match event {
                            RuntimeEvent::StreamDelta { delta, .. } => self.apply_delta(delta),
                            RuntimeEvent::UpdateLoopStatus { loop_status } => {
                                self.executing = loop_status
                                    .executing_tool_call_ids
                                    .iter()
                                    .map(ToString::to_string)
                                    .collect();
                            }
                            _ => {}
                        }
                    }

                    fn apply_delta(&mut self, delta: &StreamDelta) {
                        match delta {
                            StreamDelta::ClientToolStart(start) => {
                                self.executing
                                    .insert(start.tool_call_id.as_str().to_owned());
                            }
                            StreamDelta::ClientToolEnd(end) => {
                                self.executing.remove(end.tool_call_id.as_str());
                            }
                            StreamDelta::Other(_) => {}
                        }
                    }
                }

                #[test]
                fn authoritative_loop_snapshot_repairs_missing_tool_end() {
                    let tool_call_id = text("tool-call");
                    let sequence = [
                        RuntimeEvent::StreamDelta {
                            delta: StreamDelta::ClientToolStart(ClientToolStart {
                                id: text("message"),
                                date: text("2026-08-18T00:00:00Z"),
                                message_type: ClientToolStartType::ClientToolStart,
                                run_id: None,
                                tool_call_id: tool_call_id.clone(),
                                tool_name: Some(text("Read")),
                                tool_args: None,
                            }),
                            subagent_id: None,
                        },
                        RuntimeEvent::UpdateLoopStatus {
                            loop_status: loop_state(LoopStatus::WaitingOnInput),
                        },
                    ];
                    let mut reducer = ToolReducer::default();
                    reducer.apply(&sequence[0]);
                    assert_eq!(reducer.executing, [tool_call_id.as_str().to_owned()].into());
                    reducer.apply(&sequence[1]);
                    assert!(reducer.executing.is_empty());
                }
            }
        }
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
