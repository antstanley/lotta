//! Sanitized provider-stream fixture corpus and replay support.

use crate::TESTKIT_ITEMS_MAX;
use lotta_domain::{BoundedJsonValue, BoundedVec, ModelDescriptor};
use lotta_runtime::ports::{ProviderEvent, ProviderRequest};
use serde::{Deserialize, Deserializer};

pub(super) const DIALECT_COUNT: usize = 5;
pub(super) const DIMENSION_COUNT: usize = 10;
pub(super) const CASE_COUNT: usize = 16;
pub(super) const INVENTORY_COUNT: usize = 81;
pub(super) const ERROR_KIND_COUNT: usize = 12;
pub(super) const TRACE_EVENTS_MAX: usize = 64;
pub(super) const REPLAY_CHANNEL_EVENTS_MAX: usize = 8;
/// Maximum captured response headers replayed for one case.
pub const RESPONSE_HEADERS_MAX: usize = 8;
/// Lowest captured response status the corpus accepts.
pub(super) const RESPONSE_STATUS_MIN: u16 = 200;
/// Highest captured response status the corpus accepts.
pub(super) const RESPONSE_STATUS_MAX: u16 = 599;
pub(super) const SOURCE_COMMIT: &str = "300f923f16cc8eee50656d7da732902c1dea2b65";
pub(super) const PI_AI_VERSION: &str = "0.82.1";
pub(super) const FILES: [&str; 5] = [
    "request.json",
    "expected-request.json",
    "raw-stream.txt",
    "baseline-events.jsonl",
    "expected-trace.json",
];

/// Provider wire dialect represented by the corpus.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub enum Dialect {
    /// `OpenAI` chat-completions compatible SSE.
    OpenaiCompatible,
    /// Anthropic Messages named-event SSE.
    Anthropic,
    /// Ollama chat NDJSON.
    Ollama,
    /// LM Studio OpenAI-compatible SSE.
    LmStudio,
    /// llama.cpp OpenAI-compatible SSE.
    LlamaCpp,
}
impl Dialect {
    /// Exact dialect values in corpus order.
    pub const ALL: [Self; DIALECT_COUNT] = [
        Self::OpenaiCompatible,
        Self::Anthropic,
        Self::Ollama,
        Self::LmStudio,
        Self::LlamaCpp,
    ];
    pub(super) fn path(self) -> &'static str {
        match self {
            Self::OpenaiCompatible => "openai-compatible",
            Self::Anthropic => "anthropic",
            Self::Ollama => "ollama",
            Self::LmStudio => "lm-studio",
            Self::LlamaCpp => "llama-cpp",
        }
    }
}

/// One exact provider conformance dimension.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    /// Request mapping.
    RequestMapping,
    /// Event order.
    EventOrder,
    /// Incremental tool-call assembly.
    ToolCallAssembly,
    /// Monotonic usage.
    Usage,
    /// Cancellation and late-event suppression.
    Cancellation,
    /// Stable errors.
    Errors,
    /// Deadline timeout.
    Timeout,
    /// Context overflow.
    ContextOverflow,
    /// Safe retry-after metadata.
    RetryAfter,
    /// Strict/drop image behavior.
    ImagePolicy,
}
impl Dimension {
    /// Exact ten dimensions, independently defined from fixture metadata.
    pub const ALL: [Self; DIMENSION_COUNT] = [
        Self::RequestMapping,
        Self::EventOrder,
        Self::ToolCallAssembly,
        Self::Usage,
        Self::Cancellation,
        Self::Errors,
        Self::Timeout,
        Self::ContextOverflow,
        Self::RetryAfter,
        Self::ImagePolicy,
    ];
}

/// Stable provider error kind in fixture JSON.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    /// Authentication.
    Authentication,
    /// Authorization.
    Authorization,
    /// Invalid request.
    InvalidRequest,
    /// Rate limit.
    RateLimit,
    /// Quota.
    Quota,
    /// Timeout.
    Timeout,
    /// Context overflow.
    ContextOverflow,
    /// Overloaded.
    Overloaded,
    /// Unavailable.
    Unavailable,
    /// Protocol.
    Protocol,
    /// Cancelled.
    Cancelled,
    /// Unknown.
    Unknown,
}
impl ProviderErrorKind {
    /// Exact twelve stable kinds.
    pub const ALL: [Self; ERROR_KIND_COUNT] = [
        Self::Authentication,
        Self::Authorization,
        Self::InvalidRequest,
        Self::RateLimit,
        Self::Quota,
        Self::Timeout,
        Self::ContextOverflow,
        Self::Overloaded,
        Self::Unavailable,
        Self::Protocol,
        Self::Cancelled,
        Self::Unknown,
    ];
}

