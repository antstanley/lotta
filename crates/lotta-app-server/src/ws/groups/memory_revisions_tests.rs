//! Revision-graph reads: read-at-ref, history listing, unknown revisions.

use super::support::{bridge, encoded};
use serde_json::json;

#[tokio::test]
async fn reads_at_ref() {
    let memory = bridge();
    memory.enable().await;
    let first = memory.commit_write("j.md", "v1").await;
    let latest = memory.commit_write("j.md", "v2").await;
    for (reference, expected) in [(first.clone(), "v1"), (latest, "v2")] {
        memory
            .send(&json!({
                "type": "memory_file_at_ref",
                "request_id": "reads-at-ref",
                "agent_id": memory.agent_id,
                "file_path": "j.md",
                "ref": reference,
            }))
            .await;
        let value = encoded(memory.messages().last().expect("at-ref frame"));
        assert_eq!(value["success"], true);
        assert_eq!(value["content"], expected);
    }
    // A short revision is rejected before any Git resolution happens.
    memory
        .send(&json!({
            "type": "memory_file_at_ref",
            "request_id": "reads-at-ref-short",
            "agent_id": memory.agent_id,
            "file_path": "j.md",
            "ref": first[..7].to_owned(),
        }))
        .await;
    let shortened = encoded(memory.messages().last().expect("short ref frame"));
    assert_eq!(shortened["success"], false);
    assert!(shortened["content"].is_null());
}

#[tokio::test]
async fn history_lists_revisions() {
    let memory = bridge();
    memory.enable().await;
    memory.commit_write("n.md", "one").await;
    memory.commit_write("n.md", "two").await;
    memory.commit_write("n.md", "three").await;
    memory
        .send(&json!({
            "type": "memory_history",
            "request_id": "history-lists",
            "agent_id": memory.agent_id,
        }))
        .await;
    let value = encoded(memory.messages().last().expect("history frame"));
    assert_eq!(value["success"], true);
    let commits = value["commits"].as_array().expect("commits array");
    // initialize plus three writes.
    assert_eq!(commits.len(), 4);
    let head = memory.git(&["rev-parse", "HEAD"]);
    let parent = memory.git(&["rev-parse", "HEAD~1"]);
    assert_eq!(commits[0]["sha"], head.as_str());
    assert_eq!(commits[1]["sha"], parent.as_str());
    let shas: Vec<String> = commits
        .iter()
        .map(|row| row["sha"].as_str().expect("sha string").to_owned())
        .collect();
    assert!(
        shas.iter().all(|sha| sha.len() == 40),
        "every revision is a full sha"
    );
    let mut unique = shas.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), shas.len(), "revisions repeat");
}

#[tokio::test]
async fn unknown_ref_is_error() {
    let memory = bridge();
    memory.enable().await;
    memory.commit_write("u.md", "present").await;
    // Well-formed but nonexistent revision resolves through the graph into a
    // typed failure rather than a panic or silent success.
    memory
        .send(&json!({
            "type": "memory_file_at_ref",
            "request_id": "unknown-ref",
            "agent_id": memory.agent_id,
            "file_path": "u.md",
            "ref": "0".repeat(40),
        }))
        .await;
    let at_ref = encoded(memory.messages().last().expect("unknown ref frame"));
    assert_eq!(at_ref["success"], false);
    assert!(at_ref["content"].is_null());
    // Same class for commit diffs against an unknown revision.
    memory
        .send(&json!({
            "type": "memory_commit_diff",
            "request_id": "unknown-sha-diff",
            "agent_id": memory.agent_id,
            "sha": "f".repeat(40),
        }))
        .await;
    let diff = encoded(memory.messages().last().expect("unknown diff frame"));
    assert_eq!(diff["success"], false);
    assert!(diff["diff"].is_null());
}
