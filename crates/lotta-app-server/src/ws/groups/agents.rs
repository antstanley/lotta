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
//! artifacts already in place.
//!
//! Listing serves the Task 28 query behaviors: deterministic identifier
//! ordering with name/query/tag/hidden filters compatible with the pinned
//! local backend, which excludes hidden agents unless they are requested.
//! Identifier lookup goes through
//! [`LocalStore::query_agent`](lotta_store::LocalStore), so an absent or
//! wrong-prefix id answers one safe 404-class failure. `AGENTS_MAX` bounds
//! creation; the cap is checked before any side effect runs.
//!
//! Baseline degradations, kept honest: `pin_global` is accepted but not yet
//! persisted because no pinned-agent store exists in this version, requested
//! models are accepted verbatim rather than resolved against a model catalog,
//! personality presets carry canonical inline content rather than the pinned
//! MDX template assets, creation-time compilation renders no skill sections,
//! and the list command serves only its first page because the pinned
//! response shape carries no cursor field.

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
    AGENTS_MAX, LocalStore, StoreErrorKind, StorePaths,
    query::{
        AgentFilters, ConversationFilters, NameMatch, PageRequest, QUERY_PAGE_ITEMS_MAX, TriState,
    },
};
use serde::{
    Deserialize, Serialize,
    de::{DeserializeOwned, Deserializer, Error as _},
};
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
pub const AGENT_LIST_ITEMS_DEFAULT: usize = 20;
/// Model used when neither the request nor the preset names one.
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
/// Initial persona memory label shared by every personality preset.
const PERSONA_MEMORY_LABEL: &str = "persona";
/// Initial human memory label shared by every personality preset.
const HUMAN_MEMORY_LABEL: &str = "human";

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
    /// Optional model override accepted verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Additional tags appended after the preset tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Whether to pin the agent globally; accepted but not persisted here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_global: Option<bool>,
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
/// Field names follow the canonical `Agent` schema; `description` and
/// `hidden` preserve the absent/null/value three-state contract.
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
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<Option<String>>,
    /// Creation tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Provider model settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>,
    /// Optional hidden flag with explicit-null preservation.
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub hidden: Option<Option<bool>>,
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
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<Option<String>>,
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
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub hidden: Option<Option<bool>>,
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

/// One canonical personality preset.
struct PersonalityPreset {
    label: &'static str,
    description: &'static str,
    default_model: &'static str,
    system: &'static str,
    persona: &'static str,
}

fn personality_preset(id: PersonalityId) -> PersonalityPreset {
    match id {
        PersonalityId::Memo => PersonalityPreset {
            label: "Letta Code",
            description: "The memory-first agent",
            default_model: DEFAULT_PERSONALITY_MODEL,
            system: "You are Letta Code, a memory-first coding agent.",
            persona: "You are Letta Code. Persist durable facts about the user and workspace.",
        },
        PersonalityId::Tutorial => PersonalityPreset {
            label: "Tutor",
            description: "I help with getting started with Letta",
            default_model: DEFAULT_PERSONALITY_MODEL,
            system: "You are a patient Letta onboarding tutor.",
            persona: "You teach new users how to configure and use Letta agents.",
        },
        PersonalityId::Blank => PersonalityPreset {
            label: "Blank",
            description: "Blank starter — you provide the personality",
            default_model: DEFAULT_PERSONALITY_MODEL,
            system: "You are a helpful assistant.",
            persona: "The user provides the personality for this agent.",
        },
        PersonalityId::Linus => PersonalityPreset {
            label: "Linus",
            description: "Code with a stern hand",
            default_model: DEFAULT_PERSONALITY_MODEL,
            system: "You review and write code with blunt, exacting standards.",
            persona: "You are a stern code reviewer who tolerates no sloppiness.",
        },
        PersonalityId::Kawaii => PersonalityPreset {
            label: "Letta-Chan",
            description: "sugoi~",
            default_model: DEFAULT_PERSONALITY_MODEL,
            system: "You are a cheerful, playful assistant.",
            persona: "You answer with warmth, sparkles, and playful energy.",
        },
    }
}

