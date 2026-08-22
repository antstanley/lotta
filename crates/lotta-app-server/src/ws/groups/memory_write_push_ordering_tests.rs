//! Push-after-commit ordering, mirroring the pinned
//! memory-write-push-ordering contract (LET-9481): the bounded push lands
//! strictly between the commit and the notification/response emission, and a
//! failed push never undoes the committed write.

use super::support::{TestMemory, bridge, discriminant_of, encoded};
use serde_json::json;

/// Asserts the trailing snapshot precedes the mutation response, carries the
/// given scope, echoes `expected_sha`, and that the push had already landed on
/// the remote when both frames were emitted.
fn assert_push_preceded_notification(
    messages: &[super::MemoryMessage],
    updated_paths: &[&str],
    expected_sha: &str,
    response_tag: &'static str,
    remote_head: &str,
) {
    let updated_index = messages
        .iter()
        .rposition(|message| discriminant_of(message) == "memory_updated")
        .expect("snapshot emitted");
    let response_index = messages
        .iter()
        .rposition(|message| discriminant_of(message) == response_tag)
        .expect("response emitted");
    assert_eq!(
        encoded(&messages[updated_index])["affected_paths"],
        serde_json::json!(updated_paths)
    );
    assert_eq!(
        encoded(&messages[response_index])["commit_sha"],
        serde_json::json!(expected_sha)
    );
    // Ordering proof: apply() forwards only after transact returns — commit,
    // then push. Had the push run after notification instead, this read could
    // not yet observe the response's own revision on the remote.
    assert_eq!(remote_head, expected_sha, "push had not landed by notify");
}

#[tokio::test]
async fn write_awaits_post_commit_push_before_notifying() {
    let memory = bridge();
    memory.enable().await;
    let bare = memory.wire_bare_remote();
    memory
        .send(&json!({
            "type": "write_memory_file",
            "request_id": "push-write",
            "agent_id": memory.agent_id,
            "path": "notes/p.md",
            "content": "payload",
        }))
        .await;
    let messages = memory.messages();
    let sha = encoded(messages.last().expect("write frame"))["commit_sha"]
        .as_str()
        .expect("commit sha")
        .to_owned();
    let remote_head = TestMemory::remote_head(&bare);
    assert_push_preceded_notification(
        &messages,
        &["notes/p.md"],
        &sha,
        "write_memory_file_response",
        &remote_head,
    );
}

#[tokio::test]
async fn delete_awaits_post_commit_push_before_notifying() {
    let memory = bridge();
    memory.enable().await;
    memory.commit_write("notes/gone-remote.md", "doomed").await;
    let bare = memory.wire_bare_remote();
    memory
        .send(&json!({
            "type": "delete_memory_file",
            "request_id": "push-delete",
            "agent_id": memory.agent_id,
            "path": "notes/gone-remote.md",
        }))
        .await;
    let messages = memory.messages();
    let sha = encoded(messages.last().expect("delete frame"))["commit_sha"]
        .as_str()
        .expect("commit sha")
        .to_owned();
    let remote_head = TestMemory::remote_head(&bare);
    assert_push_preceded_notification(
        &messages,
        &["notes/gone-remote.md"],
        &sha,
        "delete_memory_file_response",
        &remote_head,
    );
}

#[tokio::test]
async fn push_failure_does_not_fail_the_committed_write() {
    let memory = bridge();
    memory.enable().await;
    // Point the push URL at an unreachable remote; the pinned contract keeps
    // the local commit and answers success so a later sync can retry.
    let missing = memory
        .backend_root
        .parent()
        .expect("fixture parent")
        .join("definitely/missing/remote.git");
    let url = missing.to_str().expect("utf8 url");
    memory.git_push_url(url);
    memory
        .send(&json!({
            "type": "write_memory_file",
            "request_id": "failed-push-write",
            "agent_id": memory.agent_id,
            "path": "notes/local-only.md",
            "content": "kept",
        }))
        .await;
    let value = encoded(memory.messages().last().expect("write frame"));
    assert_eq!(value["success"], true);
    assert_eq!(value["committed"], true);
    let sha = value["commit_sha"].as_str().expect("commit sha");
    // The commit survived locally even though the push failed.
    assert_eq!(memory.git(&["rev-parse", "HEAD"]), sha);
}
