use super::super::test_support::*;
use super::super::*;
use crate::ws::event::{
    ApprovalRequest, ApprovalSubtype, DevicePermissionMode, DeviceStatus, LoopState, LoopStatus,
    ToolsetPreference,
};
use serde_json::json;
use std::sync::{Arc, Mutex, atomic::Ordering};
use uuid::Uuid;

struct FailSecondId(std::sync::atomic::AtomicUsize);
impl EventIdGenerator for FailSecondId {
    fn generate(&self) -> Result<Uuid, crate::error::AppServerError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Uuid::parse_str("00000000-0000-4000-8000-000000000001")
                .unwrap_or_else(|e| panic!("uuid:{e}")))
        } else {
            Err(crate::error::AppServerError::Internal)
        }
    }
}

fn approval_request() -> ApprovalRequest {
    ApprovalRequest {
        subtype: ApprovalSubtype::CanUseTool,
        tool_name: text("read_file"),
        input: bounded(json!({})),
        tool_call_id: text("tool-call"),
        permission_suggestions: Vec::new(),
        blocked_path: None,
        diffs: None,
    }
}

fn device_status() -> DeviceStatus {
    DeviceStatus {
        current_connection_id: None,
        connection_name: None,
        is_online: false,
        is_processing: false,
        current_permission_mode: DevicePermissionMode::Standard,
        current_working_directory: None,
        cwd_revision: None,
        git_context: None,
        letta_code_version: None,
        current_toolset: None,
        current_toolset_preference: ToolsetPreference::Auto,
        current_loaded_tools: Vec::new(),
        current_available_skills: Vec::new(),
        background_processes: Vec::new(),
        pending_control_requests: Vec::new(),
        experiments: Vec::new(),
        memory_directory: None,
        cwd_map: None,
        boot_working_directory: None,
        should_doctor: None,
        reflection_settings: None,
        supported_commands: Vec::new(),
    }
}

fn event(index: usize) -> RuntimeEvent {
    match index {
        0 => RuntimeEvent::ControlRequest {
            request_id: text("req"),
            request: approval_request(),
            agent_id: None,
            conversation_id: None,
        },
        1 => RuntimeEvent::UpdateDeviceStatus {
            device_status: Box::new(device_status()),
        },
        2 => RuntimeEvent::UpdateLoopStatus {
            loop_status: LoopState {
                status: LoopStatus::WaitingOnInput,
                active_run_ids: Vec::new(),
                executing_tool_call_ids: Vec::new(),
            },
        },
        3 => RuntimeEvent::UpdateQueue {
            queue: Vec::new(),
            removed: Vec::new(),
        },
        4 => RuntimeEvent::StreamDelta {
            delta: crate::ws::event::StreamDelta::Other(bounded(json!({}))),
            subagent_id: Some(text("sub")),
        },
        5 => RuntimeEvent::TurnFinished {
            turn_id: text("turn"),
            run_id: None,
            stop_reason: text("end_turn"),
            error: None,
        },
        _ => RuntimeEvent::UpdateSubagentState {
            subagents: Vec::new(),
        },
    }
}
fn assert_specific(value: &serde_json::Value, index: usize) {
    match index {
        0 => {
            assert_eq!(value["type"], "control_request");
            assert_eq!(value["request_id"], "req");
            assert_eq!(value["request"]["subtype"], "can_use_tool");
            assert_eq!(value["request"]["tool_name"], "read_file");
            assert_eq!(value["request"]["tool_call_id"], "tool-call");
            assert!(value.get("agent_id").is_none());
            assert!(value.get("conversation_id").is_none());
        }
        1 => {
            assert_eq!(value["type"], "update_device_status");
            assert_eq!(value["device_status"]["is_online"], false);
            assert_eq!(
                value["device_status"]["current_permission_mode"],
                "standard"
            );
        }
        2 => {
            assert_eq!(value["type"], "update_loop_status");
            assert_eq!(value["loop_status"]["status"], "WAITING_ON_INPUT");
            assert_eq!(value["loop_status"]["active_run_ids"], json!([]));
        }
        3 => {
            assert_eq!(value["type"], "update_queue");
            assert_eq!(value["queue"], json!([]));
            assert_eq!(value["removed"], json!([]));
        }
        4 => {
            assert_eq!(value["type"], "stream_delta");
            assert_eq!(value["delta"], json!({}));
            assert_eq!(value["subagent_id"], "sub");
        }
        5 => {
            assert_eq!(value["type"], "turn_finished");
            assert_eq!(value["turn_id"], "turn");
            assert_eq!(value["stop_reason"], "end_turn");
            assert!(value.get("run_id").is_none());
            assert!(value.get("error").is_none());
        }
        6 => {
            assert_eq!(value["type"], "update_subagent_state");
            assert_eq!(value["subagents"], json!([]));
        }
        _ => panic!("event index"),
    }
}

