use super::{Deserialize, Serialize, Value};

/// Maximum bytes in one management line, excluding the newline.
pub const CONTROL_FRAME_BYTES_MAX: usize = 256 * 1024;
/// Maximum JSON nesting depth.
pub const CONTROL_JSON_DEPTH_MAX: usize = 16;
/// Maximum string bytes in any JSON member.
pub const CONTROL_STRING_BYTES_MAX: usize = 16 * 1024;
/// Maximum members in any JSON array.
pub const CONTROL_ARRAY_ITEMS_MAX: usize = 256;
/// Maximum entries in any JSON object.
pub const CONTROL_MAP_ENTRIES_MAX: usize = 128;
/// Maximum retained request identifiers used for replay rejection.
pub const CONTROL_REPLAY_IDS_MAX: usize = 1_024;
/// Maximum simultaneous request correlations.
pub const CONTROL_CORRELATIONS_MAX: usize = 128;
/// Maximum tools in one publication.
pub const RUNTIME_TOOLS_PER_PUBLICATION_MAX: usize = 64;
/// Maximum tools owned by one host across all runtimes.
pub const RUNTIME_TOOLS_PER_OWNER_MAX: usize = 256;
/// Maximum canonical channel rows returned by `/channels`.
pub const CHANNEL_STATE_ROWS_MAX: usize = 256;
/// Maximum stable identifier bytes.
pub const CONTROL_ID_BYTES_MAX: usize = 256;
/// Exact channel management protocol version repeated on every frame.
pub const CONTROL_PROTOCOL_VERSION: u16 = 1;
/// Maximum child-declared management timeout.
pub const CONTROL_TIMEOUT_MS_MAX: u64 = 10_000;

/// The sole capability declared by the Task 78 management plane.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagementCapability {
    /// Channel host lifecycle, state, and tool publication.
    ChannelManagement,
    /// Provider capability is representable but never admitted on this plane.
    Provider,
}

/// Metadata repeated on every management frame.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrameMetadata {
    /// Exact protocol version.
    pub version: u16,
    /// Exact supervised child generation.
    pub generation: u64,
    /// Declared management capability.
    pub capability: ManagementCapability,
    /// Child-declared processing timeout.
    pub timeout_ms: u64,
}

impl FrameMetadata {
    /// Creates exact bounded metadata for one generation.
    #[must_use]
    pub const fn new(generation: u64) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            generation,
            capability: ManagementCapability::ChannelManagement,
            timeout_ms: CONTROL_TIMEOUT_MS_MAX,
        }
    }
}

pub(super) const RESERVED_TOOL_NAMES: &[&str] =
    &["Bash", "Read", "Write", "Edit", "MessageChannel"];

/// One runtime identity addressed by channel tool publication.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RuntimeKey {
    /// Agent identifier.
    pub agent_id: String,
    /// Conversation identifier.
    pub conversation_id: String,
}

/// One bounded external tool definition.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeTool {
    /// Model-facing tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema parameters.
    pub parameters: Value,
}

/// Canonical bounded state returned to `/channels`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChannelState {
    /// Channel identifier.
    pub id: String,
    /// Whether persisted configuration enables the channel.
    pub enabled: bool,
    /// Number of canonical persisted account records.
    pub accounts: usize,
    /// Number of canonical persisted routes.
    pub routes: usize,
    /// Number of pending pairing records.
    pub pending_pairings: usize,
    /// Number of discovered/bound targets.
    pub targets: usize,
}

/// Parent-to-child bootstrap, command, and response frame.
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParentFrame {
    /// Secret bootstrap delivered on the inherited management pipe.
    Bootstrap {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Correlation identifier.
        request_id: String,
        /// Exact child owner identity.
        owner: String,
        /// Dedicated loopback WebSocket URL.
        websocket_url: String,
        /// Per-child bearer capability.
        token: String,
        /// Explicit canonical channels root.
        channels_root: String,
    },
    /// Canonical `/channels` response.
    ChannelsResult {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Exact child owner identity.
        owner: String,
        /// Echoed request identifier.
        correlation_id: String,
        /// Bounded canonical state.
        channels: Vec<ChannelState>,
    },
    /// Publication acknowledgement.
    RuntimeToolsPublished {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Exact child owner identity.
        owner: String,
        /// Echoed request identifier.
        correlation_id: String,
    },
    /// Release acknowledgement.
    RuntimeToolsReleased {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Exact child owner identity.
        owner: String,
        /// Echoed request identifier.
        correlation_id: String,
    },
    /// Graceful child shutdown request.
    Shutdown {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Exact child owner identity.
        owner: String,
        /// Correlation identifier.
        request_id: String,
    },
}

/// Child-to-parent management frame.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChildFrame {
    /// Startup handshake after the WebSocket has authenticated.
    Ready {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Bootstrap correlation.
        correlation_id: String,
        /// Exact owner identity.
        owner: String,
        /// Child process identifier.
        pid: u32,
        /// Actual write probe below the channels root succeeded.
        channels_probe_success: bool,
        /// Actual write probe at the parent/backend sibling scope was denied.
        parent_sibling_probe_denied: bool,
    },
    /// Publish complete tools for one runtime under this child owner.
    PublishRuntimeTools {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Unique request identifier.
        request_id: String,
        /// Exact child owner identity.
        owner: String,
        /// Runtime receiving the tools.
        runtime: RuntimeKey,
        /// Complete replacement set.
        tools: Vec<RuntimeTool>,
    },
    /// Release this child's tools for one exact runtime.
    ReleaseRuntimeTools {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Unique request identifier.
        request_id: String,
        /// Exact child owner identity.
        owner: String,
        /// Exact runtime to release.
        runtime: RuntimeKey,
    },
    /// Execute `/channels` against the parent-owned canonical store view.
    Channels {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Unique request identifier.
        request_id: String,
        /// Exact child owner identity.
        owner: String,
    },
    /// Graceful shutdown acknowledgement.
    ShutdownComplete {
        /// Repeated protocol metadata.
        #[serde(flatten)]
        metadata: FrameMetadata,
        /// Shutdown correlation.
        correlation_id: String,
        /// Exact child owner identity.
        owner: String,
    },
}

/// Stable management boundary failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ControlError {
    /// A frame exceeded a structural or byte bound.
    #[error("channel control frame exceeds bounds")]
    Bound,
    /// Input was not exactly one UTF-8 JSON object.
    #[error("channel control frame is malformed")]
    Malformed,
    /// Protocol version did not match exactly.
    #[error("channel control protocol version mismatch")]
    Version,
    /// Frame owner or generation did not match the supervised child.
    #[error("channel control owner mismatch")]
    Owner,
    /// Frame did not declare channel-management capability.
    #[error("channel control capability mismatch")]
    Capability,
    /// Frame timeout was zero or exceeded the host deadline.
    #[error("channel control timeout rejected")]
    Timeout,
    /// Request identifier was invalid, duplicated, or uncorrelated.
    #[error("channel control correlation rejected")]
    Correlation,
    /// Tool publication conflicts with registry policy.
    #[error("channel runtime tool publication rejected")]
    Registry,
    /// Management pipe failed.
    #[error("channel control pipe failed")]
    Io,
}

/// Direction and ownership of one live management correlation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlCorrelationDirection {
    /// Child request awaiting a parent response.
    ChildRequest,
    /// Parent request awaiting a child terminal response.
    ParentRequest,
}
