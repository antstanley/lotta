//! WebSocket agent management command group.
//!
//! Decodes and routes the pinned `create_agent`, `agent_list`,
//! `agent_retrieve`, `agent_create`, `agent_update`, and `agent_delete`
//! commands (the §WebSocket command groups Agent management row: create
//! shortcut plus list/retrieve/create/update/delete). Responses mirror the
//! pinned shapes exactly: correlated `request_id`, a `success` flag, and the
//! full agent snapshot — or its identifier on delete — so every mutating
//! command's response is the authoritative post-mutation state, never a diff.
//!
//! Creation with local MemFS completes all four §Agent side effects before
//! the success frame is emitted: the `git-memory-enabled` tag is stamped on
//! the agent, memory files are initialized from `memory_blocks`, the Task 29
//! repository is created, and the default conversation prompt is compiled and
//! persisted through the Task 54 prompt cache. The handler awaits every step,
//! so a client that observed the success response would always find all four
//! artifacts already in place. Deletion removes them again — including the
//! prompt-only default-conversation cache directory, which carries no
//! conversation record and therefore needs explicit removal.
//!
//! The `create_agent` shortcut mirrors the pinned command end to end:
//! requested models resolve against the embedded canonical catalog
//! (`agents_presets`) before any side effect runs and unknown identifiers
//! reject with the pinned detail, preset agents receive the canonical system
//! prompt, memory-block assets, descriptions, and origin/personality tags,
//! and the created agent is pinned in the pinned-agent side store by default
//! before the response is emitted (`pin_global: false` opts out).
//!
//! Listing serves the Task 28 query behaviors: deterministic identifier
//! ordering with name/query/tag/hidden filters compatible with the pinned
//! local backend, and `after` continuation through the Task 28 item cursor;
//! an absent or unknown cursor starts from the first page like the
//! baseline's slice semantics. Identifier lookup goes through
//! [`LocalStore::query_agent`](lotta_store::LocalStore), so an absent or
//! wrong-prefix id answers one safe 404-class failure. `AGENTS_MAX` bounds
//! creation; the cap is checked before any side effect runs.
//!
//! Baseline degradations, kept honest: creation-time compilation renders no
//! skill sections, the pinned-agent side store records one global namespace
//! rather than per-server keys, and the list response carries no cursor
//! field because the pinned response shape has none.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use lotta_domain::bounds::UNBOUNDED_MAP_FIELDS_MAX;
use lotta_domain::{
    Agent, AgentId, BoundedMap, BoundedVec, Clock, ConversationId, EntityExtras, MemoryBlockInput,
    NonEmptyString,
};
use lotta_memfs::{
    CacheRoot, DeliveryCapability, GitMemFs, PromptCompiler, PromptInputs, PromptSections,
    PromptText,
};
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::{
    boundary::{InitialMemoryBlock, InitialMemoryBlocks},
    ports::{AgentStore as _, MemFsPort},
};
use lotta_store::{
    AGENTS_MAX, LocalStore, SidePaths, StoreErrorKind, StorePaths,
    query::{
        AgentFilters, ConversationFilters, Cursor, NameMatch, PageRequest, QUERY_PAGE_ITEMS_MAX,
        TriState,
    },
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    bounds::WS_FRAME_BYTES_MAX,
    errors::{AppServerError, ProtocolErrorEnvelope},
    framing::DecodedFrame,
    ws::connection::ConnectionId,
};

/// Pinned tag marking agents whose Git memory repository is managed locally.
pub const GIT_MEMORY_ENABLED_TAG: &str = "git-memory-enabled";
/// Agents listed when the client omits an explicit limit (pinned parity).
pub const AGENT_LIST_DEFAULT_ITEMS: usize = 20;
/// Model stored when a raw `agent_create` body omits one.
pub const DEFAULT_PERSONALITY_MODEL: &str = "auto-chat";
/// Scrubbed 404-class detail shared by absent and wrong-prefix lookups.
const AGENT_NOT_FOUND: &str = "agent not found";
/// Scrubbed rejection detail for failed creations.
const CREATE_FAILURE: &str = "Failed to create agent";
/// Scrubbed rejection detail for failed listings.
const LIST_FAILURE: &str = "Failed to list agents";
/// Scrubbed rejection detail for failed retrievals.
const RETRIEVE_FAILURE: &str = "Failed to retrieve agent";
/// Scrubbed rejection detail for failed updates.
const UPDATE_FAILURE: &str = "Failed to update agent";
/// Scrubbed rejection detail for failed deletions.
const DELETE_FAILURE: &str = "Failed to delete agent";
/// Scrubbed rejection detail when creation would exceed `AGENTS_MAX`.
const AGENT_LIMIT_REACHED: &str = "agent limit reached";
/// Pinned settings-document key holding pinned agent identifiers.
const PINNED_AGENTS_KEY: &str = "agents";
/// Bounded CAS retries when the pinned-agent side store changes concurrently.
const PIN_WRITE_ATTEMPTS: usize = 4;
/// Pause between pinned-document retry attempts so a contended storage lock
/// or a lost create race clears before the next bounded attempt.
const PIN_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(25);
/// Serializes in-process pinned-document writers so concurrent creations do
/// not stampede the storage file lock; cross-process races stay covered by
/// the side store's compare-and-swap writes.
static PIN_WRITE_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// Compaction keys the pinned local backend recognizes.
const COMPACTION_SETTING_KEYS: [&str; 4] =
    ["mode", "prompt", "clip_chars", "sliding_window_percentage"];
