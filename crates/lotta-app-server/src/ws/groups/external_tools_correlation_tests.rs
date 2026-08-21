//! `ws::external_tools::correlation` certificate selectors.

use super::support::*;
use super::{ExternalToolBridge, ResponseDisposition};
use crate::ws::ConnectionId;
use lotta_runtime::ports::ToolOutcome;
use serde_json::json;

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
    assert!(matches!(
        task.await.expect("join"),
        ToolOutcome::Success { content } if content.as_str() == "ok"
    ));
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
    assert!(matches!(
        task.await.expect("join"),
        ToolOutcome::Success { content } if content.as_str() == "ok"
    ));
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
    assert!(matches!(
        task.await.expect("join"),
        ToolOutcome::Success { content } if content.as_str() == "ok"
    ));
}
