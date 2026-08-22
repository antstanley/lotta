//! One decode-route-respond case per files command in the pinned fixture.

use super::FilesMessage;
use super::support::{CONNECTION_A, bridge};
use serde_json::{Value, json};

fn fixture_discriminants(section: &str) -> Vec<String> {
    let raw = include_str!("../../../../../fixtures/protocol/discriminants.json");
    let fixture: Value = serde_json::from_str(raw).expect("bounded fixture");
    fixture[section]["discriminants"]
        .as_array()
        .expect("discriminants")
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn assert_in_fixture(section: &str, tag: &str) {
    assert!(
        fixture_discriminants(section)
            .iter()
            .any(|entry| entry == tag),
        "{tag} missing from the {section} fixture group"
    );
}

fn discriminants_of(message: &FilesMessage) -> &'static str {
    match message {
        FilesMessage::SearchFiles(_) => "search_files_response",
        FilesMessage::GrepInFiles(_) => "grep_in_files_response",
        FilesMessage::ListInDirectory(_) => "list_in_directory_response",
        FilesMessage::GetTree(_) => "get_tree_response",
        FilesMessage::ReadFile(_) => "read_file_response",
        FilesMessage::WriteFile(_) => "write_file_response",
        FilesMessage::FileOps(_) => "file_ops",
        FilesMessage::EditFile(_) => "edit_file_response",
        FilesMessage::Changed(_) => "file_changed",
    }
}

fn encoded(message: &FilesMessage) -> Value {
    serde_json::to_value(message).expect("response encodes")
}