/// Compaction modes accepted by the pinned local backend validator.
const COMPACTION_MODES: [&str; 2] = ["all", "sliding_window"];
/// Rejection detail mirroring the pinned local validator message.
const COMPACTION_MODE_REJECTED: &str =
    "Local backend compaction currently supports only modes \"all\" and \"sliding_window\"";

// ── Wire commands ───────────────────────────────────────────────────────────

/// Personality recognized by the pinned `create_agent` shortcut.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PersonalityId {
    /// The memory-first default agent.
    Memo,
    /// Guided onboarding assistant.
    Tutorial,
    /// Empty starter personality.
    Blank,
    /// Blunt reviewer persona.
    Linus,
    /// Playful companion persona.
    Kawaii,
}

/// Pinned `create_agent` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateAgentCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Built-in personality preset to create.
    pub personality: PersonalityId,
    /// Optional model override resolved against the canonical catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Additional tags appended after the preset tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Whether to pin the created agent globally; defaults to true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_global: Option<bool>,
}

/// Explicit JSON field state preserving the absent/null/value contract.
///
/// The wire field is an outer option that distinguishes an omitted key from
/// both sent states; [`ExplicitField::Null`] is a key sent as JSON null and
/// [`ExplicitField::Value`] is a key carrying a value.
///
/// Tri-state fields decode through the `explicit_field` deserializer helper
/// because the plain `Option` treatment would collapse a sent `null` into
/// absence.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ExplicitField<T> {
    /// The key was sent as JSON null.
    Null,
    /// The key carried a value.
    Value(T),
}

/// Decodes one sent tri-state key, keeping an explicit JSON null distinct
/// from an omitted one.
pub(crate) fn explicit_field<'de, D, T>(
    deserializer: D,
) -> Result<Option<ExplicitField<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let sent = Value::deserialize(deserializer)?;
    if sent.is_null() {
        return Ok(Some(ExplicitField::Null));
    }
    T::deserialize(sent)
        .map(|value| Some(ExplicitField::Value(value)))
        .map_err(serde::de::Error::custom)
}

/// Decodes one sent tri-state compaction key with the pinned local-backend
/// coercion: an explicit JSON null stays distinct from absence, a JSON
/// object carries the record, and every other value is treated as
/// undefined (the key reads as absent instead of rejecting the command).
fn explicit_compaction_field<'de, D>(
    deserializer: D,
) -> Result<Option<ExplicitField<BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let sent = Value::deserialize(deserializer)?;
    if sent.is_null() {
        return Ok(Some(ExplicitField::Null));
    }
    if !sent.is_object() {
        return Ok(None);
    }
    BoundedMap::<{ UNBOUNDED_MAP_FIELDS_MAX }>::deserialize(sent)
        .map(|value| Some(ExplicitField::Value(value)))
        .map_err(serde::de::Error::custom)
}

/// Filter set of the pinned `agent_list` payload.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentListQuery {
    /// Case-insensitive substring filter over the display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Case-insensitive substring over name, description, id, and model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_text: Option<String>,
    /// Every listed agent must carry all of these tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Hidden-state selector; absence excludes hidden agents like the baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// Maximum returned agents after the Task 28 deterministic ordering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Continue after this agent identifier (Task 28 cursor continuation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// Pinned `agent_list` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentListCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Optional filter set; absence lists visible agents with the default cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<AgentListQuery>,
}

/// Pinned `agent_retrieve` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentRetrieveCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Agent to read.
    pub agent_id: String,
}

/// Body of the pinned `agent_create` payload.
///
/// Field names follow the canonical `Agent` schema; `description`,
/// `hidden`, and `compaction_settings` preserve the absent/null/value
/// three-state contract through [`ExplicitField`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentCreateBody {
    /// Display name.
    pub name: String,
    /// Model handle; defaults to [`DEFAULT_PERSONALITY_MODEL`] when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// System-prompt source; defaults to empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Optional description with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "explicit_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<ExplicitField<String>>,
    /// Creation tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Provider model settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>,
    /// Optional hidden flag with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "explicit_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub hidden: Option<ExplicitField<bool>>,
    /// Optional compaction settings record with explicit-null preservation,
    /// non-object values read as absent, and records validated against the
    /// pinned local modes before persistence.
    #[serde(
        default,
        deserialize_with = "explicit_compaction_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub compaction_settings: Option<ExplicitField<BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>>,
    /// Initial memory files rendered into the created repository.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_blocks: Vec<MemoryBlockInput>,
}

/// Pinned `agent_create` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentCreateCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Body forwarded to the canonical agent store.
    pub body: AgentCreateBody,
}

