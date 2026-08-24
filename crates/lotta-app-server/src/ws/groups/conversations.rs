//! WebSocket conversation management command group.
//!
//! Decodes and routes the eight §WebSocket command groups Conversation
//! management commands — `conversation_list`, `conversation_retrieve`,
//! `conversation_create`, `conversation_update`, `conversation_recompile`,
//! `conversation_fork`, `conversation_messages_list`, and
//! `conversation_compact`. Responses mirror the pinned protocol shapes
//! exactly: correlated `request_id`, a `success` flag, and the canonical
//! payload — so every mutating command answers with the authoritative
//! post-mutation snapshot rather than a diff.
//!
//! Listing and retrieval serve the Task 28 query behaviors: newest-first
//! ordering keyed on the latest-message timestamp with identifier tiebreak,
//! hidden-conversation exclusion unless explicitly included, an `after`
//! continuation cursor, and a default twenty-item page. Creation enforces
//! [`lotta_store::CONVERSATIONS_PER_AGENT_MAX`] before any record or prompt-cache artifact
//! is written, mirrors the pinned creation defaults (empty context, explicit
//! nulls, optional model override), and finishes by compiling the
//! conversation prompt through the Task 30 compiler exactly like the pinned
//! backend. Updates apply only sent fields, reject the virtual default
//! conversation, and persist through the Task 23 store.
//!
//! Fork performs the §Transcript contract full-rewrite case: a brand-new
//! conversation identifier receives its own encoded key-form directory, a
//! fresh manifest and session header, and the inherited active history up to
//! the optional inclusive `message_id` cursor — written once through
//! [`lotta_store::LocalStore::persist_fork_transcript`]. The source conversation bytes are
//! never touched. Recompile renders the committed memory prompt through the
//! Task 30 [`lotta_memfs::PromptCompiler`], persisting the cache record unless the caller
//! asked for a dry run.
//!
//! Compact delegates to the Task 58 lease-serialized flow:
//! [`lotta_runtime::CompactionService`] claims a durable transaction journal entry, plans
//! against the active transcript, summarizes, appends exactly one compaction
//! entry, publishes the new in-context identifiers onto the conversation
//! record, and re-checks the turn lease before every externally visible
//! effect. This group never appends transcript rows directly. The manual
//! management trigger holds its own per-scope command lease while the service
//! runs, so concurrent compactions of one conversation serialize instead of
//! interleaving journal transactions.
//!
//! Baseline degradations, kept honest: the compatibility listener owns no
//! provider port, so summaries are deterministic extractions of the eligible
//! history rather than model output, and compaction lifecycle hooks stay
//! wired only on the production turn path.

use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex},
};

use lotta_domain::{
    AgentId, BoundedMap, BoundedVec, Clock, CompactionEntry, CompactionEntryType, Conversation,
    ConversationId, EntityExtras, InContextMessageIds, LocalMessage, LocalMessageRole,
    MessageEntry, MessageEntryType, MessageId, NonEmptyString, ProviderStack, RuntimeScope,
    SessionEntry, SessionEntryType, Timestamp, TranscriptEntry, TranscriptManifest,
    TranscriptMessageFormat, TurnLease, TurnLifecycle, bounds::UNBOUNDED_MAP_FIELDS_MAX,
};
use lotta_memfs::{
    CacheRoot, DeliveryCapability, GitMemFs, PromptCompiler, PromptInputs, PromptSections,
    PromptText,
};
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::{
    CompactionCommand, CompactionEffects, CompactionMode, CompactionRecovery, CompactionService,
    CompactionSummary, CompactionTrigger, RuntimeError,
    boundary::ProviderText,
    ports::{
        AgentStore, ConversationStore, ImagePolicy, ProviderContent, ProviderContentPart,
        ProviderContextOverflowDetail, ProviderDeadline, ProviderMessage, ProviderMessageRole,
        ProviderMessages, ProviderRequest, ProviderToolChoice, ProviderTools, ReasoningControls,
        TokenLimit, estimate_request_tokens,
    },
};
use lotta_store::{
    CONVERSATIONS_PER_AGENT_MAX, CompactionProjection, CompactionTransaction,
    CompactionTransactionState, LocalStore, StorePaths,
    query::{
        ConversationFilters, MessageListOptions, MessageOrder, PageRequest, QUERY_PAGE_ITEMS_MAX,
        ReturnMessageType, TriState,
    },
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::future::Future;
use tokio_util::sync::CancellationToken;

use super::agents::ExplicitField;
use crate::{
    bounds::WS_FRAME_BYTES_MAX,
    errors::{AppServerError, ProtocolErrorEnvelope},
    framing::DecodedFrame,
    ws::connection::ConnectionId,
};

/// Conversations listed when the client omits an explicit limit (pinned parity).
pub const CONVERSATION_LIST_DEFAULT_ITEMS: usize = 20;
/// Messages listed when the client omits an explicit limit (pinned parity).
pub const MESSAGE_LIST_DEFAULT_ITEMS: usize = 50;
/// Bounded attempts to mint an unused sequential conversation identifier.
const NEW_ID_ATTEMPTS_MAX: usize = 4;
/// Fallback context-window ceiling used for token estimates in estimates-only detail.
const COMPACTION_CONTEXT_WINDOW_FALLBACK_TOKENS: u64 = 128_000;
/// Longest per-part excerpt carried into a deterministic summary.
const SUMMARY_PART_CHARS_MAX: usize = 512;
/// Most content parts carried into a deterministic summary per message.
const SUMMARY_PARTS_PER_MESSAGE_MAX: usize = 8;

const AGENT_NOT_FOUND: &str = "agent not found";
const CONVERSATION_NOT_FOUND: &str = "conversation not found";
const MESSAGE_NOT_FOUND: &str = "message not found";
const DEFAULT_UPDATE_REJECTED: &str = "Default conversation cannot be updated";
const CONVERSATIONS_LIMIT_REACHED: &str = "conversation limit reached";
const CONVERSATION_BUSY: &str = "conversation busy";
const CREATE_FAILURE: &str = "Failed to create conversation";
const LIST_FAILURE: &str = "Failed to list conversations";
const RETRIEVE_FAILURE: &str = "Failed to retrieve conversation";
const UPDATE_FAILURE: &str = "Failed to update conversation";
const RECOMPILE_FAILURE: &str = "Failed to recompile conversation";
const FORK_FAILURE: &str = "Failed to fork conversation";
const MESSAGES_FAILURE: &str = "Failed to list conversation messages";
const COMPACT_FAILURE: &str = "Failed to compact conversation";

// ── Wire commands ───────────────────────────────────────────────────────────

/// Query parameters accepted by the pinned `conversation_list`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConversationListQuery {
    /// Restrict the listing to one owning agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Case-insensitive substring filter over identifier and summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_search: Option<String>,
    /// Include hidden conversations when true; absence excludes them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_hidden: Option<bool>,
    /// Maximum returned conversations after ordering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Continue strictly after this conversation identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// Pinned `conversation_list` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationListCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Optional filter set; absence lists visible conversations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<ConversationListQuery>,
}

/// Pinned `conversation_retrieve` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationRetrieveCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Conversation to read.
    pub conversation_id: String,
}

/// Optional agent scoping shared by several payloads.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConversationScopeBody {
    /// Owning agent identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Skip cache persistence when true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}

/// Body of the pinned `conversation_create` payload; field names follow the
/// canonical `Conversation` schema and absent keys fall back to pinned
/// defaults (unarchived, empty context, no override).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConversationCreateBody {
    /// Owning agent identifier; resolved before any artifact is written.
    pub agent_id: Option<String>,
    /// Initial summary; absent reads as an explicit null like the baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Conversation-level model override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider model settings for the override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>,
    /// Optional positive context-window limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_limit: Option<u64>,
    /// Initial hidden state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// Initial tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// Pinned `conversation_create` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationCreateCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Body forwarded to the canonical conversation store.
    pub body: ConversationCreateBody,
}