/// Raw stream framing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RawFormat {
    /// OpenAI-style SSE.
    Sse,
    /// Anthropic named-event SSE.
    AnthropicSse,
    /// Ollama NDJSON.
    Ndjson,
    /// Plain vendor JSON body delivered with an explicit non-2xx [`ResponseFixture`].
    Json,
}
/// One exact response header replayed verbatim by a loopback transport.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct HeaderFixture {
    /// Lowercase header name.
    pub name: String,
    /// Exact header value.
    pub value: String,
}
/// Explicit HTTP response metadata a loopback transport replays with `raw-stream.txt`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ResponseFixture {
    /// Exact response status code.
    pub status: u16,
    /// Exact response headers in captured order.
    pub headers: BoundedVec<HeaderFixture, RESPONSE_HEADERS_MAX>,
}
impl ResponseFixture {
    /// Reports whether the captured status is a success status.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}
/// Successful or failed terminal kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    /// Successful stop.
    Stop,
    /// Stable provider error.
    Error,
}
/// Inventory semantic class.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InventoryKind {
    /// JSON.
    Json,
    /// JSON Lines.
    Jsonl,
    /// Raw wire text.
    Raw,
}
/// Visible and redacted reasoning coverage flags.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub struct ReasoningFlags {
    /// Visible reasoning is expected.
    pub visible: bool,
    /// Opaque/redacted reasoning is expected.
    pub redacted: bool,
}
/// One indexed case.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ProviderCaseRecord {
    /// Stable dialect/name identifier.
    pub id: String,
    /// Wire dialect.
    pub dialect: Dialect,
    /// Case name.
    pub name: String,
    /// Exact five case paths.
    pub paths: [String; 5],
    /// Raw framing.
    pub raw_format: RawFormat,
    /// Genuine conformance tags.
    pub dimensions: BoundedVec<Dimension, DIMENSION_COUNT>,
    /// Reasoning coverage.
    pub reasoning: ReasoningFlags,
    /// Expected terminal kind.
    pub terminal_kind: TerminalKind,
    /// Explicit response status and headers for cases replayed over a real transport.
    #[serde(default)]
    pub response: Option<ResponseFixture>,
    /// Optional independently specified pinned compatibility-host trace.
    #[serde(default)]
    pub host_expected_trace: Option<String>,
    /// Why the host trace differs from the canonical native trace.
    #[serde(default)]
    pub host_trace_provenance: Option<String>,
}
/// One indexed corpus file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct InventoryEntry {
    /// Corpus-relative path.
    pub path: String,
    /// Semantic parser class.
    pub kind: InventoryKind,
    /// Exact bytes.
    pub bytes: usize,
    /// Exact SHA-256.
    pub sha256: String,
}
/// One pinned lexical operative-region proof.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct SourceRegion {
    /// Pinned source path.
    pub path: String,
    /// Unique source symbol.
    pub symbol: String,
    /// Lexical balanced region hash.
    pub sha256: String,
}
/// One exact error-kind to fixture-case mapping.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ErrorKindCase(pub ProviderErrorKind, pub String);
/// Complete fixed-shape provider corpus index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ProviderIndex {
    /// Index schema version.
    pub schema_version: u8,
    /// Exact source revision.
    pub source_commit: String,
    /// Exact resolved pi-ai version.
    pub pi_ai_version: String,
    /// Deterministic generator.
    pub generator: String,
    /// Provenance statement.
    pub provenance: String,
    /// Capture boundary statement.
    pub capture_boundary: String,
    /// Pinned operative regions.
    pub source_regions: [SourceRegion; 24],
    /// Exact dialects.
    pub dialects: [Dialect; DIALECT_COUNT],
    /// Exact dimensions.
    pub dimensions: [Dimension; DIMENSION_COUNT],
    /// Exact cases.
    pub cases: [ProviderCaseRecord; CASE_COUNT],
    /// Exact error-kind mapping.
    pub error_kind_to_case: [ErrorKindCase; ERROR_KIND_COUNT],
    /// Exact complete inventory.
    #[serde(deserialize_with = "deserialize_inventory")]
    pub inventory: [InventoryEntry; INVENTORY_COUNT],
}

