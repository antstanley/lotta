//! Bounded per-connection watcher set lifecycle and notification semantics.

use super::support::{CONNECTION_A, CONNECTION_B, TEST_POLL_INTERVAL_MS, bridge, wait_until};
use super::{FILE_WATCHERS_PER_CONNECTION_MAX, FilesCommand, UnwatchFileCommand, WatchFileCommand};
use serde_json::json;

fn watch(path: &str) -> FilesCommand {
    FilesCommand::Watch(WatchFileCommand {
        path: path.to_owned(),
        request_id: "wc".into(),
    })
}

fn unwatch(path: &str) -> FilesCommand {
    FilesCommand::Unwatch(UnwatchFileCommand {
        path: path.to_owned(),
        request_id: "uc".into(),
    })
}

#[tokio::test]
async fn watcher_set_is_bounded_per_connection() {
    let files = bridge();
    for index in 0..=FILE_WATCHERS_PER_CONNECTION_MAX {
        let name = format!("watch-{index}.txt");
        files.write(&name, "content");
        files.handle(CONNECTION_A, &watch(&name));
    }
    assert_eq!(
        files.bridge.watcher_count(CONNECTION_A),
        FILE_WATCHERS_PER_CONNECTION_MAX,
        "watchers beyond the per-connection cap must be refused"
    );
    // The cap is per connection: another connection can still watch.
    files.handle(CONNECTION_B, &watch("watch-0.txt"));
    assert_eq!(files.bridge.watcher_count(CONNECTION_B), 1);
}

#[tokio::test]
async fn unwatch_removes_one_watcher() {
    let files = bridge();
    files.write("first.txt", "a");
    files.write("second.txt", "b");
    files.handle(CONNECTION_A, &watch("first.txt"));
    files.handle(CONNECTION_A, &watch("second.txt"));
    assert_eq!(files.bridge.watcher_count(CONNECTION_A), 2);
    files.handle(CONNECTION_A, &unwatch("first.txt"));
    assert_eq!(files.bridge.watcher_count(CONNECTION_A), 1);
    // Removing an unwatched path is a no-op that leaves the other intact.
    files.handle(CONNECTION_A, &unwatch("absent.txt"));
    assert_eq!(files.bridge.watcher_count(CONNECTION_A), 1);
    files.handle(CONNECTION_A, &unwatch("second.txt"));
    assert_eq!(files.bridge.watcher_count(CONNECTION_A), 0);
}

#[tokio::test]
async fn close_removes_all_watchers_and_stops_tasks() {
    let files = bridge();
    for index in 0..3 {
        files.write(&format!("owned-{index}.txt"), "x");
        files.handle(CONNECTION_A, &watch(&format!("owned-{index}.txt")));
    }
    files.write("other-connection.txt", "y");
    files.handle(CONNECTION_B, &watch("other-connection.txt"));
    assert_eq!(files.bridge.watcher_count(CONNECTION_A), 3);
    files.bridge.disconnect(CONNECTION_A);
    assert_eq!(
        files.bridge.watcher_count(CONNECTION_A),
        0,
        "connection close must remove all of its watchers"
    );
    assert_eq!(
        files.bridge.watcher_count(CONNECTION_B),
        1,
        "other connections keep their watchers"
    );
    // Cancelled tasks stay cancelled: modifying a formerly watched file
    // must not resurrect notices for the closed connection.
    std::fs::write(files.workspace_root.join("owned-0.txt"), "after close")
        .expect("post-close write");
    tokio::time::sleep(std::time::Duration::from_millis(TEST_POLL_INTERVAL_MS * 4)).await;
    assert!(files.messages_for(CONNECTION_A).is_empty());
}

#[tokio::test]
async fn modification_emits_one_bounded_notice() {
    let files = bridge();
    let target = files.write("notified.txt", "before");
    files.handle(CONNECTION_A, &watch("notified.txt"));
    // Advance past the captured millisecond so the poll observes the change.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    std::fs::write(&target, "after").expect("modification");
    assert!(wait_until(|| !files.messages_for(CONNECTION_A).is_empty()).await);
    let notices = files.messages_for(CONNECTION_A);
    assert_eq!(notices.len(), 1, "one change collapses into one notice");
    let notice = serde_json::to_value(&notices[0]).expect("notice encodes");
    assert_eq!(notice["type"], "file_changed");
    assert_eq!(notice["path"], "notified.txt");
    assert!(notice["lastModified"].as_u64().expect("mtime") > 0);
}

#[tokio::test]
async fn deletion_stops_the_watcher_silently() {
    let files = bridge();
    let target = files.write("vanishing.txt", "soon gone");
    files.handle(CONNECTION_A, &watch("vanishing.txt"));
    std::fs::remove_file(&target).expect("deletion");
    assert!(
        wait_until(|| files.bridge.watcher_count(CONNECTION_A) == 0).await,
        "watcher must self-remove after its file disappears"
    );
    assert!(files.messages_for(CONNECTION_A).is_empty());
}

#[tokio::test]
async fn duplicate_watch_is_refused_not_duplicated() {
    let files = bridge();
    files.write("once.txt", "stable");
    files.handle(CONNECTION_A, &watch("once.txt"));
    files.handle(CONNECTION_A, &watch("once.txt"));
    assert_eq!(files.bridge.watcher_count(CONNECTION_A), 1);
    let target = files.workspace_root.join("once.txt");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    std::fs::write(&target, "changed").expect("modification");
    assert!(wait_until(|| !files.messages_for(CONNECTION_A).is_empty()).await);
    assert_eq!(
        files.messages_for(CONNECTION_A).len(),
        1,
        "one watcher per file means one notice per change"
    );
}

#[tokio::test]
async fn json_watch_round_trip_through_decode() {
    let files = bridge();
    files.write("wired.txt", "v1");
    files.send(
        CONNECTION_A,
        &json!({"type": "watch_file", "path": "wired.txt", "request_id": "wc-json"}),
    );
    files.send(
        CONNECTION_A,
        &json!({"type": "unwatch_file", "path": "wired.txt", "request_id": "uc-json"}),
    );
    assert!(files.messages_for(CONNECTION_A).is_empty());
    assert_eq!(files.bridge.watcher_count(CONNECTION_A), 0);
}
