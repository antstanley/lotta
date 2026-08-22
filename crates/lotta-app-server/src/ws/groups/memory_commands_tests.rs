//! One decode-route-respond case per memory command in the pinned fixture.

use super::support::{assert_in_fixture, bridge, discriminant_of, encoded};
use base64::Engine as _;
use serde_json::json;

#[tokio::test]
async fn list_memory_decodes_routes_and_responds() {
    assert_in_fixture("commands", "list_memory");
    let memory = bridge();
    memory.enable().await;
    memory.seed(
        "system/core.md",
        b"---\ndescription: core system doc\n---\nsee [[plain]]\n",
    );
    memory.seed("plain.md", b"---\ndescription: plain\n---\nbody [[core]]\n");
    memory.seed("pic.png", &[0x89, b'P', b'N', b'G']);
    memory.seed("skipped.txt", b"not memory material");
    memory
        .send(&json!({
            "type": "list_memory",
            "request_id": "l1",
            "agent_id": memory.agent_id,
            "include_references": true,
        }))
        .await;
    let responses = memory.messages();
    let listing = responses.last().expect("listing frame");
    assert_eq!(discriminant_of(listing), "list_memory_response");
    assert_in_fixture("messages", discriminant_of(listing));
    let value = encoded(listing);
    assert_eq!(value["request_id"], "l1");
    assert_eq!(value["success"], true);
    assert_eq!(value["done"], true);
    assert_eq!(value["total"], 3);
    assert_eq!(value["memfs_enabled"], true);
    assert_eq!(value["memfs_initialized"], true);
    let entries = value["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 3);
    let plain = entries
        .iter()
        .find(|e| e["relative_path"] == "plain.md")
        .expect("md");
    assert_eq!(plain["kind"], "markdown");
    assert_eq!(plain["mime_type"], "text/markdown");
    assert_eq!(plain["description"], "plain");
    assert_eq!(plain["content"], "body [[core]]");
    assert_eq!(plain["references"], serde_json::json!(["system/core.md"]),);
    let image = entries
        .iter()
        .find(|e| e["relative_path"] == "pic.png")
        .expect("png");
    assert_eq!(image["kind"], "image");
    assert_eq!(image["mime_type"], "image/png");
    assert_eq!(image["size"], 4);
    let system = entries
        .iter()
        .find(|e| e["relative_path"] == "system/core.md")
        .expect("sys");
    assert_eq!(system["is_system"], true);
    assert_eq!(system["description"], "core system doc");
}

#[tokio::test]
async fn enable_memfs_decodes_routes_and_responds() {
    assert_in_fixture("commands", "enable_memfs");
    let memory = bridge();
    memory
        .send(&json!({
            "type": "enable_memfs",
            "request_id": "en1",
            "agent_id": memory.agent_id,
        }))
        .await;
    let responses = memory.messages();
    assert_eq!(responses.len(), 2);
    assert_in_fixture("messages", discriminant_of(&responses[0]));
    let value = encoded(&responses[0]);
    assert_eq!(value["type"], "enable_memfs_response");
    assert_eq!(value["request_id"], "en1");
    assert_eq!(value["success"], true);
    let directory = value["memory_directory"].as_str().expect("directory");
    assert!(directory.ends_with("memfs/agent-local-memory-test/memory"));
    assert!(memory.memory_root().join(".git").exists());
}

#[tokio::test]
async fn write_memory_file_decodes_routes_and_responds() {
    assert_in_fixture("commands", "write_memory_file");
    let memory = bridge();
    memory.enable().await;
    let before = memory.messages().len();
    memory
        .send(&json!({
            "type": "write_memory_file",
            "request_id": "w1",
            "agent_id": memory.agent_id,
            "path": "notes/a.md",
            "content": "---\ndescription: note\n---\nhello",
            "commit_message": "first note",
        }))
        .await;
    let responses = memory.messages();
    assert_eq!(responses.len(), before + 2);
    let value = encoded(responses.last().expect("write response"));
    assert_eq!(value["type"], "write_memory_file_response");
    assert_in_fixture("messages", "write_memory_file_response");
    assert_eq!(value["request_id"], "w1");
    assert_eq!(value["path"], "notes/a.md");
    assert_eq!(value["success"], true);
    assert_eq!(value["committed"], true);
    let sha = value["commit_sha"].as_str().expect("commit sha");
    assert_eq!(sha.len(), 40);
    assert_eq!(memory.git(&["rev-parse", "HEAD"]), sha);
    let on_disk = std::fs::read(memory.memory_root().join("notes/a.md")).expect("written file");
    assert_eq!(on_disk, b"---\ndescription: note\n---\nhello");
}

#[tokio::test]
async fn delete_memory_file_decodes_routes_and_responds() {
    assert_in_fixture("commands", "delete_memory_file");
    let memory = bridge();
    memory.enable().await;
    memory.commit_write("notes/gone.md", "bye").await;
    let before = memory.messages().len();
    memory
        .send(&json!({
            "type": "delete_memory_file",
            "request_id": "d1",
            "agent_id": memory.agent_id,
            "path": "notes/gone.md",
        }))
        .await;
    let responses = memory.messages();
    assert_eq!(responses.len(), before + 2);
    let value = encoded(responses.last().expect("delete response"));
    assert_eq!(value["type"], "delete_memory_file_response");
    assert_in_fixture("messages", "delete_memory_file_response");
    assert_eq!(value["request_id"], "d1");
    assert_eq!(value["success"], true);
    assert_eq!(value["committed"], true);
    assert!(value["commit_sha"].is_string());
    assert!(!memory.memory_root().join("notes/gone.md").exists());
    // Idempotent: deleting the already-absent file commits nothing.
    memory
        .send(&json!({
            "type": "delete_memory_file",
            "request_id": "d2",
            "agent_id": memory.agent_id,
            "path": "notes/gone.md",
        }))
        .await;
    let again = memory.messages();
    assert_eq!(again.len(), responses.len() + 1);
    let repeat = encoded(again.last().expect("repeat delete response"));
    assert_eq!(repeat["success"], true);
    assert_eq!(repeat["committed"], false);
    assert!(repeat.get("commit_sha").is_none());
}

#[tokio::test]
async fn read_memory_file_decodes_routes_and_responds() {
    assert_in_fixture("commands", "read_memory_file");
    let memory = bridge();
    memory.enable().await;
    memory.seed("docs/readme.md", b"# hello\n");
    memory
        .send(&json!({
            "type": "read_memory_file",
            "request_id": "r1",
            "agent_id": memory.agent_id,
            "path": "docs/readme.md",
        }))
        .await;
    let utf8 = encoded(memory.messages().last().expect("read response"));
    assert_eq!(utf8["type"], "read_memory_file_response");
    assert_in_fixture("messages", "read_memory_file_response");
    assert_eq!(utf8["request_id"], "r1");
    assert_eq!(utf8["success"], true);
    assert_eq!(utf8["encoding"], "utf8");
    assert_eq!(utf8["content"], "# hello\n");
    memory
        .send(&json!({
            "type": "read_memory_file",
            "request_id": "r2",
            "agent_id": memory.agent_id,
            "path": "docs/readme.md",
            "encoding": "base64",
        }))
        .await;
    let base64_frame = encoded(memory.messages().last().expect("base64 read"));
    assert_eq!(base64_frame["encoding"], "base64");
    assert_eq!(
        base64_frame["content"],
        base64::engine::general_purpose::STANDARD.encode(b"# hello\n")
    );
}

#[tokio::test]
async fn memory_history_decodes_routes_and_responds() {
    assert_in_fixture("commands", "memory_history");
    let memory = bridge();
    memory.enable().await;
    memory.commit_write("notes/h.md", "one").await;
    memory.commit_write("notes/h.md", "two").await;
    memory
        .send(&json!({
            "type": "memory_history",
            "request_id": "h1",
            "agent_id": memory.agent_id,
            "limit": 10,
        }))
        .await;
    let history = memory.messages().last().expect("history frame").clone();
    assert_eq!(discriminant_of(&history), "memory_history_response");
    assert_in_fixture("messages", discriminant_of(&history));
    let value = encoded(&history);
    assert_eq!(value["request_id"], "h1");
    assert_eq!(value["file_path"], "");
    assert_eq!(value["success"], true);
    let commits = value["commits"].as_array().expect("commits array");
    // initialize plus two writes, newest first.
    assert_eq!(commits.len(), 3);
    assert_eq!(
        commits[0]["sha"],
        memory.git(&["rev-parse", "HEAD"]).as_str()
    );
    assert_eq!(commits[0]["message"], "Update memory file notes/h.md");
    assert_eq!(commits[0]["timestamp"], "");
    assert!(commits[0]["author_name"].is_null());
    assert_eq!(commits[2]["message"], "initialize");
}

#[tokio::test]
async fn memory_file_at_ref_decodes_routes_and_responds() {
    assert_in_fixture("commands", "memory_file_at_ref");
    let memory = bridge();
    memory.enable().await;
    let first = memory.commit_write("journal/log.md", "v1").await;
    memory.commit_write("journal/log.md", "v2").await;
    memory
        .send(&json!({
            "type": "memory_file_at_ref",
            "request_id": "f1",
            "agent_id": memory.agent_id,
            "file_path": "journal/log.md",
            "ref": first,
        }))
        .await;
    let at_ref = memory.messages().last().expect("at-ref frame").clone();
    assert_eq!(discriminant_of(&at_ref), "memory_file_at_ref_response");
    assert_in_fixture("messages", discriminant_of(&at_ref));
    let value = encoded(&at_ref);
    assert_eq!(value["request_id"], "f1");
    assert_eq!(value["ref"], first.as_str());
    assert_eq!(value["file_path"], "journal/log.md");
    assert_eq!(value["success"], true);
    assert_eq!(value["content"], "v1");
}

#[tokio::test]
async fn memory_commit_diff_decodes_routes_and_responds() {
    assert_in_fixture("commands", "memory_commit_diff");
    let memory = bridge();
    memory.enable().await;
    memory.commit_write("doc.md", "first line\n").await;
    let second = memory.commit_write("doc.md", "second line\n").await;
    memory
        .send(&json!({
            "type": "memory_commit_diff",
            "request_id": "diff-1",
            "agent_id": memory.agent_id,
            "sha": second,
        }))
        .await;
    let diff = memory.messages().last().expect("diff frame").clone();
    assert_eq!(discriminant_of(&diff), "memory_commit_diff_response");
    assert_in_fixture("messages", discriminant_of(&diff));
    let value = encoded(&diff);
    assert_eq!(value["request_id"], "diff-1");
    assert_eq!(value["sha"], second.as_str());
    assert_eq!(value["success"], true);
    let patch = value["diff"].as_str().expect("patch text");
    assert!(patch.contains("- first line"), "patch was {patch}");
    assert!(patch.contains("+ second line"), "patch was {patch}");
}
