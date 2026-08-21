//! `ws::external_tools::owner_disconnect` certificate selectors.

use std::sync::Arc;

use super::support::*;
use crate::ws::ConnectionId;
use lotta_runtime::ports::ToolOutcome;

#[tokio::test]
async fn resolves_every_pending_with_typed_owner_disconnected_outcome() {
    let (bridge, frames) = recorder();
    let connection: ConnectionId = 9;
    let pair = update_command(&update_value(&[definition("one"), definition("two")]));
    bridge.apply_update(connection, &pair).expect("applied");
    let snapshot = bridge.snapshot(&scope(), None).expect("snapshot");
    let one = tokio::spawn(run_pipeline(Arc::clone(&snapshot), "one"));
    let two = tokio::spawn(run_pipeline(snapshot, "two"));
    wait_for_frames(&frames, 2).await;
    bridge.disconnect(connection);
    for task in [one, two] {
        assert!(matches!(
            task.await.expect("join"),
            ToolOutcome::ToolDefinedError { code, .. }
                if code.as_str() == "external_owner_disconnected"
        ));
    }
    bridge.disconnect(connection);
}
