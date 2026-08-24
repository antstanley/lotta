use lotta_domain::{
    AgentId, Conversation, ConversationId, LocalMessage, MessageId, RuntimeConnection,
};
use serde::{Deserialize, Serialize};

/// Maximum accepted query string bytes.
pub const QUERY_TEXT_BYTES_MAX: usize = 4_096;
/// Maximum accepted page size.
pub const QUERY_PAGE_ITEMS_MAX: usize = 1_000;
/// Maximum accepted tag filters.
pub const QUERY_TAGS_MAX: usize = 64;
/// Maximum runtime connections inspected by the pure subscriber query.
pub const QUERY_RUNTIME_CONNECTIONS_MAX: usize = 1_024;
/// Maximum persisted records inspected by one query.
pub const QUERY_SCAN_ENTRIES_MAX: usize = 100_000;
/// Maximum resume-tail messages.
pub const RESUME_TAIL_MESSAGES_MAX: usize = 1_000;
/// Maximum projected messages materialized by one query.
pub const QUERY_PROJECTED_MESSAGES_MAX: usize = 1_000;

/// Three-state boolean filter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TriState {
    /// Do not filter this property.
    #[default]
    Any,
    /// Require true.
    True,
    /// Require false or absent.
    False,
}

/// Agent-name comparison mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameMatch {
    /// Exact case-sensitive match.
    Exact,
    /// Case-insensitive substring match.
    Substring,
}

/// Stable message order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MessageOrder {
    /// Oldest first.
    Ascending,
    /// Newest first.
    #[default]
    Descending,
}

/// Projected return-message category.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReturnMessageType {
    /// User content.
    User,
    /// Assistant text content.
    Assistant,
    /// Assistant reasoning content.
    Reasoning,
    /// Assistant tool call.
    ApprovalRequest,
    /// Tool result.
    ToolReturn,
    /// Compaction summary.
    Summary,
}

/// Opaque stable page cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cursor(String);

impl Cursor {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// Creates the continuation cursor for one previously returned item
    /// identifier, as clients echo it back on paginated requests.
    #[must_use]
    pub fn from_item(value: String) -> Self {
        Self(value)
    }

    /// Borrows the opaque cursor.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bounded page request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageRequest {
    /// Maximum returned items; zero is rejected.
    pub limit: Option<usize>,
    /// Continue after this opaque cursor.
    pub after: Option<Cursor>,
}

/// Deterministic query page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page<T> {
    /// Result items.
    pub items: Vec<T>,
    /// Cursor for the next page, only when more items exist.
    pub next: Option<Cursor>,
}

/// Agent list filters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentFilters {
    /// Optional name predicate.
    pub name: Option<(String, NameMatch)>,
    /// Case-insensitive substring over name, description, ID, and model.
    pub query: Option<String>,
    /// All required tags.
    pub tags: Vec<String>,
    /// Hidden-state filter.
    pub hidden: TriState,
}

impl Default for AgentFilters {
    fn default() -> Self {
        Self {
            name: None,
            query: None,
            tags: Vec::new(),
            hidden: TriState::False,
        }
    }
}
/// Conversation list filters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationFilters {
    /// Archive-state filter.
    pub archived: TriState,
    /// Hidden-state filter.
    pub hidden: TriState,
    /// All required tags.
    pub tags: Vec<String>,
}

impl Default for ConversationFilters {
    fn default() -> Self {
        Self {
            archived: TriState::Any,
            hidden: TriState::False,
            tags: Vec::new(),
        }
    }
}
/// Projected message list options.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MessageListOptions {
    /// Output order.
    pub order: MessageOrder,
    /// Exclusive projected-message cursor before filtering order.
    pub before: Option<MessageId>,
    /// Exclusive projected-message cursor after filtering order.
    pub after: Option<MessageId>,
    /// Optional positive result limit.
    pub limit: Option<usize>,
    /// Included return-message categories; empty means all.
    pub return_types: Vec<ReturnMessageType>,
}

/// Stable identity of one persisted source local message.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceMessageKey {
    /// Owning agent.
    pub agent_id: AgentId,
    /// Owning conversation.
    pub conversation_id: ConversationId,
    /// Source local message ID.
    pub source_id: MessageId,
    /// Zero-based source ordinal in the active projection.
    pub source_ordinal: usize,
}

/// One API-facing projection of a source local message.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProjectedMessage {
    /// Generated API projection ID; never persisted.
    pub id: MessageId,
    /// Canonical source identity.
    #[serde(skip)]
    pub source: SourceMessageKey,
    /// Stable projection ordinal within the source message.
    pub projection_ordinal: usize,
    /// Projected category.
    pub message_type: ReturnMessageType,
    /// Canonical projected timestamp in milliseconds.
    pub timestamp_ms: f64,
    /// Baseline-compatible projected fields.
    pub value: serde_json::Value,
}

/// Lookup result proving source identity.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageLookup {
    /// Canonical source identity.
    pub source: SourceMessageKey,
    /// Canonical persisted source local message.
    pub message: LocalMessage,
    /// Matching projection, when lookup used a projected ID.
    pub projection: Option<ProjectedMessage>,
}

/// Conversation and newest bounded projected suffix.
#[derive(Clone, Debug, PartialEq)]
pub struct ResumeTail {
    /// Scoped conversation snapshot.
    pub conversation: Conversation,
    /// Oldest-to-newest suffix.
    pub messages: Vec<ProjectedMessage>,
}

/// Transcript search input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptSearch {
    /// Required textual query.
    pub query: String,
    /// Optional exact agent scope.
    pub agent_id: Option<AgentId>,
    /// Optional exact conversation scope.
    pub conversation_id: Option<ConversationId>,
    /// Whether hidden conversations participate.
    pub include_hidden: bool,
    /// Positive result limit.
    pub limit: usize,
}

/// One persisted transcript search hit.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    /// Canonical source identity.
    pub source: SourceMessageKey,
    /// Matching projected message.
    pub message: ProjectedMessage,
    /// Deterministic lower-is-better match score.
    pub score: u64,
}

/// Runtime subscriber input snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeConnectionSnapshot {
    /// Domain projection.
    pub connection: RuntimeConnection,
    /// Whether its transport is currently live.
    pub live: bool,
}

/// Stable subscriber projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeSubscriber {
    /// Connection identifier.
    pub id: String,
    /// Stable connection ordinal.
    pub ordinal: u64,
}