/// Builds one validated initial memory block from static preset content.
fn static_block(
    label: &'static str,
    value: String,
    description: String,
) -> Result<MemoryBlockInput, ()> {
    Ok(MemoryBlockInput {
        label: NonEmptyString::new(label).map_err(|_| ())?,
        value,
        description: Some(Some(description)),
    })
}

/// Builds the two initial memory blocks every preset seeds.
///
/// # Errors
/// Fails only if a static label were ever emptied, which the type system
/// cannot express for `'static` literals.
fn preset_memory_blocks(preset: &PersonalityPreset) -> Result<Vec<MemoryBlockInput>, ()> {
    Ok(vec![
        static_block(
            PERSONA_MEMORY_LABEL,
            preset.persona.to_owned(),
            format!("{} persona", preset.label),
        )?,
        static_block(
            HUMAN_MEMORY_LABEL,
            "Facts about the user and their workspace.".to_owned(),
            format!("{} human context", preset.label),
        )?,
    ])
}

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
        Ok(Self {
            store,
            memfs,
            backend_root,
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
        agents_max: usize,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Self {
        Self {
            store,
            memfs,
            backend_root,
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
        let preset = personality_preset(command.personality);
        let memory_blocks = preset_memory_blocks(&preset).unwrap_or_default();
        let body = AgentCreateBody {
            name: preset.label.to_owned(),
            model: Some(
                command
                    .model
                    .clone()
                    .unwrap_or_else(|| preset.default_model.to_owned()),
            ),
            system: Some(preset.system.to_owned()),
            description: Some(Some(preset.description.to_owned())),
            tags: command.tags.clone(),
            model_settings: None,
            hidden: None,
            memory_blocks,
        };
        let (success, agent_id, name, model, error) = match self.create_core(&body).await {
            Ok(agent) => (
                true,
                Some(agent.id.as_str().to_owned()),
                Some(agent.name.as_str().to_owned()),
                Some(agent.model.as_str().to_owned()),
                None,
            ),
            Err(detail) => (false, None, None, None, Some(detail)),
        };
        self.emit(
            connection,
            AgentsMessage::CreateShortcut(CreateAgentResponseMessage {
                request_id: command.request_id.clone(),
                success,
                agent_id,
                name,
                model,
                error,
            }),
        );
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
            .unwrap_or(AGENT_LIST_ITEMS_DEFAULT)
            .min(QUERY_PAGE_ITEMS_MAX);
        let page = PageRequest {
            limit: Some(limit),
            after: None,
        };
        let message = match self
            .store
            .query_agents(filters_from_query(&query), page)
            .await
        {
            Ok(page) => AgentsMessage::List(AgentListResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                agents: page.items,
                error: None,
            }),
            Err(_) => AgentsMessage::List(AgentListResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                agents: Vec::new(),
                error: Some(LIST_FAILURE.to_owned()),
            }),
        };
        self.emit(connection, message);
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
        self.remove_agent_conversations(&agent.id).await?;
        self.remove_memfs_artifacts(&agent.id)?;
        // The record goes last so an interrupted delete leaves an addressable
        // agent instead of orphaned artifacts without their owner.
        match self.store.delete(&agent.id).await {
            Ok(()) => Ok(()),
            Err(_) => Err(DELETE_FAILURE.to_owned()),
        }
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
    Ok(Agent {
        id,
        name: NonEmptyString::new(body.name.clone()).map_err(|_| CREATE_FAILURE)?,
        description: body.description.clone(),
        system: body.system.clone().unwrap_or_default(),
        tags: BoundedVec::new(tags).map_err(|_| CREATE_FAILURE)?,
        model: NonEmptyString::new(model).map_err(|_| CREATE_FAILURE)?,
        model_settings,
        hidden: body.hidden,
        compaction_settings: None,
        extras: EntityExtras::default(),
    })
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
    if let Some(description) = body.description.clone() {
        updated.description = Some(description);
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
    if body.hidden.is_some() {
        updated.hidden = body.hidden;
    }
    Ok(updated)
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

/// Three-state JSON deserializer preserving the absent/null/value contract.
#[allow(clippy::option_option, reason = "three-state JSON presence contract")]
fn nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
        .map(Some)
        .map_err(D::Error::custom)
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
#[cfg(test)]
#[path = "agents_support.rs"]
mod support;