/// Body of the pinned `agent_update` payload; absent fields stay unchanged.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentUpdateBody {
    /// Replacement display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Replacement description; explicit null clears it.
    #[serde(
        default,
        deserialize_with = "explicit_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<ExplicitField<String>>,
    /// Replacement system-prompt source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Replacement tag list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Replacement model handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Replacement provider model settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>,
    /// Replacement hidden flag; explicit null clears it.
    #[serde(
        default,
        deserialize_with = "explicit_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub hidden: Option<ExplicitField<bool>>,
    /// Replacement compaction settings; explicit null clears them, a
    /// non-object value reads as absent, and a record replaces the stored
    /// value once it validates.
    #[serde(
        default,
        deserialize_with = "explicit_compaction_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub compaction_settings: Option<ExplicitField<BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>>,
}

/// Pinned `agent_update` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentUpdateCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Agent to replace fields on.
    pub agent_id: String,
    /// Validated replacement body.
    pub body: AgentUpdateBody,
}

/// Pinned `agent_delete` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentDeleteCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Agent to remove together with its `MemFS` artifacts.
    pub agent_id: String,
}

/// The six concrete agent management commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum AgentsCommand {
    /// Personality shortcut creation.
    #[serde(rename = "create_agent")]
    CreateShortcut(CreateAgentCommand),
    /// Deterministic filtered listing.
    #[serde(rename = "agent_list")]
    List(AgentListCommand),
    /// One agent snapshot by identifier.
    #[serde(rename = "agent_retrieve")]
    Retrieve(AgentRetrieveCommand),
    /// Full creation with local `MemFS` side effects.
    #[serde(rename = "agent_create")]
    Create(AgentCreateCommand),
    /// Field replacement by identifier.
    #[serde(rename = "agent_update")]
    Update(AgentUpdateCommand),
    /// Removal by identifier.
    #[serde(rename = "agent_delete")]
    Delete(AgentDeleteCommand),
}

// ── Wire responses ──────────────────────────────────────────────────────────

/// Pinned `create_agent_response` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateAgentResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Created agent identifier, present only on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Created display name, present only on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Created model handle, present only on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `agent_list_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct AgentListResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Matching agents in deterministic order.
    pub agents: Vec<Agent>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `agent_retrieve_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct AgentRetrieveResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Full snapshot, or null on failure like the pinned listener.
    pub agent: Option<Agent>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `agent_create_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct AgentCreateResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Created full snapshot, or null on failure.
    pub agent: Option<Agent>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `agent_update_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct AgentUpdateResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Replaced full snapshot, or null on failure.
    pub agent: Option<Agent>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `agent_delete_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct AgentDeleteResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Deleted identifier echoed back.
    pub agent_id: String,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Every outbound frame this group emits.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum AgentsMessage {
    /// Shortcut creation result.
    #[serde(rename = "create_agent_response")]
    CreateShortcut(CreateAgentResponseMessage),
    /// Listing result.
    #[serde(rename = "agent_list_response")]
    List(AgentListResponseMessage),
    /// Retrieval result.
    #[serde(rename = "agent_retrieve_response")]
    Retrieve(AgentRetrieveResponseMessage),
    /// Creation result.
    #[serde(rename = "agent_create_response")]
    Create(AgentCreateResponseMessage),
    /// Update result.
    #[serde(rename = "agent_update_response")]
    Update(AgentUpdateResponseMessage),
    /// Deletion result.
    #[serde(rename = "agent_delete_response")]
    Delete(AgentDeleteResponseMessage),
}

/// Push callback delivering one outbound message to one connection.
pub type AgentsForwarder =
    Arc<dyn Fn(ConnectionId, AgentsMessage) -> Result<(), AppServerError> + Send + Sync>;

/// Outcome class of one identifier lookup through the Task 28 boundary.
enum Lookup {
    /// The record exists.
    Found(Box<Agent>),
    /// Absent or wrong prefix; both map to one safe 404-class detail.
    Missing,
    /// Storage failed without exposing detail.
    Failed,
}

/// Applies wire commands to the Task 23 agent store and Task 29 repositories.
pub struct AgentsBridge {
    store: LocalStore,
    memfs: GitMemFs,
    backend_root: PathBuf,
    side_paths: SidePaths,
    agents_max: usize,
    forward: AgentsForwarder,
    clock: Arc<dyn Clock + Send + Sync>,
}