/// Body of the pinned `conversation_update`; absent fields stay unchanged and
/// summary/last-message/model keys preserve the absent/null/value contract.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConversationUpdateBody {
    /// Replacement archive state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// Replacement latest-message timestamp; explicit null clears it.
    #[serde(
        default,
        deserialize_with = "super::agents::explicit_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_message_at: Option<ExplicitField<String>>,
    /// Replacement summary; explicit null clears it.
    #[serde(
        default,
        deserialize_with = "super::agents::explicit_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub summary: Option<ExplicitField<String>>,
    /// Replacement model override; explicit null clears it.
    #[serde(
        default,
        deserialize_with = "super::agents::explicit_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub model: Option<ExplicitField<String>>,
    /// Replacement provider model settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>,
    /// Replacement context-window limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_limit: Option<u64>,
    /// Replacement hidden state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// Replacement tag list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// Pinned `conversation_update` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationUpdateCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Conversation to replace fields on.
    pub conversation_id: String,
    /// Validated replacement body.
    pub body: ConversationUpdateBody,
}

/// Pinned `conversation_recompile` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationRecompileCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Conversation whose prompt recompiles.
    pub conversation_id: String,
    /// Optional agent scope and dry-run selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ConversationScopeBody>,
}

/// Body of the pinned `conversation_fork` payload.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConversationForkBody {
    /// Target agent for agent-direct forks; also scopes source resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Whether the forked conversation should be hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// Optional projected message ID to fork through, inclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

/// Pinned `conversation_fork` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationForkCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Source conversation to inherit history from.
    pub conversation_id: String,
    /// Optional fork options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ConversationForkBody>,
}

/// Query parameters accepted by the pinned `conversation_messages_list`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConversationMessagesQuery {
    /// Optional owning-agent scope hint checked first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Maximum returned messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Exclusive older-boundary projected message ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// Exclusive newer-boundary projected message ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// `asc` serves oldest-first; everything else serves newest-first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<String>,
    /// Included return-message categories; absence includes all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_return_message_types: Option<Vec<ReturnMessageType>>,
}

/// Pinned `conversation_messages_list` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationMessagesListCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Conversation whose messages serve.
    pub conversation_id: String,
    /// Optional parameter set; defaults serve the newest fifty messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<ConversationMessagesQuery>,
}

/// Body of the pinned `conversation_compact` payload.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConversationCompactBody {
    /// Optional owning-agent scope hint checked first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// Pinned `conversation_compact` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationCompactCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Conversation to compact.
    pub conversation_id: String,
    /// Optional compaction options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ConversationCompactBody>,
}

/// The eight concrete conversation management commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum ConversationsCommand {
    /// Deterministic filtered listing.
    #[serde(rename = "conversation_list")]
    List(ConversationListCommand),
    /// One conversation snapshot by identifier.
    #[serde(rename = "conversation_retrieve")]
    Retrieve(ConversationRetrieveCommand),
    /// Creation under the per-agent cap.
    #[serde(rename = "conversation_create")]
    Create(ConversationCreateCommand),
    /// Field replacement by identifier.
    #[serde(rename = "conversation_update")]
    Update(ConversationUpdateCommand),
    /// Prompt recompilation through the Task 30 compiler.
    #[serde(rename = "conversation_recompile")]
    Recompile(ConversationRecompileCommand),
    /// Full transcript rewrite into a new key-form directory.
    #[serde(rename = "conversation_fork")]
    Fork(ConversationForkCommand),
    /// Parameterized active-projection page.
    #[serde(rename = "conversation_messages_list")]
    MessagesList(ConversationMessagesListCommand),
    /// Lease-serialized Task 58 compaction.
    #[serde(rename = "conversation_compact")]
    Compact(ConversationCompactCommand),
}

// ── Wire responses ──────────────────────────────────────────────────────────

/// Pinned reference to the newly created fork.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ForkedConversationReference {
    /// New conversation identifier.
    pub id: String,
}

/// Pinned compaction outcome summary.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompactionOutcome {
    /// Active messages before compaction.
    pub num_messages_before: usize,
    /// Summary plus retained messages after compaction.
    pub num_messages_after: usize,
    /// Recorded summary text.
    pub summary: String,
}

/// Pinned `conversation_list_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct ConversationListResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Matching conversations in deterministic order.
    pub conversations: Vec<Conversation>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Shared shape of the retrieve/create/update response payloads.
#[derive(Clone, Debug, Serialize)]
pub struct ConversationSnapshotResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Full snapshot, or null on failure like the pinned listener.
    pub conversation: Option<Conversation>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `conversation_recompile_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct ConversationRecompileResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Compiled prompt content, or null on failure.
    pub result: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `conversation_fork_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct ConversationForkResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// New conversation reference, or null on failure.
    pub conversation: Option<ForkedConversationReference>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `conversation_messages_list_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct ConversationMessagesListResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Projected messages in the requested order.
    pub messages: Vec<lotta_store::query::ProjectedMessage>,
    /// Oldest served message ID for loading older history.
    pub next_before: Option<String>,
    /// Whether messages beyond the page remain.
    pub has_more: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `conversation_compact_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct ConversationCompactResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Compaction outcome, or null on failure.
    pub compaction: Option<CompactionOutcome>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Every outbound frame this group emits.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum ConversationsMessage {
    /// Listing result.
    #[serde(rename = "conversation_list_response")]
    List(ConversationListResponseMessage),
    /// Retrieval result.
    #[serde(rename = "conversation_retrieve_response")]
    Retrieve(ConversationSnapshotResponseMessage),
    /// Creation result.
    #[serde(rename = "conversation_create_response")]
    Create(ConversationSnapshotResponseMessage),
    /// Update result.
    #[serde(rename = "conversation_update_response")]
    Update(ConversationSnapshotResponseMessage),
    /// Recompilation result.
    #[serde(rename = "conversation_recompile_response")]
    Recompile(ConversationRecompileResponseMessage),
    /// Fork result.
    #[serde(rename = "conversation_fork_response")]
    Fork(ConversationForkResponseMessage),
    /// Message-page result.
    #[serde(rename = "conversation_messages_list_response")]
    MessagesList(ConversationMessagesListResponseMessage),
    /// Compaction result.
    #[serde(rename = "conversation_compact_response")]
    Compact(ConversationCompactResponseMessage),
}

/// Push callback delivering one outbound message to one connection.
pub type ConversationsForwarder =
    Arc<dyn Fn(ConnectionId, ConversationsMessage) -> Result<(), AppServerError> + Send + Sync>;

/// Outcome class of one scoped identifier resolution.
enum Lookup {
    /// The record exists under this agent scope.
    Found(Box<Conversation>),
    /// Every candidate scope missed.
    Missing,
    /// Storage failed without exposing detail.
    Failed,
}

type LeaseRegistry = Arc<Mutex<HashMap<RuntimeScope, Arc<Mutex<TurnLifecycle>>>>>;

/// Applies wire commands to the Task 23/24 store, Task 28 queries, the Task 30
/// prompt compiler, and the Task 58 compaction service.
pub struct ConversationsBridge {
    store: LocalStore,
    memfs: GitMemFs,
    forward: ConversationsForwarder,
    clock: Arc<dyn Clock + Send + Sync>,
    conversations_per_agent_max: usize,
    compaction_leases: LeaseRegistry,
}

