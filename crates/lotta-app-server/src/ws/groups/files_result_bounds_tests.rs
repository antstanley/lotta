//! Large tree, grep, listing, and read responses stay under the frame bound.

use super::support::{CONNECTION_A, assert_under_frame_bound, bridge};
use super::{
    GREP_MATCHES_MAX, LIST_ENTRIES_MAX, READ_BASE64_BYTES_MAX, READ_UTF8_BYTES_MAX,
    TREE_ENTRIES_MAX,
};
use serde_json::Value;

fn encoded(message: &super::FilesMessage) -> Value {
    serde_json::to_value(message).expect("response encodes")
}

fn single_response(files: &super::support::TestFiles) -> Value {
    let messages = files.messages_for(CONNECTION_A);
    assert_eq!(messages.len(), 1, "exactly one response expected");
    let value = encoded(&messages[0]);
    assert_under_frame_bound(&value);
    value
}

#[tokio::test]
async fn large_tree_is_capped_and_bounded_under_frame_max() {
    let files = bridge();
    // One more than the pinned entry cap spread over shallow directories.
    for index in 0..=TREE_ENTRIES_MAX {
        files.write(&format!("d{}/file-{index}.txt", index % 7), "x");
    }
    files.send(
        CONNECTION_A,
        &serde_json::json!({"type": "get_tree", "path": ".", "depth": 3, "request_id": "t"}),
    );
    let value = single_response(&files);
    assert_eq!(value["type"], "get_tree_response");
    let entries = value["entries"].as_array().expect("entries array");
    assert_eq!(
        entries.len(),
        TREE_ENTRIES_MAX,
        "tree entries must stop at the named cap"
    );
    assert_eq!(value["has_more_depth"], true, "cap must be signalled");
}

#[tokio::test]
async fn large_grep_is_capped_and_bounded_under_frame_max() {
    let files = bridge();
    // Far more matching lines than the returned-row cap.
    let body = "needle here\n".repeat(GREP_MATCHES_MAX * 4);
    files.write("haystack.txt", &body);
    files.send(
        CONNECTION_A,
        &serde_json::json!({
            "type": "grep_in_files",
            "query": "needle",
            "max_results": GREP_MATCHES_MAX,
            "request_id": "g",
        }),
    );
    let value = single_response(&files);
    assert_eq!(value["type"], "grep_in_files_response");
    let matches = value["matches"].as_array().expect("matches array");
    assert_eq!(matches.len(), GREP_MATCHES_MAX);
    assert_eq!(
        value["truncated"], true,
        "unreturned matches must be signalled"
    );
    assert_eq!(value["total_matches"], GREP_MATCHES_MAX * 4);
}

#[tokio::test]
async fn large_listing_is_capped_and_paginated_under_frame_max() {
    let files = bridge();
    for index in 0..=LIST_ENTRIES_MAX {
        files.write(&format!("entry-{index:05}.txt"), "x");
    }
    files.send(
        CONNECTION_A,
        &serde_json::json!({
            "type": "list_in_directory",
            "path": ".",
            "include_files": true,
            "offset": 0,
            "limit": LIST_ENTRIES_MAX * 2,
            "request_id": "l",
        }),
    );
    let value = single_response(&files);
    assert_eq!(value["type"], "list_in_directory_response");
    let folders = value["folders"].as_array().expect("folders array");
    let listed = folders.len() + value["files"].as_array().map_or(0, Vec::len);
    assert_eq!(
        listed, LIST_ENTRIES_MAX,
        "listing must stop at the named cap"
    );
    assert_eq!(value["hasMore"], true);
}

#[tokio::test]
async fn base64_read_of_oversized_file_fails_without_oversized_frame() {
    let files = bridge();
    let big = "b".repeat(READ_BASE64_BYTES_MAX + 1);
    files.write("huge.bin", &big);
    files.send(
        CONNECTION_A,
        &serde_json::json!({
            "type": "read_file",
            "path": "huge.bin",
            "encoding": "base64",
            "request_id": "r1",
        }),
    );
    let value = single_response(&files);
    assert_eq!(value["success"], false, "oversized base64 read must fail");
    assert!(value["content"].is_null());
}

#[tokio::test]
async fn base64_read_of_large_file_stays_under_frame_max() {
    let files = bridge();
    let big = "b".repeat(3 * 1024 * 1024);
    files.write("large.bin", &big);
    files.send(
        CONNECTION_A,
        &serde_json::json!({
            "type": "read_file",
            "path": "large.bin",
            "encoding": "base64",
            "request_id": "r2",
        }),
    );
    let value = single_response(&files);
    assert_eq!(value["success"], true);
    let encoded_length = value["content"].as_str().expect("base64 body").len();
    assert!(encoded_length > 0);
}

#[tokio::test]
async fn utf8_read_of_oversized_file_fails_without_oversized_frame() {
    let files = bridge();
    let big = "u".repeat(READ_UTF8_BYTES_MAX + 1);
    files.write("huge.txt", &big);
    files.send(
        CONNECTION_A,
        &serde_json::json!({
            "type": "read_file",
            "path": "huge.txt",
            "encoding": "utf8",
            "request_id": "r3",
        }),
    );
    let value = single_response(&files);
    assert_eq!(value["success"], false);
}