impl AgentsBridge {
    /// Creates the production bridge over one canonical local storage root.
    ///
    /// # Errors
    /// Returns [`AppServerError::Config`] when the storage root cannot be
    /// prepared for the store or the `MemFS` backend.
    pub fn new(
        forward: AgentsForwarder,
        storage_dir: &std::path::Path,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, AppServerError> {
        let (store, memfs, backend_root) = prepare_backend(storage_dir)?;
        let side_paths = SidePaths::new(backend_root.clone(), None, [])
            .map_err(|_| AppServerError::Config("agents backend unavailable"))?;
        Ok(Self {
            store,
            memfs,
            backend_root,
            side_paths,
            agents_max: AGENTS_MAX,
            forward,
            clock,
        })
    }

    /// Composes a bridge from explicit parts (test seam for the cap bound).
    #[must_use]
    #[cfg(test)]
    pub(crate) fn compose(
        forward: AgentsForwarder,
        store: LocalStore,
        memfs: GitMemFs,
        backend_root: PathBuf,
        side_paths: SidePaths,
        agents_max: usize,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Self {
        Self {
            store,
            memfs,
            backend_root,
            side_paths,
            agents_max,
            forward,
            clock,
        }
    }

    /// Routes one decoded command in a detached task, like the baseline.
    pub fn handle(self: &Arc<Self>, connection: ConnectionId, command: &AgentsCommand) {
        let this = Arc::clone(self);
        let command = command.clone();
        tokio::spawn(async move { this.apply(connection, &command).await });
    }

    /// Applies one command inline, emitting responses through the forwarder.
    pub async fn apply(&self, connection: ConnectionId, command: &AgentsCommand) {
        match command {
            AgentsCommand::CreateShortcut(payload) => {
                self.create_shortcut(connection, payload).await;
            }
            AgentsCommand::List(payload) => self.list(connection, payload).await,
            AgentsCommand::Retrieve(payload) => self.retrieve(connection, payload).await,
            AgentsCommand::Create(payload) => self.create(connection, payload).await,
            AgentsCommand::Update(payload) => self.update(connection, payload).await,
            AgentsCommand::Delete(payload) => self.delete(connection, payload).await,
        }
    }

    async fn create_shortcut(&self, connection: ConnectionId, command: &CreateAgentCommand) {
        let message = match self.create_shortcut_core(command).await {
            Ok(agent) => AgentsMessage::CreateShortcut(CreateAgentResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                agent_id: Some(agent.id.as_str().to_owned()),
                name: Some(agent.name.as_str().to_owned()),
                model: Some(agent.model.as_str().to_owned()),
                error: None,
            }),
            Err(detail) => AgentsMessage::CreateShortcut(CreateAgentResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                agent_id: None,
                name: None,
                model: None,
                error: Some(detail),
            }),
        };
        self.emit(connection, message);
    }

    async fn create_shortcut_core(&self, command: &CreateAgentCommand) -> Result<Agent, String> {
        let preset = presets::preset(command.personality);
        // Model resolution rejects unknown identifiers before any side effect
        // runs, exactly like the pinned pre-validation.
        let model = presets::resolve_request_model(command.model.as_deref(), &preset)?;
        let body = AgentCreateBody {
            name: preset.label.to_owned(),
            model: Some(model),
            system: Some(presets::system_prompt()),
            description: Some(ExplicitField::Value(preset.description.to_owned())),
            tags: Some(presets::creation_tags(&preset, command.tags.as_deref())),
            model_settings: None,
            hidden: None,
            compaction_settings: None,
            memory_blocks: presets::memory_blocks(&preset)
                .map_err(|()| CREATE_FAILURE.to_owned())?,
        };
        let agent = self.create_core(&body).await?;
        // Pinned control flow pins by default before the success frame, so a
        // pin failure fails the response even though the agent now exists.
        if command.pin_global != Some(false) {
            self.pin_agent(agent.id.as_str())
                .map_err(|()| CREATE_FAILURE.to_owned())?;
        }
        Ok(agent)
    }