impl ConversationsBridge {
    /// Creates the production bridge over one canonical local storage root.
    ///
    /// # Errors
    /// Returns [`AppServerError::Config`] when the storage root cannot be
    /// prepared for the store or the `MemFS` backend.
    pub fn new(
        forward: ConversationsForwarder,
        storage_dir: &std::path::Path,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, AppServerError> {
        let (store, memfs) = prepare_backend(storage_dir)?;
        Ok(Self {
            store,
            memfs,
            forward,
            clock,
            conversations_per_agent_max: CONVERSATIONS_PER_AGENT_MAX,
            compaction_leases: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Composes a bridge from explicit parts (test seam for the cap bound).
    #[must_use]
    #[cfg(test)]
    pub(crate) fn compose(
        forward: ConversationsForwarder,
        store: LocalStore,
        memfs: GitMemFs,
        conversations_per_agent_max: usize,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Self {
        Self {
            store,
            memfs,
            forward,
            clock,
            conversations_per_agent_max,
            compaction_leases: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Routes one decoded command in a detached task, like the baseline.
    pub fn handle(self: &Arc<Self>, connection: ConnectionId, command: &ConversationsCommand) {
        let this = Arc::clone(self);
        let command = command.clone();
        tokio::spawn(async move { this.apply(connection, &command).await });
    }

    /// Applies one command inline, emitting responses through the forwarder.
    pub async fn apply(&self, connection: ConnectionId, command: &ConversationsCommand) {
        match command {
            ConversationsCommand::List(payload) => {
                let outcome = self.list_core(payload.query.as_ref());
                self.emit_snapshot_list(connection, payload, outcome.await);
            }
            ConversationsCommand::Retrieve(payload) => {
                let outcome = self.retrieve_core(payload).await;
                self.emit_retrieve(connection, &payload.request_id, outcome);
            }
            ConversationsCommand::Create(payload) => {
                let outcome = self.create_core(&payload.body).await;
                self.emit_create(connection, &payload.request_id, outcome);
            }
            ConversationsCommand::Update(payload) => {
                let outcome = self
                    .update_core(&payload.conversation_id, &payload.body)
                    .await;
                self.emit_update(connection, &payload.request_id, outcome);
            }
            ConversationsCommand::Recompile(payload) => {
                self.emit_recompile(connection, payload, self.recompile_core(payload).await);
            }
            ConversationsCommand::Fork(payload) => {
                self.emit_fork(connection, payload, self.fork_core(payload).await);
            }
            ConversationsCommand::MessagesList(payload) => {
                self.emit_messages(connection, payload, self.messages_core(payload).await);
            }
            ConversationsCommand::Compact(payload) => {
                self.emit_compact(connection, payload, self.compact_core(payload).await);
            }
        }
    }

    async fn list_core(
        &self,
        query: Option<&ConversationListQuery>,
    ) -> Result<Vec<Conversation>, String> {
        let query = query.cloned().unwrap_or_default();
        let mut collected = Vec::new();
        for agent in self
            .candidate_agents(query.agent_id.as_deref(), LIST_FAILURE)
            .await?
        {
            self.collect_agent_conversations(
                &agent,
                query.include_hidden,
                LIST_FAILURE,
                &mut collected,
            )
            .await?;
        }
        Ok(apply_list_options(collected, &query))
    }

    async fn collect_agent_conversations(
        &self,
        agent: &AgentId,
        include_hidden: Option<bool>,
        failure: &'static str,
        collected: &mut Vec<Conversation>,
    ) -> Result<(), String> {
        let filters = ConversationFilters {
            archived: TriState::Any,
            hidden: if include_hidden == Some(true) {
                TriState::Any
            } else {
                TriState::False
            },
            tags: Vec::new(),
        };
        let mut page = PageRequest {
            limit: Some(QUERY_PAGE_ITEMS_MAX),
            after: None,
        };
        loop {
            let found = self
                .store
                .query_conversations(agent, filters.clone(), page.clone())
                .await
                .map_err(|_| failure.to_owned())?;
            let complete = found.items.len() < QUERY_PAGE_ITEMS_MAX;
            collected.extend(found.items);
            if complete {
                return Ok(());
            }
            page.after = found.next;
        }
    }

    async fn retrieve_core(
        &self,
        command: &ConversationRetrieveCommand,
    ) -> Result<Conversation, String> {
        let conversation_id = parse_conversation(&command.conversation_id, RETRIEVE_FAILURE)?;
        match self.resolve(None, &conversation_id).await {
            Lookup::Found(conversation) => Ok(*conversation),
            Lookup::Missing => Err(CONVERSATION_NOT_FOUND.to_owned()),
            Lookup::Failed => Err(RETRIEVE_FAILURE.to_owned()),
        }
    }

    async fn create_core(&self, body: &ConversationCreateBody) -> Result<Conversation, String> {
        let agent_id = parse_agent(body.agent_id.as_deref())?;
        self.require_agent(&agent_id).await?;
        if self.count_conversations(&agent_id, CREATE_FAILURE).await?
            >= self.conversations_per_agent_max
        {
            return Err(CONVERSATIONS_LIMIT_REACHED.to_owned());
        }
        let conversation = self.build_new_conversation(&agent_id, body).await?;
        ConversationStore::save(&self.store, &conversation)
            .await
            .map_err(|_| CREATE_FAILURE.to_owned())?;
        // The pinned backend persists the compiled prompt before returning, so
        // a client that observed success would always find the cache record.
        self.compile_prompt(&agent_id, &conversation.id, false, CREATE_FAILURE)
            .await?;
        Ok(conversation)
    }

    async fn update_core(
        &self,
        conversation_id: &str,
        body: &ConversationUpdateBody,
    ) -> Result<Conversation, String> {
        let parsed = parse_conversation(conversation_id, UPDATE_FAILURE)?;
        if parsed.is_default() {
            return Err(DEFAULT_UPDATE_REJECTED.to_owned());
        }
        let current = match self.resolve(None, &parsed).await {
            Lookup::Found(conversation) => *conversation,
            Lookup::Missing => return Err(CONVERSATION_NOT_FOUND.to_owned()),
            Lookup::Failed => return Err(UPDATE_FAILURE.to_owned()),
        };
        let updated = apply_update(current, body, self.clock.now())?;
        ConversationStore::save(&self.store, &updated)
            .await
            .map_err(|_| UPDATE_FAILURE.to_owned())?;
        Ok(updated)
    }

    async fn recompile_core(
        &self,
        command: &ConversationRecompileCommand,
    ) -> Result<String, String> {
        let conversation_id = parse_conversation(&command.conversation_id, RECOMPILE_FAILURE)?;
        let hint = command.body.as_ref().and_then(|body| body.agent_id.clone());
        let dry_run = command.body.as_ref().and_then(|body| body.dry_run) == Some(true);
        let agent_id = match self.resolve(hint.as_deref(), &conversation_id).await {
            Lookup::Found(conversation) => conversation.agent_id,
            Lookup::Missing => return Err(CONVERSATION_NOT_FOUND.to_owned()),
            Lookup::Failed => return Err(RECOMPILE_FAILURE.to_owned()),
        };
        self.compile_prompt(&agent_id, &conversation_id, dry_run, RECOMPILE_FAILURE)
            .await
    }

    async fn fork_core(
        &self,
        command: &ConversationForkCommand,
    ) -> Result<ForkedConversationReference, String> {
        let body = command.body.clone().unwrap_or_default();
        let source_id = parse_conversation(&command.conversation_id, FORK_FAILURE)?;
        let (source_agent, source) = match self.resolve(body.agent_id.as_deref(), &source_id).await
        {
            Lookup::Found(found) => {
                let agent = found.agent_id.clone();
                (agent, *found)
            }
            Lookup::Missing => return Err(CONVERSATION_NOT_FOUND.to_owned()),
            Lookup::Failed => return Err(FORK_FAILURE.to_owned()),
        };
        let target_agent = match body.agent_id.as_deref() {
            Some(text) => parse_agent(Some(text))?,
            None => source_agent.clone(),
        };
        self.require_agent(&target_agent).await?;
        if self
            .count_conversations(&target_agent, FORK_FAILURE)
            .await?
            >= self.conversations_per_agent_max
        {
            return Err(CONVERSATIONS_LIMIT_REACHED.to_owned());
        }
        let kept = self
            .fork_history(
                &source_agent,
                &source_id,
                body.message_id.as_deref(),
                FORK_FAILURE,
            )
            .await?;
        let forked = self
            .build_forked_record(&source, &target_agent, &kept, body.hidden)
            .await?;
        ConversationStore::save(&self.store, &forked)
            .await
            .map_err(|_| FORK_FAILURE.to_owned())?;
        self.write_fork_transcript(&target_agent, &forked.id, &kept)
            .await?;
        Ok(ForkedConversationReference {
            id: forked.id.as_str().to_owned(),
        })
    }

    /// Loads the inherited active history for one fork, cut inclusively at the
    /// optional projected `message_id` cursor.
    async fn fork_history(
        &self,
        agent: &AgentId,
        conversation_id: &ConversationId,
        message_id: Option<&str>,
        failure: &'static str,
    ) -> Result<Vec<LocalMessage>, String> {
        let loaded = self
            .store
            .load_transcript(agent, conversation_id)
            .await
            .map_err(|_| failure.to_owned())?;
        let messages = loaded.messages();
        let Some(wanted) = message_id else {
            return Ok(messages.to_vec());
        };
        let projected = self
            .store
            .query_messages_for_conversation(
                agent,
                conversation_id,
                MessageListOptions {
                    order: MessageOrder::Ascending,
                    ..MessageListOptions::default()
                },
            )
            .await
            .map_err(|_| failure.to_owned())?;
        let cursor = projected
            .iter()
            .position(|message| message.id.as_str() == wanted)
            .ok_or_else(|| MESSAGE_NOT_FOUND.to_owned())?;
        let source_ordinal = projected[cursor].source.source_ordinal;
        Ok(messages[..=source_ordinal.min(messages.len().saturating_sub(1))].to_vec())
    }

    async fn build_forked_record(
        &self,
        source: &Conversation,
        target_agent: &AgentId,
        kept: &[LocalMessage],
        hidden: Option<bool>,
    ) -> Result<Conversation, String> {
        let now = self.clock.now();
        let id = self
            .next_conversation_id(target_agent, FORK_FAILURE)
            .await?;
        let context_ids = kept
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        Ok(Conversation {
            id,
            agent_id: target_agent.clone(),
            archived: false,
            archived_at: Some(None),
            created_at: now,
            updated_at: now,
            last_message_at: source.last_message_at,
            summary: source.summary.clone(),
            in_context_message_ids: BoundedVec::new(context_ids)
                .map_err(|_| FORK_FAILURE.to_owned())?,
            model: source.model.clone(),
            model_settings: source.model_settings.clone(),
            context_window_limit: source.context_window_limit,
            hidden: hidden.or(source.hidden),
            tags: source.tags.clone(),
            extras: EntityExtras::default(),
        })
    }

    /// Writes the fork target's own key-form directory: a fresh manifest and
    /// session header followed by one atomic full rewrite carrying the
    /// inherited history, per the §Transcript contract fork case.
    async fn write_fork_transcript(
        &self,
        agent: &AgentId,
        conversation_id: &ConversationId,
        kept: &[LocalMessage],
    ) -> Result<(), String> {
        let now = self.clock.now();
        let manifest = TranscriptManifest {
            schema_version: TRANSCRIPT_MANIFEST_SCHEMA_VERSION,
            message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
            provider_stack: ProviderStack::PiAi,
            created_at: now,
            migrated_from: None,
            migrated_at: None,
            backup_path: None,
        };
        let session = session_entry(conversation_id, &now)?;
        self.store
            .initialize_transcript(agent, conversation_id, &manifest, &session)
            .await
            .map_err(|_| FORK_FAILURE.to_owned())?;
        let mut entries = vec![session];
        for (index, message) in kept.iter().enumerate() {
            entries.push(message_entry(message, index, &now)?);
        }
        self.store
            .persist_fork_transcript(agent, conversation_id, entries)
            .await
            .map_err(|_| FORK_FAILURE.to_owned())
    }

    async fn messages_core(
        &self,
        command: &ConversationMessagesListCommand,
    ) -> Result<
        (
            Vec<lotta_store::query::ProjectedMessage>,
            Option<String>,
            bool,
        ),
        String,
    > {
        let conversation_id = parse_conversation(&command.conversation_id, MESSAGES_FAILURE)?;
        let query = command.query.clone().unwrap_or_default();
        let agent_id = match self
            .resolve(query.agent_id.as_deref(), &conversation_id)
            .await
        {
            Lookup::Found(found) => found.agent_id,
            Lookup::Missing => return Err(CONVERSATION_NOT_FOUND.to_owned()),
            Lookup::Failed => return Err(MESSAGES_FAILURE.to_owned()),
        };
        let wanted = query.limit.unwrap_or(MESSAGE_LIST_DEFAULT_ITEMS);
        let options = message_options(&query, wanted)?;
        let order = options.order;
        let fetched = self
            .store
            .query_messages_for_conversation(&agent_id, &conversation_id, options)
            .await
            .map_err(|_| MESSAGES_FAILURE.to_owned())?;
        let has_more = fetched.len() > wanted;
        let mut messages = fetched;
        messages.truncate(wanted);
        let next_before = oldest_message_id(&messages, order);
        Ok((messages, next_before, has_more))
    }

    async fn compact_core(
        &self,
        command: &ConversationCompactCommand,
    ) -> Result<CompactionOutcome, String> {
        let conversation_id = parse_conversation(&command.conversation_id, COMPACT_FAILURE)?;
        let hint = command.body.as_ref().and_then(|body| body.agent_id.clone());
        let agent_id = match self.resolve(hint.as_deref(), &conversation_id).await {
            Lookup::Found(found) => found.agent_id,
            Lookup::Missing => return Err(CONVERSATION_NOT_FOUND.to_owned()),
            Lookup::Failed => return Err(COMPACT_FAILURE.to_owned()),
        };
        let scope = RuntimeScope::new(agent_id, conversation_id, None);
        let lifecycle = self.acquire_lifecycle(&scope);
        let lease = lock_lifecycle(&lifecycle)
            .start_command()
            .map_err(|_| CONVERSATION_BUSY.to_owned())?;
        let outcome = self.run_service_compact(&scope, &lease).await;
        let _release = lock_lifecycle(&lifecycle).finish_command(&lease);
        outcome
    }

    /// Runs one manual compaction through the Task 58 lease-serialized
    /// service; the transcript is never written outside its effects.
    async fn run_service_compact(
        &self,
        scope: &RuntimeScope,
        lease: &TurnLease,
    ) -> Result<CompactionOutcome, String> {
        let summary = Arc::new(Mutex::new(None));
        let effects = StoreCompactionEffects {
            store: self.store.clone(),
            leases: Arc::clone(&self.compaction_leases),
            summary: Arc::clone(&summary),
            clock: Arc::clone(&self.clock),
        };
        let service = CompactionService::new(TranscriptSummarizer, effects);
        let command = self.build_compaction_command(scope, lease).await?;
        let progress = service
            .compact(command)
            .await
            .map_err(|_| COMPACT_FAILURE.to_owned())?;
        let recorded = lock_summary(&summary).clone().unwrap_or_default();
        Ok(CompactionOutcome {
            num_messages_before: progress.messages_before,
            num_messages_after: progress.messages_after,
            summary: recorded,
        })
    }

    async fn build_compaction_command(
        &self,
        scope: &RuntimeScope,
        lease: &TurnLease,
    ) -> Result<CompactionCommand, String> {
        let loaded = self
            .store
            .load_transcript(&scope.agent_id, &scope.conversation_id)
            .await
            .map_err(|_| COMPACT_FAILURE.to_owned())?;
        let mut messages = Vec::new();
        for message in loaded.messages() {
            messages.push(provider_message(message)?);
        }
        let request = provider_request(ProviderMessages::new(messages).map_err(|_| {
            tracing::warn!("compaction history exceeded the provider message bound");
            COMPACT_FAILURE.to_owned()
        })?)?;
        let detail = ProviderContextOverflowDetail {
            measured: None,
            estimated: estimate_request_tokens(&request),
            limit: COMPACTION_CONTEXT_WINDOW_FALLBACK_TOKENS,
            provider: LOCAL_PROVIDER_LABEL.to_owned(),
            model: LOCAL_MODEL_LABEL.to_owned(),
            attempt: 1,
            compactions_completed: 0,
        };
        Ok(CompactionCommand {
            request_id: NonEmptyString::new(format!(
                "compact-{}",
                new_uuid().map_err(|()| COMPACT_FAILURE.to_owned())?
            ))
            .map_err(|_| COMPACT_FAILURE.to_owned())?,
            scope: scope.clone(),
            lease: lease.clone(),
            mode: CompactionMode::default(),
            trigger: CompactionTrigger::Manual,
            request,
            detail,
            cancellation: CancellationToken::new(),
        })
    }

    fn acquire_lifecycle(&self, scope: &RuntimeScope) -> Arc<Mutex<TurnLifecycle>> {
        let mut registry = match self.compaction_leases.lock() {
            Ok(registry) => registry,
            Err(poisoned) => poisoned.into_inner(),
        };
        registry
            .entry(scope.clone())
            .or_insert_with(|| Arc::new(Mutex::new(TurnLifecycle::new(uuid_from_random()))))
            .clone()
    }

    /// Occupies one scope's compaction command lease until the returned guard
    /// drops (test seam proving lease serialization).
    #[cfg(test)]
    pub(crate) fn test_hold_lease(&self, scope: &RuntimeScope) -> Result<LeaseGuard, String> {
        let lifecycle = self.acquire_lifecycle(scope);
        let lease = lifecycle
            .lock()
            .expect("lease lifecycle")
            .start_command()
            .map_err(|_| CONVERSATION_BUSY.to_owned())?;
        Ok(LeaseGuard { lifecycle, lease })
    }

    async fn require_agent(&self, agent_id: &AgentId) -> Result<(), String> {
        match AgentStore::load(&self.store, agent_id).await {
            Ok(_) => Ok(()),
            Err(lotta_runtime::RuntimeError::NotFound { .. }) => Err(AGENT_NOT_FOUND.to_owned()),
            Err(_) => Err(CREATE_FAILURE.to_owned()),
        }
    }

    async fn count_conversations(
        &self,
        agent_id: &AgentId,
        failure: &'static str,
    ) -> Result<usize, String> {
        let mut collected = Vec::new();
        self.collect_agent_conversations(agent_id, Some(true), failure, &mut collected)
            .await?;
        Ok(collected.len())
    }

    /// Resolves one bare conversation identifier across candidate agent
    /// scopes: the optional hint first, then every stored agent, mirroring the
    /// baseline's global conversation scan.
    async fn resolve(&self, hint: Option<&str>, conversation_id: &ConversationId) -> Lookup {
        let mut saw_failure = false;
        for agent in self
            .candidate_agents(hint, RETRIEVE_FAILURE)
            .await
            .unwrap_or_default()
        {
            match self.store.query_conversation(&agent, conversation_id).await {
                Ok(found) => return Lookup::Found(Box::new(found)),
                Err(error) if error.kind() == lotta_store::StoreErrorKind::NotFound => {}
                Err(_) => saw_failure = true,
            }
        }
        if saw_failure {
            Lookup::Failed
        } else {
            Lookup::Missing
        }
    }

    async fn candidate_agents(
        &self,
        hint: Option<&str>,
        failure: &'static str,
    ) -> Result<Vec<AgentId>, String> {
        let mut agents = Vec::new();
        if let Some(text) = hint {
            agents.push(parse_agent(Some(text))?);
        }
        let listed = self
            .store
            .query_agents(
                lotta_store::query::AgentFilters::default(),
                PageRequest::default(),
            )
            .await
            .map_err(|_| failure.to_owned())?;
        for agent in listed.items {
            if !agents.iter().any(|seen| seen == &agent.id) {
                agents.push(agent.id);
            }
        }
        Ok(agents)
    }

    async fn next_conversation_id(
        &self,
        agent: &AgentId,
        failure: &'static str,
    ) -> Result<ConversationId, String> {
        let base = self.count_conversations(agent, failure).await?;
        for offset in 0..NEW_ID_ATTEMPTS_MAX {
            let sequence = u64::try_from(base + offset + 1).map_err(|_| failure.to_owned())?;
            if let Ok(id) = ConversationId::generate(sequence)
                && !self.conversation_exists(agent, &id)
            {
                return Ok(id);
            }
        }
        Err(failure.to_owned())
    }

    fn conversation_exists(&self, agent: &AgentId, conversation_id: &ConversationId) -> bool {
        self.store
            .paths()
            .conversation_dir(agent, conversation_id)
            .is_ok_and(|directory| directory.join("conversation.json").exists())
    }

    async fn build_new_conversation(
        &self,
        agent_id: &AgentId,
        body: &ConversationCreateBody,
    ) -> Result<Conversation, String> {
        let now = self.clock.now();
        let id = self.next_conversation_id(agent_id, CREATE_FAILURE).await?;
        let tags = match &body.tags {
            Some(tags) => Some(BoundedVec::new(tags.clone()).map_err(|_| CREATE_FAILURE)?),
            None => None,
        };
        Ok(Conversation {
            id,
            agent_id: agent_id.clone(),
            archived: false,
            archived_at: Some(None),
            created_at: now,
            updated_at: now,
            last_message_at: Some(None),
            summary: Some(body.summary.clone()),
            in_context_message_ids: BoundedVec::new(Vec::new()).map_err(|_| CREATE_FAILURE)?,
            model: body.model.clone().map(Some),
            model_settings: body.model_settings.clone(),
            context_window_limit: body.context_window_limit,
            hidden: body.hidden,
            tags,
            extras: EntityExtras::default(),
        })
    }

    /// Renders the conversation prompt through the Task 30 compiler, either
    /// touching the Task 54 cache record or rendering a dry-run copy only.
    async fn compile_prompt(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
        dry_run: bool,
        failure: &'static str,
    ) -> Result<String, String> {
        let agent = AgentStore::load(&self.store, agent_id)
            .await
            .map_err(|_| failure.to_owned())?;
        let inputs = PromptInputs::new(
            PromptText::new(agent.system.clone()).map_err(|_| failure.to_owned())?,
            agent_id.clone(),
            conversation_id.clone(),
            0,
            self.clock.now(),
            PromptSections::default(),
        )
        .map_err(|_| failure.to_owned())?;
        let compiler = PromptCompiler::new(&self.memfs);
        if dry_run {
            let record = compiler
                .compile(&inputs, CancellationToken::new())
                .await
                .map_err(|_| failure.to_owned())?;
            return Ok(record.content);
        }
        let directory = self.prompt_cache_root(agent_id, conversation_id, failure)?;
        let delivery = directory
            .get_or_compile(
                &compiler,
                &inputs,
                DeliveryCapability::RequestBoundaryOnly,
                CancellationToken::new(),
            )
            .await
            .map_err(|_| failure.to_owned())?;
        Ok(delivery.persisted.content)
    }

    fn prompt_cache_root(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
        failure: &'static str,
    ) -> Result<CacheRoot, String> {
        let directory = self
            .store
            .paths()
            .conversation_dir(agent_id, conversation_id)
            .map_err(|_| failure.to_owned())?;
        std::fs::create_dir_all(&directory).map_err(|_| failure.to_owned())?;
        let canonical = std::fs::canonicalize(&directory).map_err(|_| failure.to_owned())?;
        CacheRoot::new(&canonical).map_err(|_| failure.to_owned())
    }

    fn emit(&self, connection: ConnectionId, message: ConversationsMessage) {
        match serde_json::to_string(&message) {
            Ok(body) if body.len() <= WS_FRAME_BYTES_MAX => {
                let _ = (self.forward)(connection, message);
            }
            Ok(_) => {
                tracing::warn!("conversations response exceeded the frame bound and was dropped");
            }
            Err(_) => tracing::warn!("conversations response failed to encode"),
        }
    }

    fn emit_snapshot_list(
        &self,
        connection: ConnectionId,
        command: &ConversationListCommand,
        outcome: Result<Vec<Conversation>, String>,
    ) {
        let message = match outcome {
            Ok(conversations) => ConversationsMessage::List(ConversationListResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                conversations,
                error: None,
            }),
            Err(detail) => ConversationsMessage::List(ConversationListResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                conversations: Vec::new(),
                error: Some(detail),
            }),
        };
        self.emit(connection, message);
    }

    /// Builds one retrieve/create/update response payload from `outcome`.
    fn snapshot_payload(
        request_id: &str,
        outcome: Result<Conversation, String>,
    ) -> ConversationSnapshotResponseMessage {
        let (success, conversation, error) = match outcome {
            Ok(conversation) => (true, Some(conversation), None),
            Err(detail) => (false, None, Some(detail)),
        };
        ConversationSnapshotResponseMessage {
            request_id: request_id.to_owned(),
            success,
            conversation,
            error,
        }
    }

    fn emit_retrieve(
        &self,
        connection: ConnectionId,
        request_id: &str,
        outcome: Result<Conversation, String>,
    ) {
        self.emit(
            connection,
            ConversationsMessage::Retrieve(Self::snapshot_payload(request_id, outcome)),
        );
    }

    fn emit_create(
        &self,
        connection: ConnectionId,
        request_id: &str,
        outcome: Result<Conversation, String>,
    ) {
        self.emit(
            connection,
            ConversationsMessage::Create(Self::snapshot_payload(request_id, outcome)),
        );
    }

    fn emit_update(
        &self,
        connection: ConnectionId,
        request_id: &str,
        outcome: Result<Conversation, String>,
    ) {
        self.emit(
            connection,
            ConversationsMessage::Update(Self::snapshot_payload(request_id, outcome)),
        );
    }

    fn emit_recompile(
        &self,
        connection: ConnectionId,
        command: &ConversationRecompileCommand,
        outcome: Result<String, String>,
    ) {
        let (success, result, error) = match outcome {
            Ok(result) => (true, Some(result), None),
            Err(detail) => (false, None, Some(detail)),
        };
        self.emit(
            connection,
            ConversationsMessage::Recompile(ConversationRecompileResponseMessage {
                request_id: command.request_id.clone(),
                success,
                result,
                error,
            }),
        );
    }

    fn emit_fork(
        &self,
        connection: ConnectionId,
        command: &ConversationForkCommand,
        outcome: Result<ForkedConversationReference, String>,
    ) {
        let (success, conversation, error) = match outcome {
            Ok(reference) => (true, Some(reference), None),
            Err(detail) => (false, None, Some(detail)),
        };
        self.emit(
            connection,
            ConversationsMessage::Fork(ConversationForkResponseMessage {
                request_id: command.request_id.clone(),
                success,
                conversation,
                error,
            }),
        );
    }

    fn emit_messages(
        &self,
        connection: ConnectionId,
        command: &ConversationMessagesListCommand,
        outcome: Result<
            (
                Vec<lotta_store::query::ProjectedMessage>,
                Option<String>,
                bool,
            ),
            String,
        >,
    ) {
        let (success, messages, next_before, has_more, error) = match outcome {
            Ok((messages, next_before, has_more)) => (true, messages, next_before, has_more, None),
            Err(detail) => (false, Vec::new(), None, false, Some(detail)),
        };
        self.emit(
            connection,
            ConversationsMessage::MessagesList(ConversationMessagesListResponseMessage {
                request_id: command.request_id.clone(),
                success,
                messages,
                next_before,
                has_more,
                error,
            }),
        );
    }

    fn emit_compact(
        &self,
        connection: ConnectionId,
        command: &ConversationCompactCommand,
        outcome: Result<CompactionOutcome, String>,
    ) {
        let (success, compaction, error) = match outcome {
            Ok(compaction) => (true, Some(compaction), None),
            Err(detail) => (false, None, Some(detail)),
        };
        self.emit(
            connection,
            ConversationsMessage::Compact(ConversationCompactResponseMessage {
                request_id: command.request_id.clone(),
                success,
                compaction,
                error,
            }),
        );
    }
}

const TRANSCRIPT_MANIFEST_SCHEMA_VERSION: u8 = 2;
const TRANSCRIPT_SESSION_SCHEMA_VERSION: u8 = 3;
const LOCAL_PROVIDER_LABEL: &str = "local";
const LOCAL_MODEL_LABEL: &str = "local/default";

/// Guard finishing one held scope command lease on drop (test seam).
#[cfg(test)]
pub(crate) struct LeaseGuard {
    lifecycle: Arc<Mutex<TurnLifecycle>>,
    lease: TurnLease,
}

#[cfg(test)]
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let _ = self
            .lifecycle
            .lock()
            .expect("lease lifecycle")
            .finish_command(&self.lease);
    }
}

/// Locks one lifecycle without poisoning leakage (agents-group pattern).
fn lock_lifecycle(lifecycle: &Mutex<TurnLifecycle>) -> std::sync::MutexGuard<'_, TurnLifecycle> {
    match lifecycle.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn lock_summary(summary: &Mutex<Option<String>>) -> std::sync::MutexGuard<'_, Option<String>> {
    match summary.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Per-scope command lease holder backing the Task 58 `lease_is_current`
/// checks while a manual management compaction executes.
struct StoreCompactionEffects {
    store: LocalStore,
    leases: LeaseRegistry,
    summary: Arc<Mutex<Option<String>>>,
    clock: Arc<dyn Clock + Send + Sync>,
}

/// Deterministic extractive summarizer standing in for the provider port the
/// compatibility listener does not carry.
///
/// # Errors
/// Returns adapter failure when there is nothing eligible to summarize.
struct TranscriptSummarizer;

impl CompactionEffects for StoreCompactionEffects {
    fn claim(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<CompactionRecovery, RuntimeError>> + Send + '_>> {
        let claimed = self
            .store
            .claim_compaction(&command.scope, &command.request_id)
            .map_err(RuntimeError::from);
        Box::pin(async move {
            let (_, transaction) = claimed?;
            recovery_of(&transaction)
        })
    }

    fn record_projection(
        &self,
        command: &CompactionCommand,
        summary: CompactionSummary,
        retained: Vec<ProviderMessage>,
        progress: lotta_runtime::turn::CompactionProgress,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        let store = self.store.clone();
        let command = command.clone();
        let sink = Arc::clone(&self.summary);
        Box::pin(async move {
            record_projection_inner(&store, &command, &summary, retained.len(), progress).await?;
            *lock_summary(&sink) = Some(summary.0);
            Ok(())
        })
    }

    fn lease_is_current(&self, scope: &RuntimeScope, lease: &TurnLease) -> bool {
        lease_is_current(&self.leases, scope, lease)
    }

    fn pre_compact(
        &self,
        _command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn post_compact(
        &self,
        _command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn append(
        &self,
        command: &CompactionCommand,
        summary: CompactionSummary,
        _retained: Vec<ProviderMessage>,
        progress: lotta_runtime::turn::CompactionProgress,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        let store = self.store.clone();
        let command = command.clone();
        Box::pin(async move { append_compaction_entry(&store, &command, &summary, progress).await })
    }

    fn publish(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        let store = self.store.clone();
        let command = command.clone();
        let now = self.clock.now();
        Box::pin(async move { publish_compaction_context(&store, &command, now).await })
    }
}

impl lotta_runtime::CompactionSummarizer for TranscriptSummarizer {
    fn summarize(
        &self,
        _request: ProviderRequest,
        messages: Vec<ProviderMessage>,
        _cancellation: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<CompactionSummary, RuntimeError>> + Send + '_>> {
        Box::pin(async move {
            let sections = messages.iter().map(summary_excerpt).collect::<Vec<_>>();
            let summary = sections.join("\n");
            if summary.is_empty() {
                return Err(RuntimeError::AdapterFailure {
                    code: "conversations_compaction_summary",
                    context: "no eligible history".into(),
                });
            }
            Ok(CompactionSummary(summary))
        })
    }
}

/// Bounded plain-text excerpt of one provider message.
fn summary_excerpt(message: &ProviderMessage) -> String {
    message
        .content
        .as_slice()
        .iter()
        .take(SUMMARY_PARTS_PER_MESSAGE_MAX)
        .filter_map(|part| match part {
            ProviderContentPart::Text(text) => Some(excerpt(text.as_str())),
            ProviderContentPart::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn lease_is_current(leases: &LeaseRegistry, scope: &RuntimeScope, lease: &TurnLease) -> bool {
    leases
        .lock()
        .ok()
        .and_then(|registry| registry.get(scope).cloned())
        .is_some_and(|lifecycle| lifecycle.lock().is_ok_and(|owner| owner.is_current(lease)))
}

fn recovery_of(transaction: &CompactionTransaction) -> Result<CompactionRecovery, RuntimeError> {
    let progress =
        transaction
            .projection
            .as_ref()
            .map(|projection| lotta_runtime::turn::CompactionProgress {
                tokens_before: projection.tokens_before,
                tokens_after: projection.tokens_after,
                messages_before: projection.messages_before,
                messages_after: projection.messages_after,
            });
    match (&transaction.state, progress) {
        (CompactionTransactionState::Pending, None) => Ok(CompactionRecovery::Pending),
        (state, Some(counts)) => Ok(match state {
            CompactionTransactionState::Pending => CompactionRecovery::Planned(counts),
            CompactionTransactionState::Appended { .. } => CompactionRecovery::Appended(counts),
            CompactionTransactionState::Published => CompactionRecovery::Published(counts),
        }),
        (_, None) => Err(adapter_error("projection missing")),
    }
}

async fn record_projection_inner(
    store: &LocalStore,
    command: &CompactionCommand,
    summary: &CompactionSummary,
    retained_count: usize,
    progress: lotta_runtime::turn::CompactionProgress,
) -> Result<(), RuntimeError> {
    let current = current_transaction(store, command)?;
    let loaded = store
        .load_transcript(&command.scope.agent_id, &command.scope.conversation_id)
        .await
        .map_err(RuntimeError::from)?;
    let retained_message_ids = loaded
        .messages()
        .iter()
        .rev()
        .take(retained_count)
        .map(|message| message.id.as_str().to_owned())
        .collect::<Vec<_>>();
    store
        .record_compaction_projection(
            current.revision,
            &command.scope,
            &command.request_id,
            CompactionProjection {
                summary: summary.0.clone(),
                retained_message_ids: retained_message_ids
                    .into_iter()
                    .map(NonEmptyString::new)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| adapter_error("retained id"))?,
                tokens_before: progress.tokens_before,
                tokens_after: progress.tokens_after,
                messages_before: progress.messages_before,
                messages_after: progress.messages_after,
            },
        )
        .map_err(RuntimeError::from)?;
    Ok(())
}

async fn append_compaction_entry(
    store: &LocalStore,
    command: &CompactionCommand,
    summary: &CompactionSummary,
    progress: lotta_runtime::turn::CompactionProgress,
) -> Result<(), RuntimeError> {
    let current = current_transaction(store, command)?;
    if matches!(current.state, CompactionTransactionState::Appended { .. }) {
        return Ok(());
    }
    let projection = current
        .projection
        .clone()
        .ok_or_else(|| adapter_error("projection missing"))?;
    let summary_text = if summary.0.is_empty() {
        projection.summary.clone()
    } else {
        summary.0.clone()
    };
    let entry = compaction_entry(command, &summary_text, &projection, progress)?;
    store
        .append_compaction_if_absent(
            current.revision,
            &command.scope,
            &command.request_id,
            &entry,
        )
        .await
        .map_err(RuntimeError::from)?;
    Ok(())
}

async fn publish_compaction_context(
    store: &LocalStore,
    command: &CompactionCommand,
    now: Timestamp,
) -> Result<(), RuntimeError> {
    let current = current_transaction(store, command)?;
    if matches!(current.state, CompactionTransactionState::Published) {
        return Ok(());
    }
    let projection = current
        .projection
        .clone()
        .ok_or_else(|| adapter_error("projection missing"))?;
    let mut ids = vec![
        MessageId::accept(format!("{}-summary", command.request_id.as_str()))
            .map_err(|_| adapter_error("summary id"))?,
    ];
    ids.extend(
        projection
            .retained_message_ids
            .iter()
            .map(|id| MessageId::accept(id.as_str().to_owned()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| adapter_error("retained id"))?,
    );
    store
        .publish_compaction_context(
            &command.scope.agent_id,
            &command.scope.conversation_id,
            projection.summary.clone(),
            InContextMessageIds::new(ids).map_err(|_| adapter_error("context ids"))?,
            now,
        )
        .await
        .map_err(RuntimeError::from)?;
    store
        .mark_compaction_published(current.revision, &command.scope, &command.request_id)
        .map_err(RuntimeError::from)?;
    Ok(())
}

fn current_transaction(
    store: &LocalStore,
    command: &CompactionCommand,
) -> Result<CompactionTransaction, RuntimeError> {
    store
        .compaction_transaction(&command.scope, &command.request_id)
        .map_err(RuntimeError::from)?
        .ok_or_else(|| adapter_error("transaction missing"))
}

fn compaction_entry(
    command: &CompactionCommand,
    summary_text: &str,
    projection: &CompactionProjection,
    progress: lotta_runtime::turn::CompactionProgress,
) -> Result<TranscriptEntry, RuntimeError> {
    let message = LocalMessage {
        id: MessageId::accept(format!("{}-summary", command.request_id.as_str()))
            .map_err(|_| adapter_error("summary message id"))?,
        role: LocalMessageRole::User,
        content: Some(
            lotta_domain::BoundedJsonValue::new(serde_json::json!(summary_text))
                .map_err(|_| adapter_error("summary content"))?,
        ),
        timestamp: summary_stamp_millis()?,
        metadata: None,
        extras: EntityExtras::default(),
    };
    Ok(TranscriptEntry::Compaction(CompactionEntry {
        entry_type: CompactionEntryType::Compaction,
        id: command.request_id.clone(),
        parent_id: None,
        timestamp: Timestamp::from_utc(chrono::Utc::now()),
        summary: summary_text.to_owned(),
        first_kept_entry_id: projection
            .retained_message_ids
            .first()
            .map(|id| id.as_str().to_owned()),
        tokens_before: progress.tokens_before,
        tokens_after: Some(progress.tokens_after),
        messages_before: Some(progress.messages_before),
        messages_after: Some(progress.messages_after),
        message,
        details: None,
    }))
}

fn provider_request(messages: ProviderMessages) -> Result<ProviderRequest, String> {
    Ok(ProviderRequest {
        model: lotta_domain::ModelDescriptor {
            provider_id: NonEmptyString::new(LOCAL_PROVIDER_LABEL.to_owned())
                .map_err(|_| COMPACT_FAILURE.to_owned())?,
            handle: NonEmptyString::new(LOCAL_MODEL_LABEL.to_owned())
                .map_err(|_| COMPACT_FAILURE.to_owned())?,
            available: true,
            context_window: Some(COMPACTION_CONTEXT_WINDOW_FALLBACK_TOKENS),
            model_settings: None,
        },
        messages,
        system_prompt: None,
        tools: ProviderTools::new(Vec::new()).map_err(|_| COMPACT_FAILURE.to_owned())?,
        tool_choice: ProviderToolChoice::None,
        image_policy: ImagePolicy::Strict,
        context_tokens_max: TokenLimit::new(COMPACTION_CONTEXT_WINDOW_FALLBACK_TOKENS)
            .map_err(|_| COMPACT_FAILURE.to_owned())?,
        output_tokens_max: TokenLimit::new(COMPACTION_CONTEXT_WINDOW_FALLBACK_TOKENS)
            .map_err(|_| COMPACT_FAILURE.to_owned())?,
        reasoning: ReasoningControls {
            enabled: false,
            effort: None,
            tier: None,
        },
        context: None,
        cancellation: CancellationToken::new(),
        deadline: ProviderDeadline::default(),
    })
}

fn provider_message(message: &LocalMessage) -> Result<ProviderMessage, String> {
    let role = match message.role {
        LocalMessageRole::User => ProviderMessageRole::User,
        LocalMessageRole::Assistant => ProviderMessageRole::Assistant,
        LocalMessageRole::ToolResult => ProviderMessageRole::Tool,
    };
    Ok(ProviderMessage {
        role,
        content: ProviderContent::new(vec![ProviderContentPart::Text(
            ProviderText::new(message_text(message)).map_err(|_| COMPACT_FAILURE.to_owned())?,
        )])
        .map_err(|_| COMPACT_FAILURE.to_owned())?,
        tool_call_id: None,
    })
}

/// Extracts bounded plain text from one projected content value.
fn message_text(message: &LocalMessage) -> String {
    let Some(content) = message.content.as_ref() else {
        return String::new();
    };
    match content.as_value() {
        serde_json::Value::String(text) => excerpt(text),
        serde_json::Value::Array(parts) => parts
            .iter()
            .take(SUMMARY_PARTS_PER_MESSAGE_MAX)
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .map(excerpt)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn excerpt(text: &str) -> String {
    text.chars().take(SUMMARY_PART_CHARS_MAX).collect()
}

/// Orders, filters, cursors, and caps one merged conversation listing using
/// the pinned semantics: filter, sort newest-first, slice after the cursor,
/// then truncate.
fn apply_list_options(
    mut collected: Vec<Conversation>,
    query: &ConversationListQuery,
) -> Vec<Conversation> {
    if let Some(search) = query.summary_search.as_deref().map(str::to_lowercase) {
        collected.retain(|conversation| {
            conversation.id.as_str().to_lowercase().contains(&search)
                || conversation
                    .summary
                    .as_ref()
                    .and_then(|summary| summary.as_deref())
                    .is_some_and(|summary| summary.to_lowercase().contains(&search))
        });
    }
    collected.sort_by(list_order);
    if let Some(after) = query.after.as_deref()
        && let Some(index) = collected
            .iter()
            .position(|conversation| conversation.id.as_str() == after)
    {
        collected.drain(..=index);
    }
    let limit = query
        .limit
        .unwrap_or(CONVERSATION_LIST_DEFAULT_ITEMS)
        .min(QUERY_PAGE_ITEMS_MAX);
    collected.truncate(limit);
    collected
}

fn list_order(left: &Conversation, right: &Conversation) -> std::cmp::Ordering {
    let left_date = left.last_message_at.flatten().unwrap_or(left.updated_at);
    let right_date = right.last_message_at.flatten().unwrap_or(right.updated_at);
    right_date
        .cmp(&left_date)
        .then_with(|| left.id.cmp(&right.id))
}

/// Overlays one validated update body onto the loaded snapshot; absent fields
/// stay unchanged and explicit nulls clear their nullable counterparts.
fn apply_update(
    mut current: Conversation,
    body: &ConversationUpdateBody,
    now: Timestamp,
) -> Result<Conversation, String> {
    if let Some(archived) = body.archived {
        current.archived = archived;
        let existing = current.archived_at.flatten();
        current.archived_at = Some(if archived {
            existing.or(Some(now))
        } else {
            None
        });
    }
    if let Some(field) = body.last_message_at.clone() {
        current.last_message_at = Some(match field {
            ExplicitField::Null => None,
            ExplicitField::Value(text) => Some(
                Timestamp::parse_persisted_rfc3339(&text).map_err(|_| UPDATE_FAILURE.to_owned())?,
            ),
        });
    }
    if let Some(field) = body.summary.clone() {
        current.summary = Some(explicit_value(field));
    }
    if let Some(field) = body.model.clone() {
        current.model = Some(explicit_value(field));
    }
    if let Some(settings) = &body.model_settings {
        current.model_settings = Some(settings.clone());
    }
    if body.context_window_limit.is_some() {
        current.context_window_limit = body.context_window_limit;
    }
    if body.hidden.is_some() {
        current.hidden = body.hidden;
    }
    if let Some(tags) = &body.tags {
        current.tags = Some(BoundedVec::new(tags.clone()).map_err(|_| UPDATE_FAILURE)?);
    }
    current.updated_at = now;
    Ok(current)
}

fn explicit_value<T>(field: ExplicitField<T>) -> Option<T> {
    match field {
        ExplicitField::Null => None,
        ExplicitField::Value(value) => Some(value),
    }
}

fn message_options(
    query: &ConversationMessagesQuery,
    wanted: usize,
) -> Result<MessageListOptions, String> {
    Ok(MessageListOptions {
        order: if query.order.as_deref() == Some("asc") {
            MessageOrder::Ascending
        } else {
            MessageOrder::Descending
        },
        before: parse_optional_id(query.before.as_deref(), MESSAGES_FAILURE)?,
        after: parse_optional_id(query.after.as_deref(), MESSAGES_FAILURE)?,
        limit: Some(wanted.saturating_add(1).min(QUERY_PAGE_ITEMS_MAX)),
        return_types: query
            .include_return_message_types
            .clone()
            .unwrap_or_default(),
    })
}

/// Oldest served identifier per the pinned pagination contract, used as the
/// continuation cursor for loading older history.
fn oldest_message_id(
    messages: &[lotta_store::query::ProjectedMessage],
    order: MessageOrder,
) -> Option<String> {
    let oldest = match order {
        MessageOrder::Ascending => messages.first()?,
        MessageOrder::Descending => messages.last()?,
    };
    Some(oldest.id.as_str().to_owned())
}

fn parse_conversation(value: &str, failure: &'static str) -> Result<ConversationId, String> {
    if value.is_empty() {
        return Err(failure.to_owned());
    }
    ConversationId::accept(value).map_err(|_| failure.to_owned())
}

fn parse_agent(value: Option<&str>) -> Result<AgentId, String> {
    let text = value.ok_or_else(|| AGENT_NOT_FOUND.to_owned())?;
    AgentId::accept(text).map_err(|_| AGENT_NOT_FOUND.to_owned())
}

fn parse_optional_id(
    value: Option<&str>,
    failure: &'static str,
) -> Result<Option<MessageId>, String> {
    value
        .map(|text| MessageId::accept(text).map_err(|_| failure.to_owned()))
        .transpose()
}

fn session_entry(
    conversation_id: &ConversationId,
    now: &Timestamp,
) -> Result<TranscriptEntry, String> {
    Ok(TranscriptEntry::Session(SessionEntry {
        entry_type: SessionEntryType::Session,
        version: TRANSCRIPT_SESSION_SCHEMA_VERSION,
        id: NonEmptyString::new(format!("session-{}", conversation_id.as_str()))
            .map_err(|_| FORK_FAILURE.to_owned())?,
        timestamp: *now,
        cwd: "/".to_owned(),
    }))
}

fn message_entry(
    message: &LocalMessage,
    index: usize,
    now: &Timestamp,
) -> Result<TranscriptEntry, String> {
    Ok(TranscriptEntry::Message(MessageEntry {
        entry_type: MessageEntryType::Message,
        id: NonEmptyString::new(format!("entry-fork-{index}-{}", message.id.as_str()))
            .map_err(|_| FORK_FAILURE.to_owned())?,
        parent_id: None,
        timestamp: *now,
        message: message.clone(),
    }))
}

/// Current Unix time in whole milliseconds, converted without precision casts.
///
/// # Errors
/// Returns adapter failure when the epoch second leaves `i32` range (year 2038).
fn summary_stamp_millis() -> Result<f64, RuntimeError> {
    let now = chrono::Utc::now();
    let seconds = i32::try_from(now.timestamp()).map_err(|_| adapter_error("entry stamp"))?;
    Ok(f64::from(seconds) * 1_000.0 + f64::from(now.timestamp_subsec_millis()))
}

fn adapter_error(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "conversations_compaction",
        context: context.into(),
    }
}

/// Prepares the shared storage root behind both the store and the `MemFS` port.
fn prepare_backend(
    storage_dir: &std::path::Path,
) -> Result<(LocalStore, GitMemFs), AppServerError> {
    std::fs::create_dir_all(storage_dir)
        .map_err(|_| AppServerError::Config("conversations backend unavailable"))?;
    let root = std::fs::canonicalize(storage_dir)
        .map_err(|_| AppServerError::Config("conversations backend unavailable"))?;
    let paths = StorePaths::new(root.clone())
        .map_err(|_| AppServerError::Config("conversations backend unavailable"))?;
    let memfs = GitMemFs::new(root)
        .map_err(|_| AppServerError::Config("conversations backend unavailable"))?;
    Ok((LocalStore::new(paths), memfs))
}

/// Generates one random UUID v4 using the shared secure randomness source.
fn new_uuid() -> Result<uuid::Uuid, ()> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| ())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes))
}

fn uuid_from_random() -> uuid::Uuid {
    new_uuid().unwrap_or_else(|()| uuid::Uuid::nil())
}

/// Forwarder that drops every message (test seam, mirroring the siblings).
#[cfg(test)]
pub(crate) fn inert_forwarder() -> ConversationsForwarder {
    Arc::new(|_, _| Ok(()))
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known conversation
/// commands.
pub fn decode(frame: &DecodedFrame) -> Result<Option<ConversationsCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let parsed = match tag {
        Tag::ConversationList => {
            typed::<ConversationListCommand>(frame).map(ConversationsCommand::List)
        }
        Tag::ConversationRetrieve => {
            typed::<ConversationRetrieveCommand>(frame).map(ConversationsCommand::Retrieve)
        }
        Tag::ConversationCreate => {
            typed::<ConversationCreateCommand>(frame).map(ConversationsCommand::Create)
        }
        Tag::ConversationUpdate => {
            typed::<ConversationUpdateCommand>(frame).map(ConversationsCommand::Update)
        }
        Tag::ConversationRecompile => {
            typed::<ConversationRecompileCommand>(frame).map(ConversationsCommand::Recompile)
        }
        Tag::ConversationFork => {
            typed::<ConversationForkCommand>(frame).map(ConversationsCommand::Fork)
        }
        Tag::ConversationMessagesList => {
            typed::<ConversationMessagesListCommand>(frame).map(ConversationsCommand::MessagesList)
        }
        Tag::ConversationCompact => {
            typed::<ConversationCompactCommand>(frame).map(ConversationsCommand::Compact)
        }
        _ => return Ok(None),
    }?;
    Ok(Some(parsed))
}

fn typed<T: DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ProtocolErrorEnvelope> {
    serde_json::from_value(frame.value.clone()).map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "conversations_command_invalid",
        "invalid conversations command",
        frame.request_id.clone(),
    )
}

#[cfg(test)]
#[path = "conversations_commands_tests.rs"]
mod commands;
#[cfg(test)]
#[path = "conversations_compact_tests.rs"]
mod compact;
#[cfg(test)]
#[path = "conversations_fixture_round_trip_tests.rs"]
mod fixture_round_trip;
#[cfg(test)]
#[path = "conversations_fork_tests.rs"]
mod fork;
#[cfg(test)]
#[path = "conversations_messages_tests.rs"]
mod messages;
#[cfg(test)]
#[path = "conversations_support.rs"]
mod support;
