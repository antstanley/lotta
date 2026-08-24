//! Shared fixtures for the conversation command-group certificate selectors.
//!
//! Every fixture drives the real bridge over one unique temporary storage
//! root laid out exactly like the canonical local backend: `agents/*.json`
//! records plus encoded conversation directories carrying
//! `conversation.json`, `manifest.json`, and `messages.jsonl`, so seeded
//! records and created artifacts are asserted through their production
//! locations.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use lotta_domain::{
    Agent, AgentId, BoundedMap, BoundedVec, Conversation, ConversationId, EntityExtras,
    LocalMessage, LocalMessageRole, MessageEntry, MessageEntryType, MessageId, NonEmptyString,
    ProviderStack, SessionEntry, SessionEntryType, Timestamp, TranscriptEntry, TranscriptManifest,
    TranscriptMessageFormat,
};
use lotta_runtime::ports::{AgentStore, ConversationStore};
use lotta_store::{
    LocalStore, StorePaths,
    query::{MessageListOptions, MessageOrder},
};
use lotta_testkit::clock::FakeClock;
use serde_json::{Value, json};

use super::{ConversationsBridge, ConversationsForwarder, ConversationsMessage};
use crate::{framing, ws::ConnectionId};

/// First test connection identity.
pub(super) const CONNECTION_A: ConnectionId = 71;

/// Fixed fixture instant served by the injected fake clock.
pub(super) const FIXTURE_TIMESTAMP_TEXT: &str = "2026-01-02T03:04:05Z";

type RecordedMessages = Arc<Mutex<Vec<(ConnectionId, ConversationsMessage)>>>;

static ROOT_ORDINAL: AtomicUsize = AtomicUsize::new(0);

fn fixture_timestamp() -> Timestamp {
    Timestamp::parse_persisted_rfc3339(FIXTURE_TIMESTAMP_TEXT).expect("fixture timestamp")
}

/// A bridge over one unique temporary storage root plus its recorder.
pub(super) struct TestConversations {
    /// Bridge under test; every command applies inline via [`Self::send`].
    pub(super) bridge: Arc<ConversationsBridge>,
    /// Canonical storage root handed to the bridge.
    pub(super) root: PathBuf,
    /// Direct store access for seeding and post-state assertions.
    pub(super) store: LocalStore,
    messages: RecordedMessages,
}

impl TestConversations {
    /// Decodes a raw JSON command through framing and applies it inline.
    pub(super) async fn send(&self, command: &Value) {
        let text = command.to_string();
        let frame = framing::decode_text(&text).expect("bounded conversations frame");
        let decoded = super::decode(&frame)
            .expect("wellformed conversations command")
            .expect("conversations command routed");
        self.bridge.apply(CONNECTION_A, &decoded).await;
    }

