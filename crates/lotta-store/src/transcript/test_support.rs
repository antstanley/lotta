use crate::LocalStore;
use lotta_domain::{
    AgentId, BoundedJsonValue, CompactionEntry, CompactionEntryType, ConversationId, LocalMessage,
    LocalMessageRole, MessageEntry, MessageEntryType, MessageId, NonEmptyString, ProviderStack,
    SessionEntry, SessionEntryType, Timestamp, TranscriptEntry, TranscriptManifest,
    TranscriptMessageFormat,
};
use lotta_testkit::roots::TemporaryRoot;
use serde_json::json;
use std::io::Write as _;
use std::path::PathBuf;

const AGENT: &str = "agent-transcript-test";
const CONVERSATION: &str = "default";

pub(crate) fn setup(label: &str) -> (TemporaryRoot, LocalStore, AgentId, ConversationId) {
    let owned = TemporaryRoot::new(label).expect("temporary root");
    let paths = crate::StorePaths::new(owned.path().join("backend")).expect("paths");
    let agent = AgentId::accept(AGENT).expect("agent");
    let conversation = ConversationId::accept(CONVERSATION).expect("conversation");
    (owned, LocalStore::new(paths), agent, conversation)
}

pub(crate) fn timestamp() -> Timestamp {
    Timestamp::parse_persisted_rfc3339("2026-08-15T01:02:03.456Z").expect("timestamp")
}

pub(crate) fn manifest() -> TranscriptManifest {
    TranscriptManifest {
        schema_version: 2,
        message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
        provider_stack: ProviderStack::PiAi,
        created_at: timestamp(),
        migrated_from: None,
        migrated_at: None,
        backup_path: None,
    }
}

pub(crate) fn session() -> TranscriptEntry {
    TranscriptEntry::Session(SessionEntry {
        entry_type: SessionEntryType::Session,
        version: 3,
        id: NonEmptyString::new("session-1").expect("session id"),
        timestamp: timestamp(),
        cwd: "/tmp/ponytail".into(),
    })
}

fn local_message(id: &str, text: &str) -> LocalMessage {
    LocalMessage {
        id: MessageId::accept(id).expect("message id"),
        role: LocalMessageRole::User,
        content: Some(
            BoundedJsonValue::new(json!([{"type":"text","text":text}])).expect("content"),
        ),
        timestamp: 1_776_214_923_456.25,
        metadata: None,
    }
}

pub(crate) fn message(id: &str, parent: Option<&str>, text: &str) -> TranscriptEntry {
    TranscriptEntry::Message(MessageEntry {
        entry_type: MessageEntryType::Message,
        id: NonEmptyString::new(id).expect("entry id"),
        parent_id: parent.map(str::to_owned),
        timestamp: timestamp(),
        message: local_message(&format!("ui-{id}"), text),
    })
}

pub(crate) fn compaction(summary: String) -> TranscriptEntry {
    TranscriptEntry::Compaction(CompactionEntry {
        entry_type: CompactionEntryType::Compaction,
        id: NonEmptyString::new("compact-boundary").expect("entry id"),
        parent_id: Some("entry-1".into()),
        timestamp: timestamp(),
        summary,
        first_kept_entry_id: Some("entry-1".into()),
        tokens_before: 42,
        message: local_message("ui-compact-boundary", "summary"),
        details: None,
    })
}

pub(crate) fn transcript_files(
    store: &LocalStore,
    agent: &AgentId,
    conversation: &ConversationId,
) -> (PathBuf, PathBuf) {
    let directory = store
        .paths()
        .conversation_dir(agent, conversation)
        .expect("conversation path");
    (
        directory.join("manifest.json"),
        directory.join("messages.jsonl"),
    )
}

pub(crate) fn root_file(label: &str, payload: usize, newline: bool) -> (TemporaryRoot, PathBuf) {
    let root = TemporaryRoot::new(label).expect("root");
    let backend = root.path().join("backend");
    std::fs::create_dir_all(&backend).expect("backend");
    let path = backend.join("messages.jsonl");
    let mut file = std::fs::File::create(&path).expect("file");
    let chunk = [b'x'; 8_192];
    let mut remaining = payload;
    while remaining != 0 {
        let count = remaining.min(chunk.len());
        file.write_all(&chunk[..count]).expect("payload");
        remaining -= count;
    }
    if newline {
        file.write_all(b"\n").expect("newline");
    }
    (root, path)
}
