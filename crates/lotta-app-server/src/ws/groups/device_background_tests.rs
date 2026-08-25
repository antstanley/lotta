//! `ws::device::background_snapshot_is_state` — background-process snapshots
//! travel inside the `update_device_status` listener state message and never
//! appear in the canonical model-facing tool inventory.

use std::{
    path::PathBuf,
    process::Command as StdCommand,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use lotta_tools::names;
use serde_json::{Value, json};

use super::{
    BackgroundProcessSource, DeviceBridge, DeviceForwarder, DeviceMessage, NoRunningProcesses,
};
use crate::ws::ConnectionId;

const CONNECTION: ConnectionId = 91;

static ROOT_ORDINAL: AtomicUsize = AtomicUsize::new(0);

fn unique_root() -> PathBuf {
    let ordinal = ROOT_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let parent =
        std::env::temp_dir().join(format!("lotta-device-bg-{}-{ordinal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    parent.canonicalize().expect("canonical root")
}

fn git(workspace: &std::path::Path, args: &[&str]) {
    let output = StdCommand::new("git")
        .current_dir(workspace)
        .args(args)
        .env("GIT_AUTHOR_NAME", "lotta-test")
        .env("GIT_AUTHOR_EMAIL", "test@lotta.dev")
        .env("GIT_COMMITTER_NAME", "lotta-test")
        .env("GIT_COMMITTER_EMAIL", "test@lotta.dev")
        .output()
        .expect("git runs in tests");
    assert!(output.status.success(), "git {args:?} failed");
}

#[tokio::test]
async fn emits_update_device_status_and_no_tool_entry() {
    // One real repository so a successful checkout triggers the pinned
    // post-checkout device-status refresh.
    let root = unique_root();
    let workspace = root.join("workspace");
    let storage = root.join("storage");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&storage).expect("storage");
    git(&workspace, &["init", "-b", "main"]);
    git(&workspace, &["commit", "--allow-empty", "-m", "init"]);

    let messages: Arc<Mutex<Vec<(ConnectionId, DeviceMessage)>>> = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: DeviceForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let bridge = DeviceBridge::new(forward, &workspace, &storage).expect("bridge");

    let command: Value = json!({
        "type": "checkout_branch",
        "request_id": "bg-1",
        "branch": "topic/bg",
        "create": true,
        "cwd": workspace.to_str().expect("utf-8 workspace"),
    });
    let frame = crate::framing::decode_text(&command.to_string()).expect("bounded frame");
    let decoded = super::decode(&frame)
        .expect("wellformed checkout")
        .expect("checkout routed");
    bridge.apply(CONNECTION, &decoded).await;

    let captured = messages.lock().expect("message lock").clone();
    assert_eq!(captured.len(), 2, "response plus one state refresh");
    let state = serde_json::to_value(captured[1].1.clone()).expect("encodes");
    assert_eq!(
        state["type"], "update_device_status",
        "background snapshots ride the update_device_status listener state message"
    );
    assert_eq!(
        state["background_processes"],
        json!([]),
        "no host-registered processes means an empty summary section"
    );

    // The default source stays inert and typed.
    let source: Arc<dyn BackgroundProcessSource> = Arc::new(NoRunningProcesses);
    assert!(source.snapshot().is_empty());

    // Even the fully canonical model-facing builtin inventory has no entry
    // under the background-snapshot names: they remain protocol services.
    for row in names::rows() {
        assert_ne!(row.internal, "process_manager", "no model-facing tool");
        assert_ne!(row.model, "process_manager");
        assert_ne!(row.internal, "background_process_snapshot");
        assert_ne!(row.model, "background_process_snapshot");
    }
}
