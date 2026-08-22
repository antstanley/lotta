//! Confinement parity with the Task 39 memory tools, against the agent memory
//! root — traversal, symlink, and absolute-path classes.

use super::support::{bridge, encoded};
use serde_json::json;

async fn rejected_write(memory: &super::support::TestMemory, path: &str) -> serde_json::Value {
    memory
        .send(&json!({
            "type": "write_memory_file",
            "request_id": "confine-write",
            "agent_id": memory.agent_id,
            "path": path,
            "content": "escaped?",
        }))
        .await;
    encoded(memory.messages().last().expect("write frame"))
}

async fn rejected_read(memory: &super::support::TestMemory, path: &str) -> serde_json::Value {
    memory
        .send(&json!({
            "type": "read_memory_file",
            "request_id": "confine-read",
            "agent_id": memory.agent_id,
            "path": path,
        }))
        .await;
    encoded(memory.messages().last().expect("read frame"))
}

#[tokio::test]
async fn traversal_and_absolute_paths_are_rejected() {
    let memory = bridge();
    memory.enable().await;
    let neighbor = memory.backend_root.join("memfs").join("peer.md");
    std::fs::write(&neighbor, b"outside").expect("neighbor sentinel");
    for path in ["../peer.md", "sub/../../escape.md", ".", ".."] {
        let write = rejected_write(&memory, path).await;
        assert_eq!(write["success"], false, "accepted traversal {path:?}");
        assert!(
            write["error"]
                .as_str()
                .expect("rejection detail")
                .contains("resolve inside the memory root"),
            "wrong class for {path:?}: {write}"
        );
        let read = rejected_read(&memory, path).await;
        assert_eq!(read["success"], false, "accepted traversal {path:?}");
    }
    for path in ["/etc/passwd", ""] {
        let write = rejected_write(&memory, path).await;
        assert_eq!(write["success"], false, "accepted absolute {path:?}");
        assert!(
            write["error"]
                .as_str()
                .expect("rejection detail")
                .contains("non-empty relative path"),
            "wrong class for {path:?}: {write}"
        );
        let read = rejected_read(&memory, path).await;
        assert_eq!(read["success"], false, "accepted absolute {path:?}");
    }
    assert_eq!(
        std::fs::read(&neighbor).expect("neighbor sentinel intact"),
        b"outside"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_is_rejected() {
    use std::os::unix::fs::symlink;

    let memory = bridge();
    memory.enable().await;
    let outside = memory.backend_root.join("outside");
    std::fs::create_dir_all(&outside).expect("outside directory");
    let sentinel = outside.join("sentinel.md");
    std::fs::write(&sentinel, b"untouched").expect("sentinel");
    let root = memory.memory_root();
    symlink(&sentinel, root.join("link.md")).expect("file symlink");
    symlink(&outside, root.join("dir")).expect("directory symlink");

    let through_file = rejected_write(&memory, "link.md").await;
    assert_eq!(through_file["success"], false, "wrote through a symlink");
    let through_directory = rejected_write(&memory, "dir/x.md").await;
    assert_eq!(
        through_directory["success"], false,
        "wrote through a symlinked ancestor"
    );
    let read_back = rejected_read(&memory, "link.md").await;
    assert_eq!(read_back["success"], false, "read through a symlink");
    assert_eq!(
        std::fs::read(&sentinel).expect("sentinel intact"),
        b"untouched"
    );
    assert!(
        !outside.join("x.md").exists(),
        "symlinked ancestor received bytes"
    );
}

#[tokio::test]
async fn confinement_targets_agent_memory_root_not_workspace() {
    let memory = bridge();
    memory.enable().await;
    memory
        .send(&json!({
            "type": "write_memory_file",
            "request_id": "root-check",
            "agent_id": memory.agent_id,
            "path": "shared-name.md",
            "content": "in memory",
        }))
        .await;
    let value = encoded(memory.messages().last().expect("write frame"));
    assert_eq!(value["success"], true);
    assert!(
        memory.memory_root().join("shared-name.md").exists(),
        "write missed the agent memory root"
    );
    // The workspace sandbox directory was never wired into the bridge, so a
    // workspace-rooted confinement could not have produced this file — but the
    // absence assertions pin the root choice explicitly.
    assert!(!memory.workspace_root.join("shared-name.md").exists());
    assert!(!memory.backend_root.join("shared-name.md").exists());
    assert!(
        !memory
            .backend_root
            .join("memfs")
            .join("shared-name.md")
            .exists()
    );
}
