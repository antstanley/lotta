//! Shared fixtures for the memory command-group certificate selectors.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use lotta_domain::Timestamp;
use lotta_testkit::clock::FakeClock;
use serde_json::{Value, json};

use super::{MemoryBridge, MemoryForwarder, MemoryMessage};
use crate::{framing, ws::ConnectionId};

/// First test connection identity.
pub(super) const CONNECTION_A: ConnectionId = 11;

type RecordedMessages = Arc<Mutex<Vec<(ConnectionId, MemoryMessage)>>>;

static BACKEND_ORDINAL: AtomicUsize = AtomicUsize::new(0);

fn fixture_timestamp() -> Timestamp {
    Timestamp::parse_persisted_rfc3339("2026-01-02T03:04:05Z").expect("fixture timestamp")
}

/// A bridge over one unique temporary backend plus its recorder.
pub(super) struct TestMemory {
    /// Bridge under test; every command applies inline via [`Self::send`].
    pub(super) bridge: Arc<MemoryBridge>,
    /// Canonical backend root handed to the bridge.
    pub(super) backend_root: PathBuf,
    /// Unwired sibling directory proving confinement targets the agent root.
    pub(super) workspace_root: PathBuf,
    /// Fixture agent identifier.
    pub(super) agent_id: String,
    messages: RecordedMessages,
}

impl TestMemory {
    /// Absolute agent memory repository directory.
    pub(super) fn memory_root(&self) -> PathBuf {
        self.backend_root
            .join("memfs")
            .join(&self.agent_id)
            .join("memory")
    }

    /// Decodes a raw JSON command through framing and applies it inline.
    pub(super) async fn send(&self, command: &Value) {
        let text = command.to_string();
        let frame = framing::decode_text(&text).expect("bounded memory frame");
        let decoded = super::decode(&frame)
            .expect("wellformed memory command")
            .expect("memory command routed");
        self.bridge.apply(CONNECTION_A, &decoded).await;
    }

    /// Enables memfs for the fixture agent through the bridge itself.
    pub(super) async fn enable(&self) {
        self.send(&json!({
            "type": "enable_memfs",
            "request_id": "enable-fixture",
            "agent_id": self.agent_id,
        }))
        .await;
    }

    /// Commits one UTF-8 file through the bridge and returns its revision.
    pub(super) async fn commit_write(&self, relative: &str, content: &str) -> String {
        self.send(&json!({
            "type": "write_memory_file",
            "request_id": "seed-write",
            "agent_id": self.agent_id,
            "path": relative,
            "content": content,
        }))
        .await;
        match self.messages().last() {
            Some(MemoryMessage::WriteFileResponse(frame)) => {
                frame.commit_sha.clone().expect("committed sha")
            }
            other => panic!("expected a write response, got {other:?}"),
        }
    }

    /// Snapshot of every forwarded message for the fixture connection.
    pub(super) fn messages(&self) -> Vec<MemoryMessage> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == CONNECTION_A)
            .map(|(_, message)| message.clone())
            .collect()
    }

    /// Seeds bytes directly into the working tree (read-path fixtures only).
    pub(super) fn seed(&self, relative: &str, bytes: &[u8]) {
        let absolute = self.memory_root().join(relative);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).expect("seed parent directory");
        }
        std::fs::write(absolute, bytes).expect("seed write");
    }

    /// Runs git inside the agent repository, returning trimmed stdout.
    #[must_use]
    pub(super) fn git(&self, args: &[&str]) -> String {
        git_in(&self.memory_root(), args)
    }

    /// Creates one bare remote beside the fixture and wires the push URL.
    ///
    /// The remote lives outside the backend root so the adapter never sees it.
    pub(super) fn wire_bare_remote(&self) -> PathBuf {
        let bare = self
            .backend_root
            .parent()
            .expect("fixture parent")
            .join("remote.git");
        std::fs::create_dir(&bare).expect("bare directory");
        git_in(&bare, &["init", "--bare"]);
        let url = bare.to_str().expect("utf8 remote url").to_owned();
        git_in(
            &self.memory_root(),
            &["config", "--local", "letta.memoryRepository.url", &url],
        );
        bare
    }

    /// Reads the bare remote's `main` head revision.
    #[must_use]
    pub(super) fn remote_head(bare: &Path) -> String {
        git_in(bare, &["rev-parse", "refs/heads/main"])
    }

    /// Points the repository's push URL at an arbitrary location.
    pub(super) fn git_push_url(&self, url: &str) {
        git_in(
            &self.memory_root(),
            &["config", "--local", "letta.memoryRepository.url", url],
        );
    }
}

fn git_in(directory: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git spawn");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Creates a bridge over a fresh canonical temporary backend root.
pub(super) fn bridge() -> TestMemory {
    let ordinal = BACKEND_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let parent =
        std::env::temp_dir().join(format!("lotta-memory-ws-{}-{ordinal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture parent");
    let workspace_root = parent.join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace root");
    let backend = parent.join("backend");
    std::fs::create_dir_all(&backend).expect("fixture backend");
    let backend_root = backend.canonicalize().expect("canonical root");
    let messages: RecordedMessages = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: MemoryForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let clock = Arc::new(FakeClock::new(fixture_timestamp()));
    let bridge = Arc::new(MemoryBridge::new(forward, &backend_root, clock).expect("memory bridge"));
    TestMemory {
        bridge,
        backend_root,
        workspace_root,
        agent_id: "agent-local-memory-test".to_owned(),
        messages,
    }
}

/// Outbound discriminant of one memory group message.
#[must_use]
pub(super) fn discriminant_of(message: &MemoryMessage) -> &'static str {
    match message {
        MemoryMessage::ListResponse(_) => "list_memory_response",
        MemoryMessage::HistoryResponse(_) => "memory_history_response",
        MemoryMessage::FileAtRefResponse(_) => "memory_file_at_ref_response",
        MemoryMessage::CommitDiffResponse(_) => "memory_commit_diff_response",
        MemoryMessage::ReadFileResponse(_) => "read_memory_file_response",
        MemoryMessage::WriteFileResponse(_) => "write_memory_file_response",
        MemoryMessage::DeleteFileResponse(_) => "delete_memory_file_response",
        MemoryMessage::EnableMemfsResponse(_) => "enable_memfs_response",
        MemoryMessage::Updated(_) => "memory_updated",
    }
}

pub(super) fn fixture_discriminants(section: &str) -> Vec<String> {
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

pub(super) fn assert_in_fixture(section: &str, tag: &str) {
    assert!(
        fixture_discriminants(section)
            .iter()
            .any(|entry| entry == tag),
        "{tag} missing from the {section} fixture group"
    );
}

/// Encodes one outbound message for field-level assertions.
pub(super) fn encoded(message: &MemoryMessage) -> Value {
    serde_json::to_value(message).expect("response encodes")
}
