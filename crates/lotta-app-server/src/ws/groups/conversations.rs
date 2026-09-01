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
    collections::{BTreeMap, HashMap},
    pin::Pin,
    sync::{Arc, Mutex},
};

use chrono::SecondsFormat;
use lotta_domain::{
    AgentId, BoundedMap, BoundedVec, Clock, CompactionEntry, CompactionEntryType, Conversation,
    ConversationId, EntityExtras, InContextMessageIds, LocalMessage, LocalMessageRole,
    MessageEntry, MessageEntryType, MessageId, NonEmptyString, ProviderStack, RuntimeScope,
    SessionEntry, SessionEntryType, Timestamp, TranscriptEntry, TranscriptManifest,
    TranscriptMessageFormat, TurnLease, TurnLifecycle, bounds::UNBOUNDED_MAP_FIELDS_MAX,
};
use lotta_memfs::{
    CacheRoot, CompiledPromptRecord, GitMemFs, PromptCompiler, PromptInputs, PromptSections,
    PromptText,
};
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::{
    COMPACTION_RECENT_PERCENT_DEFAULT, COMPACTION_RECENT_PERCENT_MAX, CompactionCommand,
    CompactionEffects, CompactionMode, CompactionRecovery, CompactionService, CompactionSummary,
    CompactionTrigger, RuntimeError,
    boundary::ProviderText,
    ports::{
        AgentStore, ConversationStore, ImagePolicy, ProviderContent, ProviderContentPart,
        ProviderContextOverflowDetail, ProviderDeadline, ProviderMessage, ProviderMessageRole,
        ProviderMessages, ProviderRequest, ProviderToolChoice, ProviderTools, ReasoningControls,
        TokenLimit, estimate_request_tokens,
    },
};
use lotta_store::{
    CompactionProjection, CompactionTransaction, CompactionTransactionState, LocalStore,
    StorePaths,
    query::{
        ConversationFilters, MessageListOptions, MessageOrder, PageRequest, QUERY_PAGE_ITEMS_MAX,
        ReturnMessageType, TriState,
    },
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::future::Future;
use tokio_util::sync::CancellationToken;

use super::agents::{COMPACTION_MODE_REJECTED, ExplicitField};
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
const OPENAI_FORK_SOURCE_FIELD: &str = "openai_fork_source_conversation_id";
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
    /// Conversation-level model override; explicit null stays distinct from
    /// absence like the canonical `Conversation` schema.
    #[serde(
        default,
        deserialize_with = "super::agents::explicit_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub model: Option<ExplicitField<String>>,
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
    /// Included return-message categories as pinned message-type strings;
    /// absence includes all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_return_message_types: Option<Vec<PinnedMessageType>>,
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
    /// Optional compaction settings record; sent keys override the owning
    /// agent's stored settings when selecting the compaction strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_settings: Option<BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>,
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

// ── Pinned stored-message wire models ───────────────────────────────────────

/// Return-message categories spelled exactly like the pinned protocol's
/// `message_type` strings.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PinnedMessageType {
    /// User content.
    UserMessage,
    /// Assistant text content.
    AssistantMessage,
    /// Assistant reasoning content.
    ReasoningMessage,
    /// Assistant tool call awaiting its result.
    ApprovalRequestMessage,
    /// Completed tool result.
    ToolReturnMessage,
    /// Compaction summary.
    SummaryMessage,
}

impl PinnedMessageType {
    fn category(self) -> ReturnMessageType {
        match self {
            Self::UserMessage => ReturnMessageType::User,
            Self::AssistantMessage => ReturnMessageType::Assistant,
            Self::ReasoningMessage => ReturnMessageType::Reasoning,
            Self::ApprovalRequestMessage => ReturnMessageType::ApprovalRequest,
            Self::ToolReturnMessage => ReturnMessageType::ToolReturn,
            Self::SummaryMessage => ReturnMessageType::Summary,
        }
    }
}

