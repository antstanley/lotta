//! `ws::external_tools::correlation` certificate selectors.

use super::support::*;
use super::{ExternalToolBridge, ResponseDisposition};
use crate::ws::ConnectionId;
use lotta_runtime::ports::ToolOutcome;
use serde_json::json;
use tokio::time::{Duration, timeout};

const OWNER_CONNECTION: ConnectionId = 5;
const OTHER_CONNECTION: ConnectionId = 42;

async fn forwarded_call() -> (
    ExternalToolBridge,
    RecordedFrames,
    tokio::task::JoinHandle<ToolOutcome>,
    String,
) {
    let (bridge, frames) = recorder();
    let owned = update_command(&update_value(&[definition("owned")]));
    bridge
        .apply_update(OWNER_CONNECTION, &owned)
        .expect("applied");
    let snapshot = bridge.snapshot(&scope(), None).expect("snapshot");
    let task = tokio::spawn(run_pipeline(snapshot, "owned"));
    wait_for_frames(&frames, 1).await;
    let frame = frames.lock().expect("frames")[0].1.clone();
    let request_id = frame["request_id"].as_str().expect("request id").to_owned();
    (bridge, frames, task, request_id)
}

#[tokio::test]
async fn wrong_connection_rejected_and_pending() {
    let (bridge, _, task, request_id) = forwarded_call().await;
    let other = response_command(&response_value(&request_id, &json!(null)));
    let disposition = bridge.handle_response(OTHER_CONNECTION, &other);
    assert_eq!(disposition, ResponseDisposition::UnknownIgnored);
    assert!(!task.is_finished(), "rejected response leaves call pending");
    let correct = response_command(&response_value(&request_id, &json!(null)));
    assert_eq!(
        bridge.handle_response(OWNER_CONNECTION, &correct),
        ResponseDisposition::Resolved
    );
    assert!(
        matches!(task.await.expect("join"), ToolOutcome::Success { content }
            if content.as_str() == text_result_text("ok"))
    );
}

#[tokio::test]
async fn wrong_request_id_rejected_and_pending() {
    let (bridge, _, task, request_id) = forwarded_call().await;
    let unknown = response_command(&response_value("external-tool-unknown", &json!(null)));
    let disposition = bridge.handle_response(OWNER_CONNECTION, &unknown);
    assert_eq!(disposition, ResponseDisposition::UnknownIgnored);
    assert!(!task.is_finished(), "rejected response leaves call pending");
    let correct = response_command(&response_value(&request_id, &json!(null)));
    assert_eq!(
        bridge.handle_response(OWNER_CONNECTION, &correct),
        ResponseDisposition::Resolved
    );
    assert!(
        matches!(task.await.expect("join"), ToolOutcome::Success { content }
            if content.as_str() == text_result_text("ok"))
    );
}

#[tokio::test]
async fn wrong_tool_call_id_rejected_and_pending() {
    let (bridge, _, task, request_id) = forwarded_call().await;
    let echoed = response_command(&response_value(
        &request_id,
        &json!({"tool_call_id": "other-call"}),
    ));
    let disposition = bridge.handle_response(OWNER_CONNECTION, &echoed);
    assert_eq!(disposition, ResponseDisposition::InvalidResponse);
    assert!(!task.is_finished(), "rejected response leaves call pending");
    let correct = response_command(&response_value(&request_id, &json!(null)));
    assert_eq!(
        bridge.handle_response(OWNER_CONNECTION, &correct),
        ResponseDisposition::Resolved
    );
    assert!(
        matches!(task.await.expect("join"), ToolOutcome::Success { content }
            if content.as_str() == text_result_text("ok"))
    );
}

#[tokio::test]
async fn error_wins_when_both_members_present() {
    let (bridge, _, task, request_id) = forwarded_call().await;
    let both = response_command(&response_value(
        &request_id,
        &json!({"result": text_result("ignored"), "error": "controller failed"}),
    ));
    assert_eq!(
        bridge.handle_response(OWNER_CONNECTION, &both),
        ResponseDisposition::Resolved
    );
    assert!(matches!(
        task.await.expect("join"),
        ToolOutcome::ToolDefinedError { message, .. } if message.as_str() == "controller failed"
    ));
}

/// One connection drives two scopes whose managers minted the same per-manager
/// sequence; each response must settle exactly its own scope's pending call.
#[tokio::test]
async fn cross_scope_sequence_collision_resolves_own_call() {
    let (bridge, frames) = recorder();
    let owned_a = update_command(&update_value(&[definition("owned-a")]));
    bridge
        .apply_update(OWNER_CONNECTION, &owned_a)
        .expect("applied");
    let owned_b = update_command(&update_value_for(
        &other_wire_scope(),
        &[definition("owned-b")],
    ));
    bridge
        .apply_update(OWNER_CONNECTION, &owned_b)
        .expect("applied");
    let snapshot_a = bridge.snapshot(&scope(), None).expect("snapshot a");
    let snapshot_b = bridge.snapshot(&other_scope(), None).expect("snapshot b");
    let task_a = tokio::spawn(run_pipeline(snapshot_a, "owned-a"));
    let task_b = tokio::spawn(run_pipeline(snapshot_b, "owned-b"));
    wait_for_frames(&frames, 2).await;
    let recorded = frames.lock().expect("frames").clone();
    let request_id = |name: &str| {
        recorded
            .iter()
            .find(|(_, frame)| frame["tool_name"] == name)
            .map(|(_, frame)| frame["request_id"].as_str().expect("request id").to_owned())
            .expect("frame per scope")
    };
    let (id_a, id_b) = (request_id("owned-a"), request_id("owned-b"));
    assert_ne!(
        id_a, id_b,
        "colliding sequences must not alias across scopes"
    );
    let alpha = response_command(&response_value(
        &id_a,
        &json!({"result": text_result("alpha")}),
    ));
    assert_eq!(
        bridge.handle_response(OWNER_CONNECTION, &alpha),
        ResponseDisposition::Resolved
    );
    assert!(!task_b.is_finished(), "scope B stays pending");
    let outcome_a = timeout(Duration::from_secs(5), task_a)
        .await
        .expect("alpha settles")
        .expect("join");
    assert!(matches!(outcome_a, ToolOutcome::Success { content }
        if content.as_str() == text_result_text("alpha")));
    let beta = response_command(&response_value(
        &id_b,
        &json!({"result": text_result("beta")}),
    ));
    assert_eq!(
        bridge.handle_response(OWNER_CONNECTION, &beta),
        ResponseDisposition::Resolved
    );
    let outcome_b = timeout(Duration::from_secs(5), task_b)
        .await
        .expect("beta settles")
        .expect("join");
    assert!(matches!(outcome_b, ToolOutcome::Success { content }
        if content.as_str() == text_result_text("beta")));
}