    async fn create(&self, connection: ConnectionId, command: &AgentCreateCommand) {
        let message = match self.create_core(&command.body).await {
            Ok(agent) => AgentsMessage::Create(AgentCreateResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                agent: Some(agent),
                error: None,
            }),
            Err(detail) => AgentsMessage::Create(AgentCreateResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                agent: None,
                error: Some(detail),
            }),
        };
        self.emit(connection, message);
    }

    async fn create_core(&self, body: &AgentCreateBody) -> Result<Agent, String> {
        // Compaction validation rejects malformed records before any storage
        // work, mirroring the pinned pre-creation validation.
        validate_create_compaction(body)?;
        if self.count_agent_records()? >= self.agents_max {
            return Err(AGENT_LIMIT_REACHED.to_owned());
        }
        let agent = build_new_agent(body)?;
        self.store.save(&agent).await.map_err(|_| CREATE_FAILURE)?;
        self.init_memory_repo(&agent.id, &body.memory_blocks)
            .await?;
        // The response waits on compilation: emitting earlier would let a
        // client observe an agent whose default prompt was never compiled.
        self.compile_default_prompt(&agent).await?;
        Ok(agent)
    }

    async fn list(&self, connection: ConnectionId, command: &AgentListCommand) {
        let query = command.query.clone().unwrap_or_default();
        let limit = query
            .limit
            .unwrap_or(AGENT_LIST_DEFAULT_ITEMS)
            .min(QUERY_PAGE_ITEMS_MAX);
        let message = match self
            .list_page(filters_from_query(&query), limit, query.after.as_deref())
            .await
        {
            Ok(agents) => AgentsMessage::List(AgentListResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                agents,
                error: None,
            }),
            Err(()) => AgentsMessage::List(AgentListResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                agents: Vec::new(),
                error: Some(LIST_FAILURE.to_owned()),
            }),
        };
        self.emit(connection, message);
    }

    /// Serves one listing page; an unknown `after` cursor slices nothing in
    /// the pinned baseline, so the request restarts from the first page.
    async fn list_page(
        &self,
        filters: AgentFilters,
        limit: usize,
        after: Option<&str>,
    ) -> Result<Vec<Agent>, ()> {
        let mut page = PageRequest {
            limit: Some(limit),
            after: after.map(|id| Cursor::from_item(id.to_owned())),
        };
        match self.store.query_agents(filters.clone(), page.clone()).await {
            Ok(found) => Ok(found.items),
            Err(error) if error.kind() == StoreErrorKind::NotFound && page.after.is_some() => {
                page.after = None;
                self.store
                    .query_agents(filters, page)
                    .await
                    .map(|found| found.items)
                    .map_err(|_| ())
            }
            Err(_) => Err(()),
        }
    }

    async fn retrieve(&self, connection: ConnectionId, command: &AgentRetrieveCommand) {
        let message = match self.lookup(&command.agent_id).await {
            Lookup::Found(agent) => AgentsMessage::Retrieve(AgentRetrieveResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                agent: Some(*agent),
                error: None,
            }),
            Lookup::Missing => AgentsMessage::Retrieve(failed_retrieve(command, AGENT_NOT_FOUND)),
            Lookup::Failed => AgentsMessage::Retrieve(failed_retrieve(command, RETRIEVE_FAILURE)),
        };
        self.emit(connection, message);
    }

    async fn update(&self, connection: ConnectionId, command: &AgentUpdateCommand) {
        let message = match self.update_core(&command.agent_id, &command.body).await {
            Ok(agent) => AgentsMessage::Update(AgentUpdateResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                agent: Some(agent),
                error: None,
            }),
            Err(detail) => AgentsMessage::Update(AgentUpdateResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                agent: None,
                error: Some(detail),
            }),
        };
        self.emit(connection, message);
    }

    async fn update_core(&self, id: &str, body: &AgentUpdateBody) -> Result<Agent, String> {
        // Compaction validation rejects malformed records before the lookup,
        // mirroring the pinned pre-update validation order.
        validate_update_compaction(body)?;
        let current = match self.lookup(id).await {
            Lookup::Found(agent) => *agent,
            Lookup::Missing => return Err(AGENT_NOT_FOUND.to_owned()),
            Lookup::Failed => return Err(UPDATE_FAILURE.to_owned()),
        };
        let updated = apply_update(current, body)?;
        self.store
            .save(&updated)
            .await
            .map_err(|_| UPDATE_FAILURE)?;
        Ok(updated)
    }

    async fn delete(&self, connection: ConnectionId, command: &AgentDeleteCommand) {
        let message = match self.delete_core(&command.agent_id).await {
            Ok(()) => AgentsMessage::Delete(AgentDeleteResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                agent_id: command.agent_id.clone(),
                error: None,
            }),
            Err(detail) => AgentsMessage::Delete(AgentDeleteResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                agent_id: command.agent_id.clone(),
                error: Some(detail),
            }),
        };
        self.emit(connection, message);
    }

    async fn delete_core(&self, id: &str) -> Result<(), String> {
        let agent = match self.lookup(id).await {
            Lookup::Found(agent) => agent,
            Lookup::Missing => return Err(AGENT_NOT_FOUND.to_owned()),
            Lookup::Failed => return Err(DELETE_FAILURE.to_owned()),
        };
        // The default prompt cache directory carries no conversation record,
        // so query-driven removal never sees it; drop it explicitly first so
        // no ownerless compiled prompt outlives its agent.
        self.remove_default_prompt_cache(&agent.id)?;
        self.remove_agent_conversations(&agent.id).await?;
        self.remove_memfs_artifacts(&agent.id)?;
        // The record goes last so an interrupted delete leaves an addressable
        // agent instead of orphaned artifacts without their owner.
        match self.store.delete(&agent.id).await {
            Ok(()) => Ok(()),
            Err(_) => Err(DELETE_FAILURE.to_owned()),
        }
    }

    /// Pins one agent identifier in the pinned-agent side store with bounded
    /// conflict retries. Concurrent in-process writers are serialized by
    /// [`PIN_WRITE_GATE`], every write normalizes the document (sorted,
    /// deduplicated), and an absent document is created through
    /// create-if-absent so concurrent first writers cannot lose each
    /// other's pin.
    fn pin_agent(&self, agent_id: &str) -> Result<(), ()> {
        let _gate = match PIN_WRITE_GATE.lock() {
            Ok(gate) => gate,
            Err(poisoned) => poisoned.into_inner(),
        };
        for attempt in 0..PIN_WRITE_ATTEMPTS {
            match lotta_store::side::pinned::read(&self.side_paths) {
                Ok(file) => {
                    let mut ids = pinned_ids(file.bytes());
                    ids.push(agent_id.to_owned());
                    ids.sort();
                    ids.dedup();
                    let bytes = serde_json::to_vec(&serde_json::json!({
                        PINNED_AGENTS_KEY: ids,
                    }))
                    .map_err(|_| ())?;
                    match lotta_store::side::pinned::write_expected(&self.side_paths, &file, &bytes)
                    {
                        Ok(()) => return Ok(()),
                        Err(error) if retryable_pin_error(&error) => {}
                        Err(_) => return Err(()),
                    }
                }
                Err(error) if error.kind() == StoreErrorKind::NotFound => {
                    let bytes = serde_json::to_vec(&serde_json::json!({
                        PINNED_AGENTS_KEY: [agent_id],
                    }))
                    .map_err(|_| ())?;
                    match lotta_store::side::pinned::create_if_absent(&self.side_paths, &bytes) {
                        Ok(()) => return Ok(()),
                        Err(error) if retryable_pin_error(&error) => {}
                        Err(_) => return Err(()),
                    }
                }
                Err(error) if retryable_pin_error(&error) => {}
                Err(_) => return Err(()),
            }
            if attempt + 1 < PIN_WRITE_ATTEMPTS {
                std::thread::sleep(PIN_RETRY_PAUSE);
            }
        }
        Err(())
    }

    async fn lookup(&self, id: &str) -> Lookup {
        match self.store.query_agent(id).await {
            Ok(agent) => Lookup::Found(Box::new(agent)),
            Err(error) if error.kind() == StoreErrorKind::NotFound => Lookup::Missing,
            Err(_) => Lookup::Failed,
        }
    }

    async fn remove_agent_conversations(&self, agent: &AgentId) -> Result<(), String> {
        let filters = ConversationFilters {
            archived: TriState::Any,
            hidden: TriState::Any,
            tags: Vec::new(),
        };
        loop {
            let page = self
                .store
                .query_conversations(agent, filters.clone(), PageRequest::default())
                .await
                .map_err(|_| DELETE_FAILURE)?;
            if page.items.is_empty() {
                return Ok(());
            }
            for conversation in &page.items {
                let directory = self
                    .store
                    .paths()
                    .conversation_dir(agent, &conversation.id)
                    .map_err(|_| DELETE_FAILURE)?;
                remove_path_forcing(&directory)?;
            }
            if page.items.len() < QUERY_PAGE_ITEMS_MAX {
                return Ok(());
            }
        }
    }

    fn remove_memfs_artifacts(&self, agent: &AgentId) -> Result<(), String> {
        remove_path_forcing(&self.backend_root.join("memfs").join(agent.as_str()))
    }

    /// Removes one agent's prompt-only default-conversation cache directory.
    fn remove_default_prompt_cache(&self, agent: &AgentId) -> Result<(), String> {
        let directory = self
            .store
            .paths()
            .conversation_dir(agent, &ConversationId::default_for_agent())
            .map_err(|_| DELETE_FAILURE)?;
        remove_path_forcing(&directory)
    }

    fn count_agent_records(&self) -> Result<usize, String> {
        let directory = self.store.paths().agents();
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(_) => return Err(CREATE_FAILURE.to_owned()),
        };
        Ok(entries.filter_map(Result::ok).count())
    }

    async fn init_memory_repo(
        &self,
        agent: &AgentId,
        blocks: &[MemoryBlockInput],
    ) -> Result<(), String> {
        let converted: Result<Vec<_>, _> = blocks
            .iter()
            .cloned()
            .map(InitialMemoryBlock::from_domain)
            .collect();
        let blocks = InitialMemoryBlocks::new(converted.map_err(|_| CREATE_FAILURE)?)
            .map_err(|_| CREATE_FAILURE)?;
        match self.memfs.initialize(agent, &blocks).await {
            Ok(_) => Ok(()),
            Err(_) => Err(CREATE_FAILURE.to_owned()),
        }
    }

    async fn compile_default_prompt(&self, agent: &Agent) -> Result<(), String> {
        let conversation = ConversationId::default_for_agent();
        let directory = self
            .store
            .paths()
            .conversation_dir(&agent.id, &conversation)
            .map_err(|_| CREATE_FAILURE)?;
        std::fs::create_dir_all(&directory).map_err(|_| CREATE_FAILURE)?;
        let canonical = std::fs::canonicalize(&directory).map_err(|_| CREATE_FAILURE)?;
        let cache = CacheRoot::new(&canonical).map_err(|_| CREATE_FAILURE)?;
        let inputs = PromptInputs::new(
            PromptText::new(agent.system.clone()).map_err(|_| CREATE_FAILURE)?,
            agent.id.clone(),
            conversation,
            0,
            self.clock.now(),
            PromptSections::default(),
        )
        .map_err(|_| CREATE_FAILURE)?;
        let compiler = PromptCompiler::new(&self.memfs);
        cache
            .get_or_compile(
                &compiler,
                &inputs,
                DeliveryCapability::RequestBoundaryOnly,
                CancellationToken::new(),
            )
            .await
            .map(|_| ())
            .map_err(|_| CREATE_FAILURE.to_owned())
    }

    fn emit(&self, connection: ConnectionId, message: AgentsMessage) {
        match serde_json::to_string(&message) {
            Ok(body) if body.len() <= WS_FRAME_BYTES_MAX => {
                let _ = (self.forward)(connection, message);
            }
            Ok(_) => tracing::warn!("agents response exceeded the frame bound and was dropped"),
            Err(_) => tracing::warn!("agents response failed to encode"),
        }
    }
}

