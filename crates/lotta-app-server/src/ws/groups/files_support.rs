//! Shared fixtures for the files command-group certificate selectors.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde_json::Value;

use super::{FilesBridge, FilesCommand, FilesForwarder, FilesMessage};
use crate::{
    bounds::WS_FRAME_BYTES_MAX,
    framing,
    ws::{ConnectionId, files::decode as decode_files},
};

/// Watch poll interval used by tests so notices settle quickly.
pub(super) const TEST_POLL_INTERVAL_MS: u64 = 15;
/// First test connection identity.
pub(super) const CONNECTION_A: ConnectionId = 7;
/// Second test connection identity.
pub(super) const CONNECTION_B: ConnectionId = 8;
/// Upper bound for awaiting asynchronous watcher effects in tests.
const WAIT_ATTEMPTS_MAX: usize = 2_000;

/// Recorded forwarded messages as `(connection, message)` pairs.
pub(super) type RecordedMessages = Arc<Mutex<Vec<(ConnectionId, FilesMessage)>>>;

static WORKSPACE_ORDINAL: AtomicUsize = AtomicUsize::new(0);

/// A bridge over one unique temporary workspace plus its recorder.
pub(super) struct TestFiles {
    pub(super) bridge: FilesBridge,
    pub(super) workspace_root: PathBuf,
    pub(super) messages: RecordedMessages,
}

impl TestFiles {
    /// Writes one workspace-relative file with `content` before use.
    pub(super) fn write(&self, relative: &str, content: &str) -> PathBuf {
        let absolute = self.workspace_root.join(relative);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).expect("workspace parent directory");
        }
        std::fs::write(&absolute, content).expect("workspace file write");
        absolute
    }

    /// Reads one workspace-relative file back.
    pub(super) fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.workspace_root.join(relative)).expect("workspace file read")
    }

    /// Decodes a raw JSON command through framing and routes it to the bridge.
    pub(super) fn send(&self, connection: ConnectionId, command: &Value) {
        let text = command.to_string();
        let frame = framing::decode_text(&text).expect("bounded files frame");
        let decoded = decode_files(&frame)
            .expect("wellformed files command")
            .expect("files command routed");
        self.bridge.handle(connection, &decoded);
    }

    /// Sends an already typed command directly.
    pub(super) fn handle(&self, connection: ConnectionId, command: &FilesCommand) {
        self.bridge.handle(connection, command);
    }

    /// Snapshot of every forwarded message for one connection.
    pub(super) fn messages_for(&self, connection: ConnectionId) -> Vec<FilesMessage> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == connection)
            .map(|(_, message)| message.clone())
            .collect()
    }
}

/// Creates a bridge over a fresh canonical temporary workspace root.
pub(super) fn bridge() -> TestFiles {
    let ordinal = WORKSPACE_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let workspace_root =
        std::env::temp_dir().join(format!("lotta-files-ws-{}-{ordinal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace_root);
    std::fs::create_dir_all(&workspace_root).expect("workspace root");
    let workspace_root = workspace_root.canonicalize().expect("canonical root");
    let messages: RecordedMessages = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: FilesForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let bridge = FilesBridge::with_poll_interval(
        forward,
        &workspace_root,
        &workspace_root
            .parent()
            .unwrap_or(Path::new("/"))
            .join("lotta-files-artifacts"),
        TEST_POLL_INTERVAL_MS,
    )
    .expect("files bridge");
    TestFiles {
        bridge,
        workspace_root,
        messages,
    }
}

/// Waits until `condition` holds or the watcher deadline expires.
///
/// Sleeping through `tokio::time` keeps the current-thread test runtime
/// pumping watcher tasks while the test waits.
pub(super) async fn wait_until(condition: impl Fn() -> bool) -> bool {
    for _ in 0..WAIT_ATTEMPTS_MAX {
        if condition() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(TEST_POLL_INTERVAL_MS)).await;
    }
    condition()
}

/// Asserts `value` encodes under the transport frame ceiling.
pub(super) fn assert_under_frame_bound(value: &Value) {
    let encoded = value.to_string();
    assert!(
        encoded.len() <= WS_FRAME_BYTES_MAX,
        "response exceeded WS_FRAME_BYTES_MAX"
    );
}