/// Type-specific pinned payload flattened into one stored-message object.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "message_type")]
pub enum PinnedMessageKind {
    /// `user_message` — user turn with canonical content parts.
    #[serde(rename = "user_message")]
    User {
        /// Sender role label.
        role: &'static str,
        /// Canonical content parts.
        content: serde_json::Value,
    },
    /// `assistant_message` — assistant text parts.
    #[serde(rename = "assistant_message")]
    Assistant {
        /// Sender role label.
        role: &'static str,
        /// Text content parts.
        content: Vec<PinnedTextPart>,
    },
    /// `reasoning_message` — joined assistant reasoning text.
    #[serde(rename = "reasoning_message")]
    Reasoning {
        /// Joined reasoning text.
        reasoning: String,
    },
    /// `approval_request_message` — one pending tool call.
    #[serde(rename = "approval_request_message")]
    ApprovalRequest {
        /// The pending tool call record.
        tool_call: PinnedToolCall,
    },
    /// `tool_return_message` — one completed tool result.
    #[serde(rename = "tool_return_message")]
    ToolReturn {
        /// Identifier of the answered tool call.
        tool_call_id: Option<String>,
        /// Delivery status label.
        status: &'static str,
        /// Raw returned payload.
        tool_return: serde_json::Value,
    },
    /// `summary_message` — compaction summary text.
    #[serde(rename = "summary_message")]
    Summary {
        /// Recorded summary text.
        summary: String,
    },
}

/// One pinned text content part.
#[derive(Clone, Debug, Serialize)]
pub struct PinnedTextPart {
    /// Part discriminator.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Text bytes.
    pub text: String,
}

/// One pinned pending-tool-call record.
#[derive(Clone, Debug, Serialize)]
pub struct PinnedToolCall {
    /// Identifier of the call.
    pub tool_call_id: Option<String>,
    /// Invoked tool name.
    pub name: Option<String>,
    /// JSON-encoded arguments object.
    pub arguments: String,
}

/// One projected conversation message rendered in the exact pinned
/// stored-message shape: flattened identity fields plus a typed payload.
#[derive(Clone, Debug, Serialize)]
pub struct PinnedStoredMessage {
    /// Generated API projection ID.
    pub id: String,
    /// ISO-8601 UTC instant of the source message.
    pub date: String,
    /// Owning agent identifier.
    pub agent_id: String,
    /// Owning conversation identifier.
    pub conversation_id: String,
    /// Type-specific payload carrying the `message_type` tag.
    #[serde(flatten)]
    pub kind: PinnedMessageKind,
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
    /// Projected messages in the requested order, in the pinned shape.
    pub messages: Vec<PinnedStoredMessage>,
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

/// Why one authoritative command lease could not begin on its scope.
#[derive(Clone, Copy, Debug)]
pub struct CommandLeaseUnavailable;

/// Authoritative turn-lifecycle port shared between this group and the
/// production turn pipeline.
///
/// When injected, manual compactions acquire their command lease from the same
/// lifecycle registry the turn path uses — so an active production turn blocks
/// compaction with the busy failure — and execute through the registered
/// production compaction service with its real lifecycle hooks and provider
/// summarization instead of a standalone service.
pub trait ConversationAuthority: Send + Sync {
    /// Begins a command on the authoritative lifecycle for `scope`.
    ///
    /// # Errors
    /// Returns [`CommandLeaseUnavailable`] when a turn or another command
    /// currently owns the scope.
    fn begin_command(&self, scope: &RuntimeScope) -> Result<TurnLease, CommandLeaseUnavailable>;

    /// Releases one previously begun command lease.
    ///
    /// The release awaits the authoritative registry, so it completes
    /// deterministically even while an unrelated operation holds the registry
    /// busy; the scope always returns to idle once this resolves.
    ///
    /// # Errors
    /// Returns the production release failure verbatim.
    fn finish_command<'a>(
        &'a self,
        scope: &'a RuntimeScope,
        lease: &'a TurnLease,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + 'a>>;

    /// Runs one compaction through the registered production service.
    ///
    /// # Errors
    /// Returns the production compaction failure verbatim.
    fn compact(
        &self,
        command: CompactionCommand,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<lotta_runtime::turn::CompactionProgress, RuntimeError>>
                + Send
                + '_,
        >,
    >;
}

/// Backend capability queried by Responses before any allocation or fork work.
pub trait ConversationCommandRepository: Send + Sync {
    /// Returns whether hidden canonical conversation forks are supported atomically.
    fn supports_hidden_fork(&self) -> bool;

    /// Tests whether one agent owns the referenced stored conversation.
    fn contains_for_openai<'a>(
        &'a self,
        agent_id: &'a AgentId,
        conversation_id: &'a ConversationId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, AppServerError>> + Send + 'a>>;

    /// Creates one hidden canonical fork of a stored conversation.
    fn fork_for_openai<'a>(
        &'a self,
        agent_id: &'a AgentId,
        conversation_id: &'a ConversationId,
    ) -> Pin<Box<dyn Future<Output = Result<ConversationId, AppServerError>> + Send + 'a>>;