#[test]
fn seven_types_have_exact_fields_and_stamps() {
    for index in 0..7 {
        let (router, _, _, id) = router();
        let mut r = lock_router(&router).unwrap_or_else(|e| panic!("lock: {e}"));
        r.connections
            .subscribe(id, scope(1))
            .unwrap_or_else(|e| panic!("sub: {e}"));
        let delivery = r
            .broadcast(&scope(1), &event(index))
            .unwrap_or_else(|e| panic!("broadcast: {e}"));
        let value = serde_json::to_value(&delivery.as_slice()[0].frame)
            .unwrap_or_else(|e| panic!("json: {e}"));
        assert_specific(&value, index);
        assert_eq!(value["runtime"], serde_json::to_value(scope(1)).unwrap());
        assert_eq!(value["event_seq"], 1);
        assert!(
            value["emitted_at"]
                .as_str()
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                .is_some()
        );
        let kind = value["type"].as_str().unwrap_or_else(|| panic!("type"));
        let key = value["idempotency_key"]
            .as_str()
            .unwrap_or_else(|| panic!("key"));
        let prefix = format!("{kind}:1:");
        assert!(key.starts_with(&prefix));
        let uuid = Uuid::parse_str(&key[prefix.len()..]).unwrap_or_else(|e| panic!("uuid: {e}"));
        assert!(!uuid.is_nil());
        assert_eq!(uuid.get_version_num(), 4);
    }
}
#[test]
fn runtime_start_response_is_unstamped() {
    let value = serde_json::to_value(ConnectionResponse::RuntimeStart {
        request_id: "r".into(),
        success: false,
        runtime: None,
        agent: None,
        conversation: None,
        created: router::CreatedFlags {
            agent: false,
            conversation: false,
        },
        error: Some("x".into()),
    })
    .unwrap_or_else(|e| panic!("json: {e}"));
    for field in ["event_seq", "emitted_at", "idempotency_key"] {
        assert!(value.get(field).is_none(), "{field}");
    }
}
#[test]
fn input_accepted_is_unstamped() {
    let value = serde_json::to_value(ConnectionResponse::InputAccepted {
        request_id: "r".into(),
        runtime: scope(1),
        accepted: false,
        disposition: None,
        error: Some("x".into()),
    })
    .unwrap_or_else(|e| panic!("json: {e}"));
    assert!(value.get("disposition").is_none());
    for field in ["event_seq", "emitted_at", "idempotency_key"] {
        assert!(value.get(field).is_none(), "{field}");
    }
}
#[test]
fn management_is_unstamped() {
    let value = serde_json::to_value(ConnectionResponse::Management {
        request_id: "r".into(),
        success: true,
    })
    .unwrap_or_else(|e| panic!("json: {e}"));
    assert!(value.get("runtime").is_none());
    for field in ["event_seq", "emitted_at", "idempotency_key"] {
        assert!(value.get(field).is_none(), "{field}");
    }
}
#[test]
fn per_connection_sequences_and_replay_keys_are_unique() {
    let (router, _, _, first) = router();
    let _second = {
        let mut r = lock_router(&router).unwrap_or_else(|e| panic!("lock:{e}"));
        let second = r.connections.open().unwrap_or_else(|e| panic!("open:{e}"));
        r.connections
            .initialize(second)
            .unwrap_or_else(|e| panic!("init:{e}"));
        for id in [first, second] {
            r.connections
                .subscribe(id, scope(1))
                .unwrap_or_else(|e| panic!("sub:{e}"));
        }
        second
    };
    let mut r = lock_router(&router).unwrap_or_else(|e| panic!("lock:{e}"));
    let a = r
        .broadcast(&scope(1), &event(2))
        .unwrap_or_else(|e| panic!("a:{e}"));
    let b = r
        .broadcast(&scope(1), &event(2))
        .unwrap_or_else(|e| panic!("b:{e}"));
    assert_eq!(
        a.as_slice()
            .iter()
            .map(|d| d.frame.event_seq)
            .collect::<Vec<_>>(),
        vec![1, 1]
    );
    assert_eq!(
        b.as_slice()
            .iter()
            .map(|d| d.frame.event_seq)
            .collect::<Vec<_>>(),
        vec![2, 2]
    );
    assert_ne!(
        a.as_slice()[0].frame.idempotency_key,
        b.as_slice()[0].frame.idempotency_key
    );
}
#[test]
fn second_subscriber_starts_one_while_first_continues() {
    let (router, _, _, first) = router();
    let mut r = lock_router(&router).unwrap_or_else(|e| panic!("lock:{e}"));
    r.connections
        .subscribe(first, scope(1))
        .unwrap_or_else(|e| panic!("sub:{e}"));
    let _ = r.broadcast(&scope(1), &event(2));
    let second = r.connections.open().unwrap_or_else(|e| panic!("open:{e}"));
    r.connections
        .initialize(second)
        .unwrap_or_else(|e| panic!("init:{e}"));
    r.connections
        .subscribe(second, scope(1))
        .unwrap_or_else(|e| panic!("sub:{e}"));
    let d = r
        .broadcast(&scope(1), &event(2))
        .unwrap_or_else(|e| panic!("broadcast:{e}"));
    assert_eq!(
        d.as_slice()
            .iter()
            .map(|x| x.frame.event_seq)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );
}
#[test]
fn overflow_does_not_mutate_or_call_generators() {
    let (router, clock, ids, id) = router();
    let mut r = lock_router(&router).unwrap_or_else(|e| panic!("lock:{e}"));
    r.connections
        .subscribe(id, scope(1))
        .unwrap_or_else(|e| panic!("sub:{e}"));
    r.connections.set_event_seq(id, u64::MAX);
    let clock_calls = clock.calls.load(Ordering::SeqCst);
    assert!(r.broadcast(&scope(1), &event(2)).is_err());
    assert_eq!(clock.calls.load(Ordering::SeqCst), clock_calls);
    assert_eq!(ids.calls.load(Ordering::SeqCst), 0);
    assert_eq!(r.connections.event_seq(id), Some(u64::MAX));
    assert!(r.broadcast(&scope(1), &event(2)).is_err());
}
#[test]
fn second_generator_failure_keeps_all_target_sequences_unchanged() {
    let (base, clock, _, _) = router();
    drop(base);
    let ids = Arc::new(FailSecondId(std::sync::atomic::AtomicUsize::new(0)));
    let router = Arc::new(Mutex::new(RuntimeRouter::new(clock, ids)));
    let mut r = lock_router(&router).unwrap_or_else(|e| panic!("lock:{e}"));
    let first = r.connections.open().unwrap_or_else(|e| panic!("open:{e}"));
    let second = r.connections.open().unwrap_or_else(|e| panic!("open:{e}"));
    for id in [first, second] {
        r.connections
            .initialize(id)
            .unwrap_or_else(|e| panic!("init:{e}"));
        r.connections
            .subscribe(id, scope(1))
            .unwrap_or_else(|e| panic!("sub:{e}"));
    }
    assert!(r.broadcast(&scope(1), &event(2)).is_err());
    assert_eq!(r.connections.event_seq(first), Some(0));
    assert_eq!(r.connections.event_seq(second), Some(0));
}

#[test]
fn key_has_type_sequence_nonnil_v4_uuid() {
    let (router, _, _, id) = router();
    let mut r = lock_router(&router).unwrap_or_else(|e| panic!("lock:{e}"));
    r.connections
        .subscribe(id, scope(1))
        .unwrap_or_else(|e| panic!("sub:{e}"));
    let d = r
        .broadcast(&scope(1), &event(2))
        .unwrap_or_else(|e| panic!("broadcast:{e}"));
    let key = d.as_slice()[0].frame.idempotency_key.as_str();
    let uuid = Uuid::parse_str(key.rsplit(':').next().unwrap_or(""))
        .unwrap_or_else(|e| panic!("uuid:{e}"));
    assert!(!uuid.is_nil());
    assert_eq!(uuid.get_version(), Some(uuid::Version::Random));
    assert!(key.starts_with("update_loop_status:1:"));
}