/// Forwarder that drops every message (test seam, mirroring the siblings).
#[cfg(test)]
pub(crate) fn inert_forwarder() -> AgentsForwarder {
    Arc::new(|_, _| Ok(()))
}

/// Prepares the shared storage root behind both the store and the `MemFS` port.
fn prepare_backend(
    storage_dir: &std::path::Path,
) -> Result<(LocalStore, GitMemFs, PathBuf), AppServerError> {
    std::fs::create_dir_all(storage_dir)
        .map_err(|_| AppServerError::Config("agents backend unavailable"))?;
    let root = std::fs::canonicalize(storage_dir)
        .map_err(|_| AppServerError::Config("agents backend unavailable"))?;
    let paths = StorePaths::new(root.clone())
        .map_err(|_| AppServerError::Config("agents backend unavailable"))?;
    let memfs = GitMemFs::new(root.clone())
        .map_err(|_| AppServerError::Config("agents backend unavailable"))?;
    Ok((LocalStore::new(paths), memfs, root))
}

/// Generates one random UUID v4 using the shared secure randomness source.
fn new_agent_uuid() -> Result<uuid::Uuid, ()> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| ())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes))
}

/// Builds a fresh tagged domain agent from one validated wire body.
fn build_new_agent(body: &AgentCreateBody) -> Result<Agent, String> {
    let id = AgentId::generate(new_agent_uuid().map_err(|()| CREATE_FAILURE.to_owned())?)
        .map_err(|_| CREATE_FAILURE)?;
    let mut tags = body.tags.clone().unwrap_or_default();
    stamp_git_memory_tag(&mut tags);
    let model = body
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_PERSONALITY_MODEL.to_owned());
    let model_settings = match &body.model_settings {
        Some(settings) => settings.clone(),
        None => BoundedMap::new(BTreeMap::new()).map_err(|_| CREATE_FAILURE)?,
    };
    // Creation mirrors the pinned local-backend create semantics: only a
    // record carrying at least one local key persists (verbatim), a
    // local-keyless record reads as absent, and an explicit null stays
    // distinct from absence.
    let compaction_settings = match &body.compaction_settings {
        Some(ExplicitField::Null) => Some(None),
        Some(ExplicitField::Value(record)) if has_local_compaction(record) => {
            Some(Some(record.clone()))
        }
        Some(ExplicitField::Value(_)) | None => None,
    };
    Ok(Agent {
        id,
        name: NonEmptyString::new(body.name.clone()).map_err(|_| CREATE_FAILURE)?,
        description: body.description.clone().map(explicit_value),
        system: body.system.clone().unwrap_or_default(),
        tags: BoundedVec::new(tags).map_err(|_| CREATE_FAILURE)?,
        model: NonEmptyString::new(model).map_err(|_| CREATE_FAILURE)?,
        model_settings,
        hidden: body.hidden.clone().map(explicit_value),
        compaction_settings,
        extras: EntityExtras::default(),
    })
}