    /// Deletes one request-owned conversation and its artifacts.
    fn delete_for_openai<'a>(
        &'a self,
        agent_id: &'a AgentId,
        conversation_id: &'a ConversationId,
    ) -> Pin<Box<dyn Future<Output = Result<(), AppServerError>> + Send + 'a>>;
}

/// Applies wire commands to the Task 23/24 store, Task 28 queries, the Task 30
/// prompt compiler, and the Task 58 compaction service.
pub struct ConversationsBridge {
    store: LocalStore,
    memfs: GitMemFs,
    forward: ConversationsForwarder,
    clock: Arc<dyn Clock + Send + Sync>,
    compaction_leases: LeaseRegistry,
    authority: Option<Arc<dyn ConversationAuthority>>,
}

#[cfg(test)]
enum OpenAiCreatePanic {
    Immediate,
    Gated {
        entered: Arc<tokio::sync::Semaphore>,
        release: Arc<tokio::sync::Semaphore>,
    },
}

#[cfg(test)]
static OPENAI_CREATE_PANIC: std::sync::LazyLock<Mutex<HashMap<String, OpenAiCreatePanic>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
pub(crate) fn panic_next_openai_create_for_agent(agent_id: &str) {
    OPENAI_CREATE_PANIC
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(agent_id.to_owned(), OpenAiCreatePanic::Immediate);
}

#[cfg(test)]
pub(crate) fn gate_next_openai_create_panic(
    agent_id: &str,
) -> (Arc<tokio::sync::Semaphore>, Arc<tokio::sync::Semaphore>) {
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    OPENAI_CREATE_PANIC
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            agent_id.to_owned(),
            OpenAiCreatePanic::Gated {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            },
        );
    (entered, release)
}

impl ConversationCommandRepository for ConversationsBridge {
    fn supports_hidden_fork(&self) -> bool {
        true
    }

    fn contains_for_openai<'a>(
        &'a self,
        agent_id: &'a AgentId,
        conversation_id: &'a ConversationId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, AppServerError>> + Send + 'a>> {
        Box::pin(async move { self.contains_for_openai(agent_id, conversation_id).await })
    }

