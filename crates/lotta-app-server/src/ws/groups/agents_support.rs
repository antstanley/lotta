//! Shared fixtures for the agent command-group certificate selectors.
//!
//! Every fixture drives the real bridge over one unique temporary storage
//! root laid out exactly like the canonical local backend: `agents/*.json`
//! records, encoded conversation directories, and `memfs/<agent>/memory`
//! repositories, so seeded records and created artifacts are asserted
//! through their production locations.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use lotta_domain::{
    Agent, AgentId, BoundedMap, BoundedVec, ConversationId, EntityExtras, NonEmptyString, Timestamp,
};
use lotta_memfs::CacheRoot;
use lotta_store::{ConversationKey, LocalStore, StorePaths};
use lotta_testkit::clock::FakeClock;
use serde_json::{Value, json};

use super::{AgentsBridge, AgentsForwarder, AgentsMessage};
use crate::{framing, ws::ConnectionId};

/// First test connection identity.
pub(super) const CONNECTION_A: ConnectionId = 71;

/// Fixed fixture instant served by the injected fake clock.
pub(super) const FIXTURE_TIMESTAMP_TEXT: &str = "2026-01-02T03:04:05Z";

type RecordedMessages = Arc<Mutex<Vec<(ConnectionId, AgentsMessage)>>>;

static ROOT_ORDINAL: AtomicUsize = AtomicUsize::new(0);

fn fixture_timestamp() -> Timestamp {
    Timestamp::parse_persisted_rfc3339(FIXTURE_TIMESTAMP_TEXT).expect("fixture timestamp")
}

/// A bridge over one unique temporary storage root plus its recorder.
pub(super) struct TestAgents {
    /// Bridge under test; every command applies inline via [`Self::send`].
    pub(super) bridge: Arc<AgentsBridge>,
    /// Canonical storage root handed to the bridge.
    pub(super) root: PathBuf,
    messages: RecordedMessages,
}

impl TestAgents {
    /// Canonical agents record directory.
    pub(super) fn agents_dir(&self) -> PathBuf {
        self.root.join("agents")
    }

    /// Absolute agent `MemFS` repository directory.
    pub(super) fn memory_root(&self, agent_id: &str) -> PathBuf {
        self.root.join("memfs").join(agent_id).join("memory")
    }

    /// Directory of the default conversation prompt cache for one agent.
    pub(super) fn default_conversation_dir(&self, agent_id: &str) -> PathBuf {
        let key = ConversationKey::from_ids(
            &AgentId::accept(agent_id.to_owned()).expect("fixture agent id"),
            &ConversationId::default_for_agent(),
        );
        self.root
            .join("conversations")
            .join(key.encoded().expect("fixture key"))
    }

    /// Loads the persisted default-conversation prompt record for one agent.
    pub(super) fn load_prompt_record(
        &self,
        agent_id: &str,
    ) -> Option<lotta_memfs::CompiledPromptRecord> {
        let cache =
            CacheRoot::new(&self.default_conversation_dir(agent_id)).expect("prompt cache root");
        cache.load().expect("prompt cache read")
    }

    /// Whether the pinned-agent side store currently lists one identifier.
    pub(super) fn is_pinned(&self, agent_id: &str) -> bool {
        let bytes = match lotta_store::side::pinned::read(
            &lotta_store::SidePaths::new(self.root.clone(), None, []).expect("fixture side paths"),
        ) {
            Ok(file) => file.bytes().to_vec(),
            Err(_) => return false,
        };
        let document: Value = serde_json::from_slice(&bytes).expect("pinned document");
        document["agents"]
            .as_array()
            .expect("pinned list")
            .iter()
            .any(|entry| entry.as_str() == Some(agent_id))
    }

    /// Decodes a raw JSON command through framing and applies it inline.
    pub(super) async fn send(&self, command: &Value) {
        let text = command.to_string();
        let frame = framing::decode_text(&text).expect("bounded agents frame");
        let decoded = super::decode(&frame)
            .expect("wellformed agents command")
            .expect("agents command routed");
        self.bridge.apply(CONNECTION_A, &decoded).await;
    }