/// Unwraps one sent field state onto the domain's inner option.
fn explicit_value<T>(field: ExplicitField<T>) -> Option<T> {
    match field {
        ExplicitField::Null => None,
        ExplicitField::Value(value) => Some(value),
    }
}

/// Stamps the pinned Git-memory tag exactly once, mirroring the baseline.
fn stamp_git_memory_tag(tags: &mut Vec<String>) {
    if !tags.iter().any(|tag| tag == GIT_MEMORY_ENABLED_TAG) {
        tags.push(GIT_MEMORY_ENABLED_TAG.to_owned());
    }
}

/// Maps the wire filter set onto the Task 28 typed filters.
///
/// `name` narrows case-insensitively like the pinned `query_text` behavior,
/// and hidden agents stay invisible unless explicitly requested.
fn filters_from_query(query: &AgentListQuery) -> AgentFilters {
    AgentFilters {
        name: query.name.clone().map(|name| (name, NameMatch::Substring)),
        query: query.query_text.clone(),
        tags: query.tags.clone(),
        hidden: if query.hidden == Some(true) {
            TriState::True
        } else {
            TriState::False
        },
    }
}

/// Overlays the validated update body onto the loaded snapshot.
fn apply_update(current: Agent, body: &AgentUpdateBody) -> Result<Agent, String> {
    let mut updated = current;
    if let Some(name) = &body.name {
        updated.name = NonEmptyString::new(name.clone()).map_err(|_| UPDATE_FAILURE)?;
    }
    if let Some(field) = body.description.clone() {
        updated.description = Some(explicit_value(field));
    }
    if let Some(system) = &body.system {
        updated.system.clone_from(system);
    }
    if let Some(tags) = &body.tags {
        updated.tags = BoundedVec::new(tags.clone()).map_err(|_| UPDATE_FAILURE)?;
    }
    if let Some(model) = &body.model {
        updated.model = NonEmptyString::new(model.clone()).map_err(|_| UPDATE_FAILURE)?;
    }
    if let Some(settings) = &body.model_settings {
        updated.model_settings.clone_from(settings);
    }
    if let Some(field) = &body.hidden {
        updated.hidden = Some(explicit_value(field.clone()));
    }
    // Explicit null clears; a record replaces only when it carries a local
    // key, matching the pinned local-backend update semantics.
    match &body.compaction_settings {
        Some(ExplicitField::Null) => updated.compaction_settings = Some(None),
        Some(ExplicitField::Value(record)) if has_local_compaction(record) => {
            updated.compaction_settings = Some(Some(record.clone()));
        }
        Some(ExplicitField::Value(_)) | None => {}
    }
    Ok(updated)
}

/// Validates the create body's compaction record before any storage work.
fn validate_create_compaction(body: &AgentCreateBody) -> Result<(), String> {
    match &body.compaction_settings {
        Some(ExplicitField::Value(record)) => validate_compaction_record(record),
        _ => Ok(()),
    }
}