    fn fork_for_openai<'a>(
        &'a self,
        agent_id: &'a AgentId,
        conversation_id: &'a ConversationId,
    ) -> Pin<Box<dyn Future<Output = Result<ConversationId, AppServerError>> + Send + 'a>> {
        Box::pin(async move { self.fork_for_openai(agent_id, conversation_id).await })
    }

    fn delete_for_openai<'a>(
        &'a self,
        agent_id: &'a AgentId,
        conversation_id: &'a ConversationId,
    ) -> Pin<Box<dyn Future<Output = Result<(), AppServerError>> + Send + 'a>> {
        Box::pin(async move { self.delete_for_openai(agent_id, conversation_id).await })
    }
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
        authority: Option<Arc<dyn ConversationAuthority>>,
    ) -> Result<Self, AppServerError> {
        let (store, memfs) = prepare_backend(storage_dir)?;
        Ok(Self {
            store,
            memfs,
            forward,
            clock,
            compaction_leases: Arc::new(Mutex::new(HashMap::new())),
            authority,
        })
    }

    /// Composes a bridge from explicit parts (test seam).
    #[must_use]
    #[cfg(test)]
    pub(crate) fn compose(
        forward: ConversationsForwarder,
        store: LocalStore,
        memfs: GitMemFs,
        clock: Arc<dyn Clock + Send + Sync>,
        authority: Option<Arc<dyn ConversationAuthority>>,
    ) -> Self {
        Self {
            store,
            memfs,
            forward,
            clock,
            compaction_leases: Arc::new(Mutex::new(HashMap::new())),
            authority,
        }
    }

    /// Creates one conversation through the canonical repository and prompt compiler.
    ///
    /// # Errors
    /// Returns a scrubbed boundary failure if creation or compilation fails.
    ///
    /// # Panics
    /// Test builds may inject a repository panic for allocation-owner recovery coverage.
    pub async fn create_for_openai(
        &self,
        agent_id: &AgentId,
    ) -> Result<ConversationId, AppServerError> {
        #[cfg(test)]
        {
            let injected = OPENAI_CREATE_PANIC
                .lock()
                .ok()
                .and_then(|mut targets| targets.remove(agent_id.as_str()));
            match injected {
                Some(OpenAiCreatePanic::Gated {
                    entered, release, ..
                }) => {
                    entered.add_permits(1);
                    release
                        .acquire()
                        .await
                        .expect("panic gate remains open")
                        .forget();
                    panic!("injected canonical conversation repository panic");
                }
                Some(OpenAiCreatePanic::Immediate) => {
                    panic!("injected canonical conversation repository panic");
                }
                None => {}
            }
        }
        let body = ConversationCreateBody {
            agent_id: Some(agent_id.as_str().to_owned()),
            ..ConversationCreateBody::default()
        };
        self.create_core(&body)
            .await
            .map(|conversation| conversation.id)
            .map_err(|_| AppServerError::Unavailable)
    }

    /// Deletes an ephemeral conversation through the canonical repository.
    ///
    /// # Errors
    /// Returns a scrubbed boundary failure when the durable record cannot be removed.
    pub async fn delete_for_openai(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
    ) -> Result<(), AppServerError> {
        self.delete_conversation_artifacts(agent_id, conversation_id)
            .await
            .map_err(|()| AppServerError::Unavailable)
    }

    /// Confirms that a conversation exists under the exact agent scope.
    ///
    /// # Errors
    /// Returns an opaque repository failure when lookup cannot complete.
    pub async fn contains_for_openai(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
    ) -> Result<bool, AppServerError> {
        match self.resolve(Some(agent_id.as_str()), conversation_id).await {
            Lookup::Found(found) => Ok(found.agent_id == *agent_id),
            Lookup::Missing => Ok(false),
            Lookup::Failed => Err(AppServerError::Unavailable),
        }
    }

    /// Creates a hidden canonical transcript-rewrite fork for Responses.
    ///
    /// # Errors
    /// Returns an opaque repository failure when fork setup cannot complete atomically.
    pub async fn fork_for_openai(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
    ) -> Result<ConversationId, AppServerError> {
        let command = ConversationForkCommand {
            request_id: "openai-response-fork".to_owned(),
            conversation_id: conversation_id.as_str().to_owned(),
            body: Some(ConversationForkBody {
                agent_id: Some(agent_id.as_str().to_owned()),
                hidden: Some(true),
                message_id: None,
            }),
        };
        let forked = self
            .fork_core(&command)
            .await
            .map_err(|_| AppServerError::Unavailable)?;
        ConversationId::accept(forked.id).map_err(|_| AppServerError::Unavailable)
    }

    async fn delete_conversation_artifacts(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
    ) -> Result<(), ()> {
        let directory = self
            .store
            .paths()
            .conversation_dir(agent_id, conversation_id)
            .map_err(|_| ())?;
        ConversationStore::delete(&self.store, agent_id, conversation_id)
            .await
            .map_err(|_| ())?;
        match tokio::fs::remove_dir_all(&directory).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(()),
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
        let snapshot = body.clone();
        let now = self.clock.now();
        // The store allocates a globally unused identifier, enforces the
        // per-agent cap, and inserts the record inside one locked operation.
        let created = {
            let agent = agent_id.clone();
            self.store
                .create_conversation(agent_id.clone(), move |conversation_id| {
                    build_new_conversation(conversation_id, &agent, &snapshot, now)
                        .map_err(|()| bounded_input_failure())
                })
                .await
                .map_err(|error| create_store_failure(&error))?
        };
        // Creation is transactional at this boundary: prompt compilation must
        // succeed before publication, and a failure is compensated by removing
        // the record plus every conversation-owned transcript/cache artifact.
        if self
            .compile_prompt(&agent_id, &created.id, false, CREATE_FAILURE)
            .await
            .is_err()
        {
            self.delete_conversation_artifacts(&agent_id, &created.id)
                .await
                .map_err(|()| CREATE_FAILURE.to_owned())?;
            return Err(CREATE_FAILURE.to_owned());
        }
        Ok(created)
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
        let kept = self
            .fork_history(
                &source_agent,
                &source_id,
                body.message_id.as_deref(),
                FORK_FAILURE,
            )
            .await?;
        // The store allocates the fork identifier, enforces the per-agent cap,
        // and inserts the record inside one locked operation.
        let now = self.clock.now();
        let forked = {
            let agent = target_agent.clone();
            let source = source.clone();
            let kept = kept.clone();
            let agent_for_call = target_agent.clone();
            self.store
                .create_conversation(agent_for_call, move |conversation_id| {
                    build_forked_record(conversation_id, &source, &agent, &kept, body.hidden, now)
                        .map_err(|()| bounded_input_failure())
                })
                .await
                .map_err(|error| fork_store_failure(&error))?
        };
        if self
            .write_fork_transcript(&target_agent, &forked.id, &kept)
            .await
            .is_err()
        {
            self.delete_conversation_artifacts(&target_agent, &forked.id)
                .await
                .map_err(|()| FORK_FAILURE.to_owned())?;
            return Err(FORK_FAILURE.to_owned());
        }
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
        let body = command.body.clone().unwrap_or_default();
        let agent_id = match self
            .resolve(body.agent_id.as_deref(), &conversation_id)
            .await
        {
            Lookup::Found(found) => found.agent_id,
            Lookup::Missing => return Err(CONVERSATION_NOT_FOUND.to_owned()),
            Lookup::Failed => return Err(COMPACT_FAILURE.to_owned()),
        };
        let scope = RuntimeScope::new(agent_id.clone(), conversation_id, None);
        let mode = self
            .compaction_mode(&agent_id, body.compaction_settings.as_ref())
            .await?;
        if let Some(authority) = self.authority.as_ref() {
            // The authoritative lifecycle rejects the command while a
            // production turn owns the scope; otherwise the registered
            // production compaction service runs with real hooks.
            let lease = authority
                .begin_command(&scope)
                .map_err(|CommandLeaseUnavailable| CONVERSATION_BUSY.to_owned())?;
            let outcome = self
                .run_authority_compact(authority, &scope, &lease, mode)
                .await;
            // The release awaits the authoritative registry, so a failed
            // release is real: reporting success would strand the scope busy.
            authority
                .finish_command(&scope, &lease)
                .await
                .map_err(|_| COMPACT_FAILURE.to_owned())?;
            outcome
        } else {
            let lifecycle = self.acquire_lifecycle(&scope);
            let lease = lock_lifecycle(&lifecycle)
                .start_command()
                .map_err(|_| CONVERSATION_BUSY.to_owned())?;
            let outcome = self.run_standalone_compact(&scope, &lease, mode).await;
            let _release = lock_lifecycle(&lifecycle).finish_command(&lease);
            outcome
        }
    }

    /// Runs one `/compact` slash command for a runtime scope through the exact
    /// conversation compaction flow, returning pinned output text.
    ///
    /// The optional argument carries the pinned mode word (`all`,
    /// `sliding_window`, or `help`); unknown modes fail like any rejected
    /// compaction.
    ///
    /// # Errors
    /// Returns the scrubbed compaction failure text on rejection.
    pub async fn compact_runtime_text(
        &self,
        scope: &RuntimeScope,
        args: Option<&str>,
    ) -> Result<String, String> {
        if args.map(str::trim) == Some("help") {
            return Ok(compact_help_output().to_owned());
        }
        let mut settings = BTreeMap::new();
        if let Some(mode) = args.map(str::trim).filter(|mode| !mode.is_empty()) {
            settings.insert("mode".to_owned(), serde_json::json!(mode));
        }
        let command = ConversationCompactCommand {
            request_id: format!("device-compact-{}", scope.conversation_id.as_str()),
            conversation_id: scope.conversation_id.as_str().to_owned(),
            body: Some(ConversationCompactBody {
                agent_id: None,
                compaction_settings: Some(
                    BoundedMap::new(settings).map_err(|_| COMPACT_FAILURE.to_owned())?,
                ),
            }),
        };
        let outcome = self.compact_core(&command).await?;
        Ok(format!(
            "Compacted {} messages into {}",
            outcome.num_messages_before, outcome.num_messages_after
        ))
    }

    /// Routes one manual compaction through the injected authoritative
    /// production service.
    async fn run_authority_compact(
        &self,
        authority: &Arc<dyn ConversationAuthority>,
        scope: &RuntimeScope,
        lease: &TurnLease,
        mode: CompactionMode,
    ) -> Result<CompactionOutcome, String> {
        let command = self.build_compaction_command(scope, lease, mode).await?;
        let progress = authority
            .compact(command.clone())
            .await
            .map_err(|_| COMPACT_FAILURE.to_owned())?;
        self.outcome_of(&command, progress)
    }

    /// Runs one manual compaction through the standalone Task 58
    /// lease-serialized service; the transcript is never written outside its
    /// effects. Used only when no production runtime is reachable.
    async fn run_standalone_compact(
        &self,
        scope: &RuntimeScope,
        lease: &TurnLease,
        mode: CompactionMode,
    ) -> Result<CompactionOutcome, String> {
        let effects = StoreCompactionEffects {
            store: self.store.clone(),
            leases: Arc::clone(&self.compaction_leases),
            clock: Arc::clone(&self.clock),
        };
        let service = CompactionService::new(TranscriptSummarizer, effects);
        let command = self.build_compaction_command(scope, lease, mode).await?;
        let progress = service
            .compact(command.clone())
            .await
            .map_err(|_| COMPACT_FAILURE.to_owned())?;
        self.outcome_of(&command, progress)
    }

    /// Builds the pinned compaction outcome from the durable journal projection.
    fn outcome_of(
        &self,
        command: &CompactionCommand,
        progress: lotta_runtime::turn::CompactionProgress,
    ) -> Result<CompactionOutcome, String> {
        let summary = self
            .store
            .compaction_transaction(&command.scope, &command.request_id)
            .map_err(|_| COMPACT_FAILURE.to_owned())?
            .and_then(|transaction| transaction.projection)
            .map(|projection| projection.summary)
            .ok_or_else(|| COMPACT_FAILURE.to_owned())?;
        Ok(CompactionOutcome {
            num_messages_before: progress.messages_before,
            num_messages_after: progress.messages_after,
            summary,
        })
    }

    /// Resolves the pinned compaction strategy: optional sent settings merged
    /// over the owning agent's stored settings, then mapped onto the Task 58
    /// modes (`all`, or `sliding_window` with a retained percentage).
    async fn compaction_mode(
        &self,
        agent_id: &AgentId,
        sent: Option<&BoundedMap<{ UNBOUNDED_MAP_FIELDS_MAX }>>,
    ) -> Result<CompactionMode, String> {
        let stored = AgentStore::load(&self.store, agent_id)
            .await
            .ok()
            .and_then(|agent| agent.compaction_settings.flatten());
        let setting = |key: &str| -> Option<serde_json::Value> {
            sent.and_then(|settings| settings.get(key))
                .cloned()
                .or_else(|| {
                    stored
                        .as_ref()
                        .and_then(|settings| settings.get(key))
                        .cloned()
                })
        };
        let mode = setting("mode").and_then(|value| value.as_str().map(str::to_owned));
        match mode.as_deref() {
            Some("all") => Ok(CompactionMode::All),
            Some("sliding_window") | None => {
                let percent = sliding_window_percent(setting("sliding_window_percentage"))?;
                CompactionMode::sliding_window(percent).map_err(|_| COMPACT_FAILURE.to_owned())
            }
            Some(_) => Err(COMPACTION_MODE_REJECTED.to_owned()),
        }
    }

    async fn build_compaction_command(
        &self,
        scope: &RuntimeScope,
        lease: &TurnLease,
        mode: CompactionMode,
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
            mode,
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

    /// Renders the conversation prompt through the Task 30 compiler.
    ///
    /// Recompilation always renders committed memory afresh — the Task 54
    /// cache record is only written back, never reused — and a dry run skips
    /// the persistence step entirely. The previous-message count reflects the
    /// conversation's current projected history instead of a hard-coded zero.
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
        let previous_message_count = self
            .store
            .query_messages_for_conversation(
                agent_id,
                conversation_id,
                MessageListOptions::default(),
            )
            .await
            .map_err(|_| failure.to_owned())?
            .len();
        let inputs = PromptInputs::new(
            PromptText::new(agent.system.clone()).map_err(|_| failure.to_owned())?,
            agent_id.clone(),
            conversation_id.clone(),
            previous_message_count,
            self.clock.now(),
            PromptSections::default(),
        )
        .map_err(|_| failure.to_owned())?;
        let compiler = PromptCompiler::new(&self.memfs);
        let record = compiler
            .compile(&inputs, CancellationToken::new())
            .await
            .map_err(|_| failure.to_owned())?;
        if dry_run {
            return Ok(record.content);
        }
        let directory = self.prompt_cache_root(agent_id, conversation_id, failure)?;
        persist_prompt_record(&directory, &record).map_err(|_| failure.to_owned())?;
        Ok(record.content)
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
        let mapped = outcome.and_then(|(projected, next_before, has_more)| {
            let messages = projected
                .iter()
                .map(pinned_message)
                .collect::<Result<Vec<_>, String>>()?;
            Ok((messages, next_before, has_more))
        });
        let (success, messages, next_before, has_more, error) = match mapped {
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

/// Per-scope command lease holder backing the Task 58 `lease_is_current`
/// checks while a manual management compaction executes.
struct StoreCompactionEffects {
    store: LocalStore,
    leases: LeaseRegistry,
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
        Box::pin(async move {
            record_projection_inner(&store, &command, &summary, retained.len(), progress).await
        })
    }

    fn lease_is_current(
        &self,
        scope: &RuntimeScope,
        lease: &TurnLease,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let scope = scope.clone();
        let lease = lease.clone();
        Box::pin(async move { lease_is_current(&self.leases, &scope, &lease) })
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

/// Maps one store create failure onto the pinned scrubbed wire detail: the cap
/// rejection keeps its own message while every other failure stays generic.
fn create_store_failure(error: &lotta_store::StoreError) -> String {
    store_failure(error, CREATE_FAILURE)
}

/// Fork twin of [`create_store_failure`].
fn fork_store_failure(error: &lotta_store::StoreError) -> String {
    store_failure(error, FORK_FAILURE)
}

fn store_failure(error: &lotta_store::StoreError, failure: &'static str) -> String {
    if error.kind() == lotta_store::StoreErrorKind::Limit {
        CONVERSATIONS_LIMIT_REACHED.to_owned()
    } else {
        failure.to_owned()
    }
}

/// Synthetic store error for bounded-collection rejections inside a builder.
///
/// The bridge immediately scrubs this into its generic wire detail; the value
/// never outlives the create call.
fn bounded_input_failure() -> lotta_store::StoreError {
    lotta_store::StoreError::new(
        lotta_store::StoreErrorKind::Parse,
        std::path::Path::new("."),
    )
}

/// Builds one canonical new conversation record around an allocated identifier.
fn build_new_conversation(
    conversation_id: ConversationId,
    agent_id: &AgentId,
    body: &ConversationCreateBody,
    now: Timestamp,
) -> Result<Conversation, ()> {
    let tags = body
        .tags
        .as_ref()
        .map(|tags| BoundedVec::new(tags.clone()).map_err(|_| ()))
        .transpose()?;
    Ok(Conversation {
        id: conversation_id,
        agent_id: agent_id.clone(),
        archived: false,
        archived_at: Some(None),
        created_at: now,
        updated_at: now,
        last_message_at: Some(None),
        summary: Some(body.summary.clone()),
        in_context_message_ids: BoundedVec::new(Vec::new()).map_err(|_| ())?,
        // A sent explicit null stays distinct from an absent key like the
        // canonical `Conversation` schema.
        model: body.model.clone().map(explicit_value),
        model_settings: body.model_settings.clone(),
        context_window_limit: body.context_window_limit,
        hidden: body.hidden,
        tags,
        extras: EntityExtras::default(),
    })
}

/// Builds one forked conversation record around an allocated identifier.
fn build_forked_record(
    conversation_id: ConversationId,
    source: &Conversation,
    target_agent: &AgentId,
    kept: &[LocalMessage],
    hidden: Option<bool>,
    now: Timestamp,
) -> Result<Conversation, ()> {
    let context_ids = kept.iter().map(|message| message.id.clone()).collect();
    Ok(Conversation {
        id: conversation_id,
        agent_id: target_agent.clone(),
        archived: false,
        archived_at: Some(None),
        created_at: now,
        updated_at: now,
        last_message_at: source.last_message_at,
        summary: source.summary.clone(),
        in_context_message_ids: BoundedVec::new(context_ids).map_err(|_| ())?,
        model: source.model.clone(),
        model_settings: source.model_settings.clone(),
        context_window_limit: source.context_window_limit,
        hidden: hidden.or(source.hidden),
        tags: source.tags.clone(),
        extras: fork_source_extras(source)?,
    })
}

fn fork_source_extras(source: &Conversation) -> Result<EntityExtras, ()> {
    let values = BTreeMap::from([(
        OPENAI_FORK_SOURCE_FIELD.to_owned(),
        serde_json::Value::String(source.id.as_str().to_owned()),
    )]);
    EntityExtras::new(values, &[]).map_err(|_| ())
}

/// Converts one pinned sliding-window percentage (a fraction in `0..=1`) into
/// the Task 58 retained-percentage bound, defaulting like the pinned backend.
fn sliding_window_percent(sent: Option<serde_json::Value>) -> Result<u8, String> {
    let Some(fraction) = sent.and_then(|value| value.as_f64()) else {
        return Ok(COMPACTION_RECENT_PERCENT_DEFAULT);
    };
    let scaled = fraction * f64::from(COMPACTION_RECENT_PERCENT_MAX);
    if !(0.0..=f64::from(COMPACTION_RECENT_PERCENT_MAX)).contains(&scaled) {
        return Err(COMPACT_FAILURE.to_owned());
    }
    let whole = format!("{:.0}", scaled.round())
        .parse::<u64>()
        .map_err(|_| COMPACT_FAILURE.to_owned())?;
    u8::try_from(whole).map_err(|_| COMPACT_FAILURE.to_owned())
}

/// Persists one freshly rendered prompt record into the conversation cache root.
fn persist_prompt_record(
    directory: &CacheRoot,
    record: &CompiledPromptRecord,
) -> Result<(), RuntimeError> {
    directory.persist(record)
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
            .unwrap_or_default()
            .into_iter()
            .map(PinnedMessageType::category)
            .collect(),
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

/// Renders one projected millisecond stamp as the pinned ISO-8601 UTC date.
fn iso_date(timestamp_ms: f64) -> Result<String, String> {
    let invalid = || MESSAGES_FAILURE.to_owned();
    let rounded = timestamp_ms.round();
    if !rounded.is_finite() || rounded < 0.0 {
        return Err(invalid());
    }
    let millis = format!("{rounded:.0}")
        .parse::<i64>()
        .map_err(|_| invalid())?;
    let seconds = millis.div_euclid(1_000);
    let sub_milli = millis.rem_euclid(1_000);
    let nanos = sub_milli.checked_mul(1_000_000).ok_or_else(invalid)?;
    let stamp_nanos = u32::try_from(nanos).map_err(|_| invalid())?;
    chrono::DateTime::from_timestamp(seconds, stamp_nanos)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or_else(invalid)
}

/// Translates one Task 28 projection into the exact pinned stored-message
/// object: flattened identity fields plus the tagged typed payload.
fn pinned_message(
    projected: &lotta_store::query::ProjectedMessage,
) -> Result<PinnedStoredMessage, String> {
    Ok(PinnedStoredMessage {
        id: projected.id.as_str().to_owned(),
        date: iso_date(projected.timestamp_ms)?,
        agent_id: projected.source.agent_id.as_str().to_owned(),
        conversation_id: projected.source.conversation_id.as_str().to_owned(),
        kind: pinned_kind(projected.message_type, &projected.value)?,
    })
}

/// Builds the pinned typed payload for one projected category.
fn pinned_kind(
    category: ReturnMessageType,
    value: &serde_json::Value,
) -> Result<PinnedMessageKind, String> {
    let missing = || MESSAGES_FAILURE.to_owned();
    Ok(match category {
        ReturnMessageType::User => PinnedMessageKind::User {
            role: "user",
            content: value.get("content").cloned().ok_or_else(missing)?,
        },
        ReturnMessageType::Assistant => PinnedMessageKind::Assistant {
            role: "assistant",
            content: text_parts(value.get("content").ok_or_else(missing)?)?,
        },
        ReturnMessageType::Reasoning => PinnedMessageKind::Reasoning {
            reasoning: value
                .get("reasoning")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(missing)?
                .to_owned(),
        },
        ReturnMessageType::ApprovalRequest => PinnedMessageKind::ApprovalRequest {
            tool_call: pinned_tool_call(value)?,
        },
        ReturnMessageType::ToolReturn => PinnedMessageKind::ToolReturn {
            tool_call_id: value
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            status: if value
                .get("is_error")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                "error"
            } else {
                "success"
            },
            tool_return: value.get("tool_return").cloned().ok_or_else(missing)?,
        },
        ReturnMessageType::Summary => PinnedMessageKind::Summary {
            summary: value
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(missing)?
                .to_owned(),
        },
    })
}

/// Converts projected plain-text entries into pinned `{type,text}` parts.
fn text_parts(value: &serde_json::Value) -> Result<Vec<PinnedTextPart>, String> {
    value
        .as_array()
        .ok_or_else(|| MESSAGES_FAILURE.to_owned())?
        .iter()
        .map(|entry| {
            Ok(PinnedTextPart {
                kind: "text",
                text: entry
                    .as_str()
                    .ok_or_else(|| MESSAGES_FAILURE.to_owned())?
                    .to_owned(),
            })
        })
        .collect()
}

/// Converts one projected raw `toolCall` part into the pinned record with a
/// JSON-stringified arguments object like the pinned backend.
fn pinned_tool_call(value: &serde_json::Value) -> Result<PinnedToolCall, String> {
    let arguments = match value.get("arguments") {
        Some(arguments) if !arguments.is_null() => {
            serde_json::to_string(arguments).map_err(|_| MESSAGES_FAILURE.to_owned())?
        }
        _ => "{}".to_owned(),
    };
    Ok(PinnedToolCall {
        tool_call_id: value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        arguments,
    })
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

/// The pinned `/compact help` text, mirroring the baseline listener verbatim.
fn compact_help_output() -> &'static str {
    "/compact help\n\
     \nSummarize conversation history (compaction).\n\
     \nUSAGE\n  \
     /compact                   — compact with default mode\n  \
     /compact all               — compact all messages\n  \
     /compact sliding_window    — compact with sliding window\n"
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
