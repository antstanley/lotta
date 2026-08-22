//! Shared-policy confinement rejections for every path-taking command.
//!
//! Every path-taking command gets the three rejections the tool pipeline
//! produces through the shared Task 34 units: a `..` traversal, a symlink
//! escape, and an out-of-root absolute path.

use super::support::{CONNECTION_A, TEST_POLL_INTERVAL_MS, TestFiles, bridge};
use serde_json::{Value, json};

/// Name of the in-workspace symlink pointing outside the root.
const LINK_NAME: &str = "link";

/// One of the three canonical rejection shapes.
#[derive(Clone, Copy)]
enum BadPath {
    /// Lexical parent-directory escape.
    Traversal,
    /// In-root symlink resolving outside the workspace.
    SymlinkEscape,
    /// Absolute path rooted outside the workspace.
    Absolute,
}

fn encoded(message: &super::FilesMessage) -> Value {
    serde_json::to_value(message).expect("response encodes")
}

fn responses(files: &TestFiles) -> Vec<Value> {
    files
        .messages_for(CONNECTION_A)
        .iter()
        .map(encoded)
        .collect()
}

fn assert_failed(files: &TestFiles, discriminant: &str) {
    let failure = responses(files)
        .into_iter()
        .find(|value| value["type"] == discriminant)
        .unwrap_or_else(|| panic!("no {discriminant} recorded"));
    assert_eq!(failure["success"], false, "{discriminant} must fail");
    assert!(failure["error"].is_string());
}

/// Installs the escape fixture and returns the in-workspace path text.
///
/// The traversal shape matches the Task 37 tool confinement suite
/// (`../peer/...`): lexical normalization lands outside the root and is
/// rejected by the same shared check.
fn bad_target(files: &TestFiles, bad: BadPath) -> String {
    match bad {
        BadPath::Traversal => "../peer/outside.txt".to_owned(),
        BadPath::SymlinkEscape => {
            let outside_dir = files
                .workspace_root
                .parent()
                .expect("workspace parent")
                .join("lotta-files-outside");
            std::fs::create_dir_all(&outside_dir).expect("outside directory");
            std::fs::write(outside_dir.join("secret.txt"), "top secret").expect("outside file");
            std::os::unix::fs::symlink(&outside_dir, files.workspace_root.join(LINK_NAME))
                .expect("inside symlink");
            format!("{LINK_NAME}/secret.txt")
        }
        BadPath::Absolute => "/etc/hosts".to_owned(),
    }
}

/// Runs all three rejection shapes for one command builder. Responsive
/// commands must record a scrubbed failure; silent commands must stay silent.
/// The settle pause lets any wrongly created watcher task run before the
/// silence assertion, so a rejected watch can never emit a late notice.
async fn reject_all(build: impl Fn(&str) -> Value, discriminant: Option<&str>) {
    for bad in [
        BadPath::Traversal,
        BadPath::SymlinkEscape,
        BadPath::Absolute,
    ] {
        let files = bridge();
        let target = bad_target(&files, bad);
        files.send(CONNECTION_A, &build(&target));
        tokio::time::sleep(std::time::Duration::from_millis(TEST_POLL_INTERVAL_MS)).await;
        match discriminant {
            Some(kind) => assert_failed(&files, kind),
            None => assert!(
                files.messages_for(CONNECTION_A).is_empty(),
                "rejected command must stay silent"
            ),
        }
    }
}

#[tokio::test]
async fn search_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| json!({"type": "search_files", "query": "x", "cwd": bad, "request_id": "s"}),
        Some("search_files_response"),
    )
    .await;
}

#[tokio::test]
async fn grep_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| json!({"type": "grep_in_files", "query": "x", "cwd": bad, "request_id": "g"}),
        Some("grep_in_files_response"),
    )
    .await;
}

#[tokio::test]
async fn list_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| json!({"type": "list_in_directory", "path": bad}),
        Some("list_in_directory_response"),
    )
    .await;
}

#[tokio::test]
async fn tree_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| json!({"type": "get_tree", "path": bad, "depth": 2, "request_id": "t"}),
        Some("get_tree_response"),
    )
    .await;
}

#[tokio::test]
async fn read_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| json!({"type": "read_file", "path": bad, "request_id": "r"}),
        Some("read_file_response"),
    )
    .await;
}

#[tokio::test]
async fn write_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| json!({"type": "write_file", "path": bad, "content": "x", "request_id": "w"}),
        Some("write_file_response"),
    )
    .await;
}

#[tokio::test]
async fn edit_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| {
            json!({
                "type": "edit_file",
                "file_path": bad,
                "old_string": "a",
                "new_string": "b",
                "request_id": "e",
            })
        },
        Some("edit_file_response"),
    )
    .await;
}

#[tokio::test]
async fn watch_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| json!({"type": "watch_file", "path": bad, "request_id": "wc"}),
        None,
    )
    .await;
}

#[tokio::test]
async fn unwatch_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| json!({"type": "unwatch_file", "path": bad, "request_id": "uc"}),
        None,
    )
    .await;
}

#[tokio::test]
async fn ops_rejects_traversal_symlink_escape_and_absolute() {
    reject_all(
        |bad| {
            json!({
                "type": "file_ops",
                "path": bad,
                "cg_entries": [],
                "ops": [],
                "source": "window-1",
                "document_content": "tamper",
            })
        },
        None,
    )
    .await;
}