    /// Snapshot of every forwarded message for the fixture connection.
    pub(super) fn messages(&self) -> Vec<ConversationsMessage> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == CONNECTION_A)
            .map(|(_, message)| message.clone())
            .collect()
    }

    /// Encodes the most recent outbound message for field-level assertions.
    pub(super) fn last(&self) -> Value {
        let messages = self.messages();
        serde_json::to_value(messages.last().expect("at least one message")).expect("encodes")
    }

    /// Seeds one canonical agent record directly and returns its identifier.
    pub(super) async fn seed_agent(&self, suffix: &str, name: &str) -> AgentId {
        let agent = Agent {
            id: AgentId::accept(format!("agent-local-{suffix}")).expect("seeded agent id"),
            name: NonEmptyString::new(name.to_owned()).expect("seeded name"),
            description: None,
            system: format!("Base system prompt for {name}."),
            tags: BoundedVec::new(vec!["fixture".to_owned()]).expect("seeded tags"),
            model: NonEmptyString::new("letta/auto").expect("seeded model"),
            model_settings: BoundedMap::new(std::collections::BTreeMap::default())
                .expect("empty settings"),
            hidden: None,
            compaction_settings: None,
            extras: EntityExtras::default(),
        };
        AgentStore::save(&self.store, &agent)
            .await
            .expect("seeded agent save");
        agent.id
    }
    /// Seeds one conversation record plus `history` alternating user/assistant
    /// text messages, returning its identifier.
    pub(super) async fn seed_conversation(
        &self,
        agent: &AgentId,
        id_text: &str,
        history: usize,
    ) -> ConversationId {
        let conversation_id = ConversationId::accept(id_text).expect("seeded conversation id");
        let mut record = Self::blank_record(agent, &conversation_id);
        ConversationStore::save(&self.store, &record)
            .await
            .expect("seeded conversation save");
        self.store
            .initialize_transcript(
                agent,
                &conversation_id,
                &Self::manifest(),
                &TranscriptEntry::Session(SessionEntry {
                    entry_type: SessionEntryType::Session,
                    version: 3,
                    id: NonEmptyString::new(format!("session-{id_text}")).expect("session id"),
                    timestamp: fixture_timestamp(),
                    cwd: "/".to_owned(),
                }),
            )
            .await
            .expect("seeded transcript initialize");
        for index in 0..history {
            let message = Self::seed_message(&conversation_id, index);
            let source_id = message.id.clone();
            self.store
                .append_transcript_entry(
                    agent,
                    &conversation_id,
                    &TranscriptEntry::Message(MessageEntry {
                        entry_type: MessageEntryType::Message,
                        id: NonEmptyString::new(format!("entry-{id_text}-{index}"))
                            .expect("entry id"),
                        parent_id: None,
                        timestamp: fixture_timestamp(),
                        message,
                    }),
                )
                .await
                .expect("seeded message append");
            record = ConversationStore::load(&self.store, agent, &conversation_id)
                .await
                .expect("reload");
            let mut context = record.in_context_message_ids.as_slice().to_vec();
            context.push(source_id);
            record.in_context_message_ids = BoundedVec::new(context).expect("context push");
            ConversationStore::save(&self.store, &record)
                .await
                .expect("context save");
        }
        conversation_id
    }

    /// Ascending active-projection identifiers for one seeded conversation.
    pub(super) async fn projected_ids(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
    ) -> Vec<String> {
        self.store
            .query_messages_for_conversation(
                agent,
                conversation,
                MessageListOptions {
                    order: MessageOrder::Ascending,
                    ..MessageListOptions::default()
                },
            )
            .await
            .expect("projected messages")
            .into_iter()
            .map(|message| message.id.as_str().to_owned())
            .collect()
    }

    /// Canonical scoped transcript path.
    pub(super) fn transcript_path(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
    ) -> PathBuf {
        self.store
            .paths()
            .conversation_dir(agent, conversation)
            .expect("scoped path")
            .join("messages.jsonl")
    }

    /// Count of persisted compaction rows in one transcript.
    pub(super) fn compaction_rows(&self, agent: &AgentId, conversation: &ConversationId) -> usize {
        let path = self.transcript_path(agent, conversation);
        let bytes = std::fs::read(&path).expect("transcript bytes");
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|row| !row.is_empty())
            .filter_map(|row| serde_json::from_slice::<Value>(row).ok())
            .filter(|row| row.get("type").and_then(Value::as_str) == Some("compaction"))
            .count()
    }

    /// Persisted conversation record rendered as JSON.
    pub(super) async fn conversation_value(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
    ) -> Value {
        let record =
            lotta_runtime::ports::ConversationStore::load(&self.store, agent, conversation)
                .await
                .expect("conversation record");
        serde_json::to_value(&record).expect("encodes")
    }

    /// Durable Task 58 compaction journal document.
    pub(super) fn compaction_journal(&self) -> Value {
        let path = self.store.paths().runtime().join("compactions.json");
        let bytes = std::fs::read(path).expect("compaction journal");
        serde_json::from_slice(&bytes).expect("journal encodes")
    }

    fn blank_record(agent: &AgentId, conversation_id: &ConversationId) -> Conversation {
        let now = fixture_timestamp();
        Conversation {
            id: conversation_id.clone(),
            agent_id: agent.clone(),
            archived: false,
            archived_at: Some(None),
            created_at: now,
            updated_at: now,
            last_message_at: Some(None),
            summary: Some(None),
            in_context_message_ids: BoundedVec::new(Vec::new()).expect("empty context"),
            model: None,
            model_settings: None,
            context_window_limit: None,
            hidden: None,
            tags: None,
            extras: EntityExtras::default(),
        }
    }

    fn manifest() -> TranscriptManifest {
        TranscriptManifest {
            schema_version: 2,
            message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
            provider_stack: ProviderStack::PiAi,
            created_at: fixture_timestamp(),
            migrated_from: None,
            migrated_at: None,
            backup_path: None,
        }
    }

    fn seed_message(conversation: &ConversationId, index: usize) -> LocalMessage {
        let role = if index.is_multiple_of(2) {
            LocalMessageRole::User
        } else {
            LocalMessageRole::Assistant
        };
        LocalMessage {
            id: MessageId::accept(format!("ui-msg-{}-{index}", conversation.as_str()))
                .expect("seeded message id"),
            role,
            content: Some(
                lotta_domain::BoundedJsonValue::new(json!([
                    {"type": "text", "text": format!("seed message {index}")}
                ]))
                .expect("seeded content"),
            ),
            timestamp: 1_000.0 * f64::from(u32::try_from(index).expect("seed index") + 1),
            metadata: None,
            extras: EntityExtras::default(),
        }
    }
}