/// Validates the update body's compaction record before the lookup.
fn validate_update_compaction(body: &AgentUpdateBody) -> Result<(), String> {
    match &body.compaction_settings {
        Some(ExplicitField::Value(record)) => validate_compaction_record(record),
        _ => Ok(()),
    }
}

/// Rejects records whose `mode` is present but not a pinned local mode;
/// an absent or null mode passes validation like the baseline guard.
fn validate_compaction_record(
    record: &BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>,
) -> Result<(), String> {
    let recognized = match record.get("mode") {
        None | Some(Value::Null) => true,
        Some(Value::String(mode)) => COMPACTION_MODES.contains(&mode.as_str()),
        Some(_) => false,
    };
    if recognized {
        return Ok(());
    }
    let received = record
        .get("mode")
        .map_or_else(|| "null".to_owned(), javascript_string);
    Err(format!(
        "{COMPACTION_MODE_REJECTED} (received \"{received}\")."
    ))
}

/// Renders one JSON value exactly like the baseline's JavaScript
/// `String(value)` for the shapes JSON can carry: strings verbatim, arrays
/// as their elements joined with `,` (each coerced in turn, where null
/// elements render empty per JavaScript element-wise `ToString`), objects as
/// `[object Object]`, and null, booleans, and numbers per JavaScript rules.
fn javascript_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => javascript_number(number),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    javascript_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// Formats one JSON number like JavaScript `String(number)` for values parsed
/// from JSON: fixed notation inside the digit window, exponential with an
/// explicit positive sign outside it.
fn javascript_number(number: &serde_json::Number) -> String {
    let value = number.as_f64().unwrap_or_default();
    if value == 0.0 {
        return "0".to_owned();
    }
    let rendered = if (1e-6..1e21).contains(&value.abs()) {
        value.to_string()
    } else {
        format!("{value:e}")
    };
    match rendered.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => {
            format!("{mantissa}e+{exponent}")
        }
        _ => rendered,
    }
}

/// Whether one compaction record carries at least one local key; records
/// without any leave the stored value untouched on update.
fn has_local_compaction(record: &BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>) -> bool {
    COMPACTION_SETTING_KEYS
        .iter()
        .any(|key| record.get(key).is_some())
}

/// Transient pinned-document outcomes worth another bounded attempt: a lost
/// compare-and-swap race or a contended storage lock.
fn retryable_pin_error(error: &lotta_store::StoreError) -> bool {
    matches!(
        error.kind(),
        StoreErrorKind::StorageConflict | StoreErrorKind::LottaLock
    )
}

/// Parses the pinned-agent side-store document into its identifier list.
fn pinned_ids(bytes: &[u8]) -> Vec<String> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|document| {
            document
                .get(PINNED_AGENTS_KEY)
                .and_then(Value::as_array)
                .cloned()
        })
        .map_or_else(Vec::new, |entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
}

/// Removes one path tree, tolerating an already-absent target like the pinned
/// `rmSync(..., { force: true })`.
fn remove_path_forcing(path: &std::path::Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(DELETE_FAILURE.to_owned()),
    }
}

/// Failure-shaped retrieval frame carrying one scrubbed detail.
fn failed_retrieve(command: &AgentRetrieveCommand, detail: &str) -> AgentRetrieveResponseMessage {
    AgentRetrieveResponseMessage {
        request_id: command.request_id.clone(),
        success: false,
        agent: None,
        error: Some(detail.to_owned()),
    }
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known agent commands.
pub fn decode(frame: &DecodedFrame) -> Result<Option<AgentsCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let parsed = match tag {
        Tag::CreateAgent => typed::<CreateAgentCommand>(frame).map(AgentsCommand::CreateShortcut),
        Tag::AgentList => typed::<AgentListCommand>(frame).map(AgentsCommand::List),
        Tag::AgentRetrieve => typed::<AgentRetrieveCommand>(frame).map(AgentsCommand::Retrieve),
        Tag::AgentCreate => typed::<AgentCreateCommand>(frame).map(AgentsCommand::Create),
        Tag::AgentUpdate => typed::<AgentUpdateCommand>(frame).map(AgentsCommand::Update),
        Tag::AgentDelete => typed::<AgentDeleteCommand>(frame).map(AgentsCommand::Delete),
        _ => return Ok(None),
    }?;
    Ok(Some(parsed))
}

fn typed<T: DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ProtocolErrorEnvelope> {
    serde_json::from_value(frame.value.clone()).map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "agents_command_invalid",
        "invalid agents command",
        frame.request_id.clone(),
    )
}

#[cfg(test)]
#[path = "agents_commands_tests.rs"]
mod commands;
#[cfg(test)]
#[path = "agents_create_side_effects_tests.rs"]
mod create_side_effects;
#[cfg(test)]
#[path = "agents_errors_tests.rs"]
mod errors;
#[cfg(test)]
#[path = "agents_fixture_round_trip_tests.rs"]
mod fixture_round_trip;
#[cfg(test)]
#[path = "agents_listing_tests.rs"]
mod listing;
#[path = "agents_presets.rs"]
mod presets;
#[cfg(test)]
#[path = "agents_support.rs"]
mod support;