    /// Snapshot of every forwarded message for the fixture connection.
    pub(super) fn messages(&self) -> Vec<AgentsMessage> {
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

    /// Creates one agent through the full wire path and returns its id.
    pub(super) async fn create_agent(&self, name: &str) -> String {
        self.send(&json!({
            "type": "agent_create",
            "request_id": "fixture-create",
            "body": {"name": name},
        }))
        .await;
        self.last()["agent"]["id"]
            .as_str()
            .expect("created agent id")
            .to_owned()
    }
}

/// One seeded canonical agent record (fixture options bundle).
#[derive(Clone, Copy)]
pub(super) struct SeedAgent<'a> {
    /// Identifier suffix producing `agent-local-<suffix>`.
    pub(super) suffix: &'a str,
    /// Display name.
    pub(super) name: &'a str,
    /// Creation tags.
    pub(super) tags: &'a [&'a str],
    /// Hidden flag.
    pub(super) hidden: bool,
    /// Description.
    pub(super) description: &'a str,
    /// Model handle.
    pub(super) model: &'a str,
}

/// Writes one canonical agent record directly into the storage layout.
///
/// Seeding bypasses creation so cap and listing fixtures control the exact
/// on-disk population without running the `MemFS` side effects per record.
pub(super) fn seed_agent_record(root: &Path, seed: SeedAgent<'_>) -> String {
    let id_text = format!("agent-local-{}", seed.suffix);
    let agent = Agent {
        id: AgentId::accept(id_text.clone()).expect("seeded agent id"),
        name: NonEmptyString::new(seed.name.to_owned()).expect("seeded name"),
        description: Some(Some(seed.description.to_owned())),
        system: format!("System prompt for {}.", seed.name),
        tags: BoundedVec::new(seed.tags.iter().map(|tag| (*tag).to_owned()).collect())
            .expect("seeded tags"),
        model: NonEmptyString::new(seed.model.to_owned()).expect("seeded model"),
        model_settings: BoundedMap::new(BTreeMap::new()).expect("empty settings"),
        hidden: Some(Some(seed.hidden)),
        compaction_settings: None,
        extras: EntityExtras::default(),
    };
    std::fs::create_dir_all(root.join("agents")).expect("agents directory");
    // Records live under their canonical encoded segment, exactly where the
    // store writes and scans them.
    let paths = StorePaths::new(root).expect("fixture store paths");
    let path = paths
        .agent_record(&AgentId::accept(id_text.clone()).expect("seeded id"))
        .expect("seeded record path");
    let bytes = serde_json::to_vec_pretty(&agent).expect("agent record encodes");
    std::fs::write(path, bytes).expect("agent record write");
    id_text
}

/// Creates a bridge over a fresh temporary storage root with the given cap.
fn build(root: &Path, forward: AgentsForwarder, agents_max: usize) -> AgentsBridge {
    let paths = StorePaths::new(root).expect("fixture store paths");
    let memfs = lotta_memfs::GitMemFs::new(root.to_path_buf()).expect("fixture memfs backend");
    let side_paths =
        lotta_store::SidePaths::new(root.to_path_buf(), None, []).expect("fixture side paths");
    let clock: Arc<dyn lotta_domain::Clock + Send + Sync> =
        Arc::new(FakeClock::new(fixture_timestamp()));
    AgentsBridge::compose(
        forward,
        LocalStore::new(paths),
        memfs,
        root.to_path_buf(),
        side_paths,
        agents_max,
        clock,
    )
}

/// Creates a bridge over a fresh temporary storage root (production cap).
pub(super) fn bridge() -> TestAgents {
    bridge_with_limit(lotta_store::AGENTS_MAX)
}

/// Creates a bridge over a fresh temporary root with an explicit small cap.
pub(super) fn bridge_with_limit(agents_max: usize) -> TestAgents {
    let ordinal = ROOT_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let parent =
        std::env::temp_dir().join(format!("lotta-agents-ws-{}-{ordinal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    let root = parent.canonicalize().expect("canonical root");
    std::fs::create_dir_all(root.join("memfs")).expect("memfs backend root");
    let messages: RecordedMessages = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: AgentsForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let bridge = Arc::new(build(&root, forward, agents_max));
    TestAgents {
        bridge,
        root,
        messages,
    }
}

/// Outbound discriminant of one agents group message.
#[must_use]
pub(super) fn discriminant_of(message: &AgentsMessage) -> &'static str {
    match message {
        AgentsMessage::CreateShortcut(_) => "create_agent_response",
        AgentsMessage::List(_) => "agent_list_response",
        AgentsMessage::Retrieve(_) => "agent_retrieve_response",
        AgentsMessage::Create(_) => "agent_create_response",
        AgentsMessage::Update(_) => "agent_update_response",
        AgentsMessage::Delete(_) => "agent_delete_response",
    }
}