/// Loaded and validated fixture case.
pub struct ProviderCase {
    /// Indexed metadata.
    pub record: ProviderCaseRecord,
    /// Real bounded provider request.
    pub request: ProviderRequest,
    /// Exact expected vendor/preflight request mapping.
    pub expected_request: BoundedJsonValue,
    /// Raw sanitized wire text.
    pub raw_stream: String,
    /// Captured boundary events including provenance.
    /// Number of captured boundary events before terminal suppression.
    pub baseline_event_count: usize,
    /// Real Task07 expected trace.
    pub expected_trace: BoundedVec<ProviderEvent, TRACE_EVENTS_MAX>,
    /// Optional pinned compatibility-host trace for a documented pi-ai information loss.
    pub host_expected_trace: Option<BoundedVec<ProviderEvent, TRACE_EVENTS_MAX>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct RequestFixture {
    pub(super) model: ModelDescriptor,
    pub(super) system: Option<String>,
    pub(super) messages: BoundedVec<MessageFixture, TESTKIT_ITEMS_MAX>,
    pub(super) tools: BoundedVec<ToolFixture, TESTKIT_ITEMS_MAX>,
    pub(super) tool_choice: ToolChoiceFixture,
    pub(super) image_policy: ImagePolicyFixture,
    pub(super) context_tokens_max: u64,
    pub(super) output_tokens_max: u64,
    pub(super) reasoning: ReasoningFixture,
    pub(super) deadline_ms: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(super) struct MessageFixture {
    pub(super) role: RoleFixture,
    pub(super) content: BoundedVec<ContentFixture, TESTKIT_ITEMS_MAX>,
    pub(super) tool_call_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ContentFixture {
    Text { text: String },
    Image { media_type: String, base64: String },
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct ToolFixture {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) input_schema: BoundedJsonValue,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum RoleFixture {
    User,
    Assistant,
    Tool,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ToolChoiceFixture {
    Auto,
    None,
    Required,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ImagePolicyFixture {
    Strict,
    Drop,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(super) struct ReasoningFixture {
    pub(super) enabled: bool,
    pub(super) effort: Option<String>,
    pub(super) tier: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(super) struct Provenance {
    pub(super) source_test: String,
    pub(super) source_symbol: String,
    pub(super) capture_boundary: String,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct MetadataEntryFixture {
    pub(super) key: String,
    pub(super) value: BoundedJsonValue,
}
/// Distinct pinned baseline/raw-boundary vocabulary.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum BaselineEventRecord {
    TextDelta {
        text: String,
        provenance: Provenance,
    },
    ThinkingDelta {
        text: String,
        provenance: Provenance,
    },
    RedactedReasoning {
        marker: String,
        provenance: Provenance,
    },
    ToolcallStart {
        call_id: String,
        name: String,
        provenance: Provenance,
    },
    ToolcallArgumentsDelta {
        call_id: String,
        arguments: String,
        provenance: Provenance,
    },
    ToolcallEnd {
        call_id: String,
        provenance: Provenance,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        reasoning_tokens: u64,
        provenance: Provenance,
    },
    Metadata {
        entries: BoundedVec<MetadataEntryFixture, 8>,
        provenance: Provenance,
    },
    Done {
        reason: StopReasonFixture,
        provenance: Provenance,
    },
    Error {
        kind: ProviderErrorKind,
        code: String,
        context: String,
        #[serde(default)]
        retry_after: Option<RetryAfterFixture>,
        provenance: Provenance,
    },
    Cancelled {
        code: String,
        context: String,
        provenance: Provenance,
    },
    LateTextDelta {
        text: String,
        provenance: Provenance,
    },
}
/// Exact Task07 expected trace vocabulary, parsed independently from baseline records.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub(super) enum FixtureEventRecord {
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    RedactedReasoning {
        marker: String,
    },
    ToolCallStart {
        call_id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        call_id: String,
        bytes_base64: String,
    },
    ToolCallEnd {
        call_id: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        reasoning_tokens: u64,
    },
    ProviderMetadata {
        entries: BoundedVec<MetadataEntryFixture, 8>,
    },
    Stop {
        reason: StopReasonFixture,
    },
    Error {
        kind: ProviderErrorKind,
        code: String,
        context: String,
        /// Absent unless the capture preserved a provider-directed retry delay.
        #[serde(default)]
        retry_after: Option<RetryAfterFixture>,
    },
}
/// Provider-directed retry timing preserved by error normalization.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum RetryAfterFixture {
    Milliseconds { value: u64 },
    DateMilliseconds { value: u64 },
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum StopReasonFixture {
    EndTurn,
    OutputLimit,
    ToolUse,
    ContentFilter,
    Other,
}

pub(super) fn deserialize_inventory<'de, D>(
    d: D,
) -> Result<[InventoryEntry; INVENTORY_COUNT], D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<InventoryEntry>::deserialize(d)?;
    values
        .try_into()
        .map_err(|v: Vec<_>| serde::de::Error::invalid_length(v.len(), &"exact provider inventory"))
}