#[tokio::test]
async fn search_files_decodes_routes_and_responds() {
    assert_in_fixture("commands", "search_files");
    let files = bridge();
    files.write("notes.txt", "hello");
    files.write("sub/report.log", "data");
    files.send(
        CONNECTION_A,
        &json!({
            "type": "search_files",
            "query": ".log",
            "max_results": 10,
            "request_id": "s1",
        }),
    );
    let responses = files.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    assert_in_fixture("messages", discriminants_of(&responses[0]));
    let value = encoded(&responses[0]);
    assert_eq!(value["request_id"], "s1");
    assert_eq!(value["success"], true);
    let entries = value["files"].as_array().expect("files array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "sub/report.log");
    assert_eq!(entries[0]["type"], "file");
}

#[tokio::test]
async fn grep_in_files_decodes_routes_and_responds() {
    assert_in_fixture("commands", "grep_in_files");
    let files = bridge();
    files.write("code/app.rs", "let value = compute();\n");
    files.send(
        CONNECTION_A,
        &json!({
            "type": "grep_in_files",
            "query": "compute",
            "max_results": 50,
            "context_lines": 1,
            "request_id": "g1",
        }),
    );
    let responses = files.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    assert_in_fixture("messages", discriminants_of(&responses[0]));
    let value = encoded(&responses[0]);
    assert_eq!(value["success"], true);
    assert_eq!(value["total_matches"], 1);
    assert_eq!(value["total_files"], 1);
    assert_eq!(value["truncated"], false);
    let matches = value["matches"].as_array().expect("matches array");
    assert_eq!(matches[0]["path"], "code/app.rs");
    assert_eq!(matches[0]["line"], 1);
    assert_eq!(matches[0]["column"], 13);
    assert_eq!(matches[0]["column_end"], 20);
    assert_eq!(matches[0]["text"], "let value = compute();");
    assert_eq!(matches[0]["before"].as_array().map(Vec::len), Some(0usize));
}

#[tokio::test]
async fn list_in_directory_decodes_routes_and_responds() {
    assert_in_fixture("commands", "list_in_directory");
    let files = bridge();
    files.write("alpha/inner.txt", "x");
    files.write("beta.txt", "y");
    files.send(
        CONNECTION_A,
        &json!({
            "type": "list_in_directory",
            "path": ".",
            "include_files": true,
            "offset": 0,
            "limit": 10,
            "request_id": "l1",
        }),
    );
    let responses = files.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    assert_in_fixture("messages", discriminants_of(&responses[0]));
    let value = encoded(&responses[0]);
    assert_eq!(value["path"], ".");
    assert_eq!(value["folders"], json!(["alpha"]));
    assert_eq!(value["files"], json!(["beta.txt"]));
    assert_eq!(value["hasMore"], false);
    assert_eq!(value["total"], 2);
    assert_eq!(value["success"], true);
    assert_eq!(value["request_id"], "l1");
}

#[tokio::test]
async fn get_tree_decodes_routes_and_responds() {
    assert_in_fixture("commands", "get_tree");
    let files = bridge();
    files.write("src/main.rs", "fn main() {}\n");
    files.write("src/deep/util.rs", "ok\n");
    files.send(
        CONNECTION_A,
        &json!({
            "type": "get_tree",
            "path": ".",
            "depth": 3,
            "request_id": "t1",
        }),
    );
    let responses = files.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    assert_in_fixture("messages", discriminants_of(&responses[0]));
    let value = encoded(&responses[0]);
    assert_eq!(value["success"], true);
    assert_eq!(value["has_more_depth"], false);
    let entries = value["entries"].as_array().expect("entries array");
    let paths: Vec<&str> = entries
        .iter()
        .map(|entry| entry["path"].as_str().expect("entry path"))
        .collect();
    // Pinned breadth-first order: the root level's entries precede deeper ones.
    assert_eq!(
        paths,
        vec!["src", "src/deep", "src/main.rs", "src/deep/util.rs"]
    );
    assert_eq!(entries[0]["type"], "dir");
    assert_eq!(entries[3]["type"], "file");
}

#[tokio::test]
async fn read_file_decodes_routes_and_responds() {
    assert_in_fixture("commands", "read_file");
    let files = bridge();
    files.write("doc.md", "# title\n");
    files.send(
        CONNECTION_A,
        &json!({
            "type": "read_file",
            "path": "doc.md",
            "encoding": "utf8",
            "request_id": "r1",
        }),
    );
    let responses = files.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    assert_in_fixture("messages", discriminants_of(&responses[0]));
    let value = encoded(&responses[0]);
    assert_eq!(value["request_id"], "r1");
    assert_eq!(value["path"], "doc.md");
    assert_eq!(value["content"], "# title\n");
    assert_eq!(value["encoding"], "utf8");
    assert_eq!(value["success"], true);
}

#[tokio::test]
async fn write_file_decodes_routes_and_responds() {
    assert_in_fixture("commands", "write_file");
    let files = bridge();
    files.send(
        CONNECTION_A,
        &json!({
            "type": "write_file",
            "path": "new/thing.txt",
            "content": "created by the listener",
            "request_id": "w1",
        }),
    );
    let responses = files.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    assert_in_fixture("messages", discriminants_of(&responses[0]));
    let value = encoded(&responses[0]);
    assert_eq!(value["request_id"], "w1");
    assert_eq!(value["path"], "new/thing.txt");
    assert_eq!(value["success"], true);
    assert_eq!(files.read("new/thing.txt"), "created by the listener");
}

#[tokio::test]
async fn watch_file_decodes_and_stays_silent() {
    assert_in_fixture("commands", "watch_file");
    let files = bridge();
    let target = files.write("watched.txt", "v1");
    files.send(
        CONNECTION_A,
        &json!({
            "type": "watch_file",
            "path": "watched.txt",
            "request_id": "wc1",
        }),
    );
    assert!(files.messages_for(CONNECTION_A).is_empty());
    assert_eq!(files.bridge.watcher_count(CONNECTION_A), 1);
    // Let the clock advance past the captured millisecond so the poll sees a
    // different mtime.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    std::fs::write(&target, "v2").expect("modification");
    assert!(super::support::wait_until(|| !files.messages_for(CONNECTION_A).is_empty()).await);
    let notices = files.messages_for(CONNECTION_A);
    assert_in_fixture("messages", discriminants_of(&notices[0]));
    let value = encoded(&notices[0]);
    assert_eq!(value["type"], "file_changed");
    assert_eq!(value["path"], "watched.txt");
    assert!(value["lastModified"].as_u64().expect("mtime") > 0);
}

#[tokio::test]
async fn unwatch_file_decodes_and_stays_silent() {
    assert_in_fixture("commands", "unwatch_file");
    let files = bridge();
    files.write("watched.txt", "v1");
    files.handle(
        CONNECTION_A,
        &super::FilesCommand::Watch(super::WatchFileCommand {
            path: "watched.txt".into(),
            request_id: "wc2".into(),
        }),
    );
    files.send(
        CONNECTION_A,
        &json!({
            "type": "unwatch_file",
            "path": "watched.txt",
            "request_id": "uc2",
        }),
    );
    assert!(files.messages_for(CONNECTION_A).is_empty());
    assert_eq!(files.bridge.watcher_count(CONNECTION_A), 0);
}

#[tokio::test]
async fn edit_file_decodes_routes_and_responds_with_ops_notice() {
    assert_in_fixture("commands", "edit_file");
    let files = bridge();
    files.write("editable.txt", "first line\nsecond line\n");
    files.send(
        CONNECTION_A,
        &json!({
            "type": "edit_file",
            "file_path": "editable.txt",
            "old_string": "second line",
            "new_string": "final line",
            "replace_all": false,
            "request_id": "e1",
        }),
    );
    let responses = files.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 2);
    assert_in_fixture("messages", discriminants_of(&responses[0]));
    assert_in_fixture("messages", discriminants_of(&responses[1]));
    let notice = encoded(&responses[0]);
    assert_eq!(notice["type"], "file_ops");
    assert_eq!(notice["path"], "editable.txt");
    assert_eq!(notice["source"], "agent");
    assert!(notice["document_content"].as_str().is_some());
    let response = encoded(&responses[1]);
    assert_eq!(response["type"], "edit_file_response");
    assert_eq!(response["request_id"], "e1");
    assert_eq!(response["replacements"], 1);
    assert_eq!(response["start_line"], 2);
    assert_eq!(response["success"], true);
    assert_eq!(files.read("editable.txt"), "first line\nfinal line\n");
}

#[tokio::test]
async fn file_ops_decodes_writes_content_and_stays_silent() {
    assert_in_fixture("commands", "file_ops");
    let files = bridge();
    files.write("shared.md", "stale\n");
    files.send(
        CONNECTION_A,
        &json!({
            "type": "file_ops",
            "path": "shared.md",
            "cg_entries": [],
            "ops": [],
            "source": "window-1",
            "document_content": "client document state",
        }),
    );
    assert!(files.messages_for(CONNECTION_A).is_empty());
    assert_eq!(files.read("shared.md"), "client document state");
}
