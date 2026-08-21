//! `ws::external_tools::update` certificate selectors.

use super::support::*;
use super::{ResponseDisposition, decode};
use crate::ws::ConnectionId;
use lotta_runtime::ports::ToolOutcome;
use serde_json::{Value, json};

#[tokio::test]
async fn atomic_group() {
    let (bridge, _) = recorder();
    let connection: ConnectionId = 7;
    let good = update_command(&update_value(&[definition("stable")]));
    bridge.apply_update(connection, &good).expect("applied");
    assert_eq!(bridge.tracked_revision(&scope()), Some(1));
    let mixed = update_command(&update_value(&[
        definition("candidate"),
        json!({
            "name": "",
            "description": "bad",
            "parameters": {"type": "object"},
        }),
    ]));
    assert!(bridge.apply_update(connection, &mixed).is_err());
    let snapshot = bridge.snapshot(&scope(), None).expect("snapshot");
    assert!(snapshot.by_model("stable").is_some());
    assert!(snapshot.by_model("candidate").is_none());
    assert_eq!(bridge.tracked_revision(&scope()), Some(1));
    for malformed in [
        json!({
            "type": "runtime_external_tools_update",
            "request_id": "u2",
            "updates": [{"runtimes": [], "external_tools": []}],
        }),
        json!({
            "type": "runtime_external_tools_update",
            "request_id": "u3",
            "updates": [{"runtimes": [wire_scope(), wire_scope()], "external_tools": []}],
        }),
        json!({
            "type": "runtime_external_tools_update",
            "request_id": "u4",
            "updates": [
                {"runtimes": [wire_scope()], "external_tools":
                    [{"tools": [{"name": "bad", "description": "d", "parameters": []}]}]},
            ],
        }),
    ] {
        let frame = crate::framing::decode_text(&malformed.to_string()).expect("frame");
        assert!(
            decode(&frame).is_err(),
            "malformed must reject: {malformed}"
        );
    }
    assert_eq!(bridge.tracked_revision(&scope()), Some(1));
    let replacement = update_command(&update_value(&[definition("replacement")]));
    bridge
        .apply_update(connection, &replacement)
        .expect("applied");
    let snapshot = bridge.snapshot(&scope(), None).expect("snapshot");
    assert!(snapshot.by_model("replacement").is_some());
    assert!(snapshot.by_model("stable").is_none());
}

#[tokio::test]
async fn call_request_fields() {
    let (bridge, frames) = recorder();
    let connection: ConnectionId = 3;
    let grouped = json!({
        "type": "runtime_external_tools_update",
        "request_id": "update-1",
        "updates": [{
            "runtimes": [wire_scope()],
            "external_tools": [
                {"tools": [definition("plain")]},
                {"scope_id": "scope-a", "tools": [definition("scoped")]},
            ],
        }],
    });
    let command = update_command(&grouped);
    bridge.apply_update(connection, &command).expect("applied");
    let unscoped = bridge.snapshot(&scope(), None).expect("unscoped");
    let scoped = bridge.snapshot(&scope(), Some("scope-a")).expect("scoped");
    let plain = tokio::spawn(run_pipeline(unscoped, "plain"));
    wait_for_frames(&frames, 1).await;
    let scoped_task = tokio::spawn(run_pipeline(scoped, "scoped"));
    wait_for_frames(&frames, 2).await;
    let recorded = frames.lock().expect("frames").clone();
    for (target, frame) in &recorded {
        assert_eq!(*target, connection);
        assert_eq!(frame["type"], "external_tool_call_request");
        assert!(
            frame["request_id"]
                .as_str()
                .expect("request id")
                .starts_with("external-tool-")
        );
        assert_eq!(frame["runtime"], wire_scope());
        assert!(!frame["tool_call_id"].as_str().expect("call id").is_empty());
        assert_eq!(frame["input"], json!({"tool_call_id": "call-wire"}));
    }
    let pick = |name: &str| find_frame(&recorded, name).expect("frame per registered tool");
    let plain_frame = pick("plain");
    let scoped_frame = pick("scoped");
    assert_eq!(plain_frame["tool_name"], "plain");
    assert!(plain_frame.get("scope_id").is_none());
    assert_eq!(scoped_frame["tool_name"], "scoped");
    assert_eq!(scoped_frame["scope_id"], "scope-a");
    for frame in [plain_frame, scoped_frame] {
        let request_id = frame["request_id"].as_str().expect("request id").to_owned();
        let response = response_command(&response_value(&request_id, &json!(null)));
        let disposition = bridge.handle_response(connection, &response);
        assert_eq!(disposition, ResponseDisposition::Resolved);
    }
    let outcomes = tokio::join!(plain, scoped_task);
    assert_success(outcomes.0.expect("join"));
    assert_success(outcomes.1.expect("join"));
}

fn find_frame(recorded: &[(ConnectionId, Value)], name: &str) -> Option<Value> {
    recorded
        .iter()
        .find(|(_, frame)| frame["tool_name"] == name)
        .map(|(_, frame)| frame.clone())
}

fn assert_success(outcome: ToolOutcome) {
    assert!(matches!(outcome, ToolOutcome::Success { content }
        if content.as_str() == text_result_text("ok")));
}

/// Decode mirrors the pinned controller guard for `external_tool_call_response`.
#[test]
fn call_response_result_shape_matches_pin() {
    let accepted = [
        json!({"type": "external_tool_call_response", "request_id": "r1",
               "result": {"content": []}}),
        json!({"type": "external_tool_call_response", "request_id": "r2",
               "result": {"content": [{"type": "text", "text": "ok"}], "is_error": true}}),
        json!({"type": "external_tool_call_response", "request_id": "r3",
               "result": {"content": [{"type": "text"}]}, "error": "wins"}),
        json!({"type": "external_tool_call_response", "request_id": "r4",
               "error": "boom"}),
    ];
    for value in &accepted {
        let frame = crate::framing::decode_text(&value.to_string()).expect("frame");
        assert!(decode(&frame).is_ok(), "pin accepts: {value}");
    }
    let rejected = [
        json!({"type": "external_tool_call_response", "request_id": "r5", "result": 42}),
        json!({"type": "external_tool_call_response", "request_id": "r6",
               "result": {"content": 42}}),
        json!({"type": "external_tool_call_response", "request_id": "r7",
               "result": {"content": [42]}}),
        json!({"type": "external_tool_call_response", "request_id": "r8",
               "result": {"content": [], "is_error": "yes"}}),
        json!({"type": "external_tool_call_response", "request_id": "r9", "result": []}),
        json!({"type": "external_tool_call_response", "request_id": "r10"}),
    ];
    for value in &rejected {
        let frame = crate::framing::decode_text(&value.to_string()).expect("frame");
        assert!(decode(&frame).is_err(), "pin rejects: {value}");
    }
}
