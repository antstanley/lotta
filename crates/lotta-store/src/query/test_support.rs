use crate::LocalStore;
use lotta_domain::{
    Agent, AgentId, BoundedJsonValue, BoundedMap, BoundedVec, Conversation, ConversationId,
    EntityExtras, LocalMessage, LocalMessageRole, MessageEntry, MessageEntryType, MessageId,
    NonEmptyString, ProviderStack, SessionEntry, SessionEntryType, Timestamp, TranscriptEntry,
    TranscriptManifest, TranscriptMessageFormat,
};
use lotta_runtime::ports::{AgentStore, ConversationStore};
use lotta_testkit::roots::TemporaryRoot;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) struct Fixture {
    pub(crate) root: TemporaryRoot,
    pub(crate) store: LocalStore,
    pub(crate) agents: [AgentId; 2],
    pub(crate) conversations: [ConversationId; 3],
}

pub(crate) async fn fixture(label: &str) -> Fixture {
    let root = TemporaryRoot::new(label).expect("temporary root");
    let paths = crate::StorePaths::new(root.path().join("backend")).expect("paths");
    std::fs::create_dir_all(paths.agents()).expect("agents");
    std::fs::create_dir_all(paths.conversations()).expect("conversations");
    let store = LocalStore::new(paths);
    let agents = [agent_id("agent-local-a"), agent_id("agent-local-b")];
    for (index, id) in agents.iter().enumerate() {
        AgentStore::save(&store, &agent(id.clone(), index))
            .await
            .expect("save agent");
    }
    let conversations = [
        conversation_id("default"),
        conversation_id("same-a"),
        conversation_id("z"),
    ];
    save_conversation(&store, &agents[0], &conversations[0], false, false, 1).await;
    save_conversation(&store, &agents[1], &conversations[0], false, false, 1).await;
    save_conversation(&store, &agents[0], &conversations[1], false, true, 2).await;
    save_conversation(&store, &agents[0], &conversations[2], false, false, 2).await;
    seed_transcript(&store, &agents[0], &conversations[0], "a", 1_000.0, 10_001).await;
    seed_transcript(&store, &agents[0], &conversations[1], "b", 1_000.0, 20_001).await;
    seed_transcript(&store, &agents[0], &conversations[2], "c", 2_000.0, 30_001).await;
    seed_transcript(
        &store,
        &agents[1],
        &conversations[0],
        "isolated",
        3_000.0,
        40_001,
    )
    .await;
    Fixture {
        root,
        store,
        agents,
        conversations,
    }
}

fn agent(id: AgentId, index: usize) -> Agent {
    Agent {
        id,
        name: NonEmptyString::new(if index == 0 {
            "Alpha Pony"
        } else {
            "alpha pony"
        })
        .expect("name"),
        description: Some(Some(
            if index == 0 {
                "OpenAI red"
            } else {
                "OpenAI blue"
            }
            .into(),
        )),
        system: "system".into(),
        tags: BoundedVec::new(if index == 0 {
            vec!["x".into(), "common".into()]
        } else {
            vec!["common".into()]
        })
        .expect("tags"),
        model: NonEmptyString::new("openai/gpt-5").expect("model"),
        model_settings: BoundedMap::default(),
        hidden: Some(Some(index == 1)),
        compaction_settings: None,
        extras: EntityExtras::default(),
    }
}

pub(crate) async fn save_conversation(
    store: &LocalStore,
    agent: &AgentId,
    id: &ConversationId,
    archived: bool,
    hidden: bool,
    second: u32,
) {
    let timestamp = time(second);
    let conversation = Conversation {
        id: id.clone(),
        agent_id: agent.clone(),
        archived,
        archived_at: None,
        created_at: timestamp,
        updated_at: timestamp,
        last_message_at: Some(Some(timestamp)),
        summary: Some(Some(format!("summary-{}", id.as_str()))),
        in_context_message_ids: BoundedVec::new(Vec::new()).expect("empty context"),
        model: None,
        model_settings: None,
        context_window_limit: None,
        hidden: Some(hidden),
        tags: Some(
            BoundedVec::new(if hidden {
                vec!["hidden".into()]
            } else {
                vec!["common".into()]
            })
            .expect("tags"),
        ),
        extras: EntityExtras::default(),
    };
    ConversationStore::save(store, &conversation)
        .await
        .expect("save conversation");
}