/// Creates a bridge over a fresh temporary storage root (production cap).
pub(super) fn bridge() -> TestConversations {
    bridge_with_authority(|_| None)
}

/// Creates a bridge over a fresh temporary root whose lease authority is
/// composed from the fixture's own store handle, so an authority can share
/// exactly the storage the bridge serves.
pub(super) fn bridge_with_authority(
    compose_authority: impl FnOnce(LocalStore) -> Option<Arc<dyn super::ConversationAuthority>>,
) -> TestConversations {
    let ordinal = ROOT_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let parent = std::env::temp_dir().join(format!(
        "lotta-conversations-ws-{ordinal}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    let root = parent.canonicalize().expect("canonical root");
    let paths = StorePaths::new(root.clone()).expect("fixture store paths");
    let memfs = lotta_memfs::GitMemFs::new(root.clone()).expect("fixture memfs backend");
    let clock: Arc<dyn lotta_domain::Clock + Send + Sync> =
        Arc::new(FakeClock::new(fixture_timestamp()));
    let messages: RecordedMessages = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: ConversationsForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let authority = compose_authority(LocalStore::new(paths.clone()));
    let bridge = Arc::new(ConversationsBridge::compose(
        forward,
        LocalStore::new(paths.clone()),
        memfs,
        clock,
        authority,
    ));
    TestConversations {
        bridge,
        root,
        store: LocalStore::new(paths),
        messages,
    }
}

/// Outbound discriminant of one conversations group message.
pub(super) fn discriminant_of(message: &ConversationsMessage) -> &'static str {
    match message {
        ConversationsMessage::List(_) => "conversation_list_response",
        ConversationsMessage::Retrieve(_) => "conversation_retrieve_response",
        ConversationsMessage::Create(_) => "conversation_create_response",
        ConversationsMessage::Update(_) => "conversation_update_response",
        ConversationsMessage::Recompile(_) => "conversation_recompile_response",
        ConversationsMessage::Fork(_) => "conversation_fork_response",
        ConversationsMessage::MessagesList(_) => "conversation_messages_list_response",
        ConversationsMessage::Compact(_) => "conversation_compact_response",
    }
}
