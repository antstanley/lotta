//! Snapshot emission: write, delete, and enable each notify after success.

use super::support::{bridge, discriminant_of, encoded};
use serde_json::json;

#[tokio::test]
async fn write_emits_snapshot_before_its_response() {
    let memory = bridge();
    memory.enable().await;
    let before = memory.messages().len();
    memory
        .send(&json!({
            "type": "write_memory_file",
            "request_id": "snap-write",
            "agent_id": memory.agent_id,
            "path": "notes/s.md",
            "content": "fresh",
        }))
        .await;
    let tail: Vec<_> = memory.messages()[before..].to_vec();
    assert_eq!(tail.len(), 2, "expected snapshot plus response");
    assert_eq!(discriminant_of(&tail[0]), "memory_updated");
    let updated = encoded(&tail[0]);
    assert_eq!(updated["affected_paths"], serde_json::json!(["notes/s.md"]));
    assert!(
        updated["timestamp"].as_i64().expect("epoch millis") > 0,
        "snapshot timestamp must be set"
    );
    assert_eq!(discriminant_of(&tail[1]), "write_memory_file_response");
    assert_eq!(encoded(&tail[1])["committed"], true);
}

#[tokio::test]
async fn delete_emits_snapshot_only_when_committing() {
    let memory = bridge();
    memory.enable().await;
    memory.commit_write("notes/d.md", "doomed").await;
    let before = memory.messages().len();
    memory
        .send(&json!({
            "type": "delete_memory_file",
            "request_id": "snap-delete",
            "agent_id": memory.agent_id,
            "path": "notes/d.md",
        }))
        .await;
    let tail: Vec<_> = memory.messages()[before..].to_vec();
    assert_eq!(tail.len(), 2, "expected snapshot plus response");
    assert_eq!(discriminant_of(&tail[0]), "memory_updated");
    assert_eq!(
        encoded(&tail[0])["affected_paths"],
        serde_json::json!(["notes/d.md"])
    );
    assert_eq!(discriminant_of(&tail[1]), "delete_memory_file_response");
    // The idempotent absent-file delete commits nothing and stays silent.
    let quiet_before = memory.messages().len();
    memory
        .send(&json!({
            "type": "delete_memory_file",
            "request_id": "snap-delete-missing",
            "agent_id": memory.agent_id,
            "path": "notes/d.md",
        }))
        .await;
    let quiet: Vec<_> = memory.messages()[quiet_before..].to_vec();
    assert_eq!(quiet.len(), 1, "absent-file delete must not snapshot");
    assert_eq!(discriminant_of(&quiet[0]), "delete_memory_file_response");
    assert_eq!(encoded(&quiet[0])["committed"], false);
}

#[tokio::test]
async fn enable_emits_snapshot_with_star_scope() {
    let memory = bridge();
    memory
        .send(&json!({
            "type": "enable_memfs",
            "request_id": "snap-enable",
            "agent_id": memory.agent_id,
        }))
        .await;
    let responses = memory.messages();
    assert_eq!(responses.len(), 2);
    // The pinned enable answers first, then refreshes the whole listing.
    assert_eq!(discriminant_of(&responses[0]), "enable_memfs_response");
    assert_eq!(encoded(&responses[0])["success"], true);
    assert_eq!(discriminant_of(&responses[1]), "memory_updated");
    assert_eq!(
        encoded(&responses[1])["affected_paths"],
        serde_json::json!(["*"])
    );
}