async fn seed_transcript(
    store: &LocalStore,
    agent: &AgentId,
    conversation: &ConversationId,
    text: &str,
    timestamp: f64,
    source_sequence: u64,
) {
    store
        .initialize_transcript(agent, conversation, &manifest(), &session(conversation))
        .await
        .expect("initialize transcript");
    let first = local_message(
        &format!("ui-msg-{source_sequence}"),
        LocalMessageRole::User,
        json!([{"type":"text","text":format!("needle {text}")}]),
        timestamp,
    );
    let second = local_message(
        &format!("ui-msg-{}", source_sequence + 1),
        LocalMessageRole::Assistant,
        json!([
            {"type":"thinking","thinking":format!("reason {text}")},
            {"type":"text","text":format!("answer {text}")},
            {
                "type":"toolCall",
                "id":format!("call-{text}"),
                "name":"read",
                "arguments":{"path":"safe"}
            }
        ]),
        timestamp,
    );
    for (index, message) in [first, second].into_iter().enumerate() {
        let source_id = message.id.clone();
        let entry = TranscriptEntry::Message(MessageEntry {
            entry_type: MessageEntryType::Message,
            id: NonEmptyString::new(format!("entry-{text}-{index}")).expect("entry"),
            parent_id: None,
            timestamp: time(u32::try_from(index + 3).expect("small")),
            message,
        });
        store
            .append_transcript_entry(agent, conversation, &entry)
            .await
            .expect("append");
        let mut record = store
            .query_conversation(agent, conversation)
            .await
            .expect("conversation");
        let mut context = record.in_context_message_ids.as_slice().to_vec();
        context.push(source_id);
        record.in_context_message_ids = BoundedVec::new(context).expect("context push");
        ConversationStore::save(store, &record)
            .await
            .expect("save context");
    }
}

pub(crate) fn projection_message(id: &str, projections: usize, timestamp: f64) -> LocalMessage {
    let parts = (0..projections)
        .map(|index| {
            if index % 2 == 0 {
                json!({"type":"text","text":format!("text-{index}")})
            } else {
                json!({"type":"thinking","thinking":format!("thinking-{index}")})
            }
        })
        .collect();
    local_message(
        id,
        LocalMessageRole::Assistant,
        serde_json::Value::Array(parts),
        timestamp,
    )
}

pub(crate) fn transcript_path(fixture: &Fixture, conversation: &ConversationId) -> PathBuf {
    fixture
        .store
        .paths()
        .conversation_dir(&fixture.agents[0], conversation)
        .expect("conversation dir")
        .join("messages.jsonl")
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct FileSnapshot {
    pub(crate) bytes: Vec<u8>,
    pub(crate) sha256: [u8; 32],
    pub(crate) len: u64,
    pub(crate) modified: SystemTime,
    #[cfg(unix)]
    pub(crate) inode: u64,
}

pub(crate) fn file_snapshot(path: &Path) -> FileSnapshot {
    let bytes = std::fs::read(path).expect("snapshot bytes");
    let metadata = std::fs::metadata(path).expect("snapshot metadata");
    FileSnapshot {
        sha256: Sha256::digest(&bytes).into(),
        len: metadata.len(),
        modified: metadata.modified().expect("modified"),
        #[cfg(unix)]
        inode: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.ino()
        },
        bytes,
    }
}

pub(crate) fn projected_number(id: &MessageId) -> u64 {
    id.as_str()
        .strip_prefix("letta-msg-")
        .expect("projected prefix")
        .parse()
        .expect("projected number")
}

fn local_message(
    id: &str,
    role: LocalMessageRole,
    content: serde_json::Value,
    timestamp: f64,
) -> LocalMessage {
    LocalMessage {
        id: message_id(id),
        role,
        content: Some(BoundedJsonValue::new(content).expect("content")),
        timestamp,
        metadata: None,
        extras: EntityExtras::default(),
    }
}

fn manifest() -> TranscriptManifest {
    TranscriptManifest {
        schema_version: 2,
        message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
        provider_stack: ProviderStack::PiAi,
        created_at: time(0),
        migrated_from: None,
        migrated_at: None,
        backup_path: None,
    }
}

fn session(conversation: &ConversationId) -> TranscriptEntry {
    TranscriptEntry::Session(SessionEntry {
        entry_type: SessionEntryType::Session,
        version: 3,
        id: NonEmptyString::new(format!("session-{}", conversation.as_str())).expect("session"),
        timestamp: time(0),
        cwd: "/tmp".into(),
    })
}

pub(crate) fn time(second: u32) -> Timestamp {
    Timestamp::parse_persisted_rfc3339(&format!("2026-01-01T00:00:{second:02}Z")).expect("time")
}

pub(crate) fn agent_id(value: &str) -> AgentId {
    AgentId::accept(value).expect("agent")
}

pub(crate) fn conversation_id(value: &str) -> ConversationId {
    ConversationId::accept(value).expect("conversation")
}

pub(crate) fn message_id(value: &str) -> MessageId {
    MessageId::accept(value).expect("message")
}
