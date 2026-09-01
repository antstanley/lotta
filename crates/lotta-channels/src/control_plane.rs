//! Strict newline-delimited JSON management plane.

use serde::{
    Deserialize, Serialize,
    de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::ErrorKind,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

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

const RESERVED_TOOL_NAMES: &[&str] = &["Bash", "Read", "Write", "Edit", "MessageChannel"];

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

#[derive(Clone, Debug)]
struct InFlightCorrelation {
    direction: ControlCorrelationDirection,
    owner: String,
    generation: u64,
    deadline_ms: u64,
}

/// Stateful owner/correlation validator and command dispatcher.
pub struct ControlPlane {
    owner: String,
    generation: u64,
    replay_order: VecDeque<String>,
    replay: BTreeSet<String>,
    in_flight: BTreeMap<String, InFlightCorrelation>,
    tools: Arc<lotta_tools::external::ChannelExternalToolManager>,
}

impl ControlPlane {
    /// Creates a plane bound to one exact supervised child identity.
    ///
    /// # Errors
    /// Returns when the owner identity is invalid.
    pub fn new(
        owner: String,
        generation: u64,
        tools: Arc<lotta_tools::external::ChannelExternalToolManager>,
    ) -> Result<Self, ControlError> {
        validate_id(&owner)?;
        if generation == 0 {
            return Err(ControlError::Owner);
        }
        Ok(Self {
            owner,
            generation,
            replay_order: VecDeque::new(),
            replay: BTreeSet::new(),
            in_flight: BTreeMap::new(),
            tools,
        })
    }

    /// Validates and applies one child management frame.
    ///
    /// # Errors
    /// Returns a structural, owner, correlation, registry, or bound failure.
    pub fn dispatch(
        &mut self,
        frame: ChildFrame,
        channels: &[ChannelState],
    ) -> Result<Option<ParentFrame>, ControlError> {
        self.dispatch_at(frame, channels, current_millis()?)
    }

    /// Validates and applies one frame at an explicit monotonic test deadline instant.
    ///
    /// Admission happens only after complete structural, owner, capability, and
    /// operation validation. Parent terminals must match a live parent-owned
    /// correlation; child requests remain live until their exact response is written.
    ///
    /// # Errors
    /// Returns a structural, ownership, deadline, correlation, registry, or bound failure.
    pub fn dispatch_at(
        &mut self,
        frame: ChildFrame,
        channels: &[ChannelState],
        now_ms: u64,
    ) -> Result<Option<ParentFrame>, ControlError> {
        let (metadata, owner, request_id) = frame_identity(&frame);
        if metadata.version != CONTROL_PROTOCOL_VERSION {
            return Err(ControlError::Version);
        }
        if owner != self.owner || metadata.generation != self.generation {
            return Err(ControlError::Owner);
        }
        if metadata.capability != ManagementCapability::ChannelManagement {
            return Err(ControlError::Capability);
        }
        if metadata.timeout_ms == 0 || metadata.timeout_ms > CONTROL_TIMEOUT_MS_MAX {
            return Err(ControlError::Timeout);
        }
        validate_frame(&frame, channels)?;
        let id = request_id.ok_or(ControlError::Correlation)?.to_owned();
        if matches!(
            frame,
            ChildFrame::Ready { .. } | ChildFrame::ShutdownComplete { .. }
        ) {
            self.complete_correlation(
                &id,
                ControlCorrelationDirection::ParentRequest,
                owner,
                metadata.generation,
                now_ms,
            )?;
            return self.apply(frame, channels);
        }
        self.admit_request(
            &id,
            ControlCorrelationDirection::ChildRequest,
            metadata.timeout_ms,
            now_ms,
        )?;
        let outcome = self.apply(frame, channels);
        if outcome.is_err() {
            self.in_flight.remove(&id);
        }
        outcome
    }

    fn apply(
        &self,
        frame: ChildFrame,
        channels: &[ChannelState],
    ) -> Result<Option<ParentFrame>, ControlError> {
        match frame {
            ChildFrame::PublishRuntimeTools {
                request_id,
                runtime,
                tools,
                ..
            } => self.publish(request_id, runtime, tools),
            ChildFrame::ReleaseRuntimeTools {
                request_id,
                runtime,
                ..
            } => self.release(request_id, runtime),
            ChildFrame::Channels { request_id, .. } => Ok(Some(ParentFrame::ChannelsResult {
                metadata: FrameMetadata::new(self.generation),
                owner: self.owner.clone(),
                correlation_id: request_id,
                channels: channels.to_vec(),
            })),
            ChildFrame::Ready { .. } | ChildFrame::ShutdownComplete { .. } => Ok(None),
        }
    }

    fn publish(
        &self,
        request_id: String,
        runtime: RuntimeKey,
        tools: Vec<RuntimeTool>,
    ) -> Result<Option<ParentFrame>, ControlError> {
        self.tools
            .publish(
                &self.owner,
                self.generation,
                canonical_runtime(runtime),
                tools.into_iter().map(canonical_tool).collect(),
            )
            .map_err(|_| ControlError::Registry)?;
        Ok(Some(ParentFrame::RuntimeToolsPublished {
            metadata: FrameMetadata::new(self.generation),
            owner: self.owner.clone(),
            correlation_id: request_id,
        }))
    }

    fn release(
        &self,
        request_id: String,
        runtime: RuntimeKey,
    ) -> Result<Option<ParentFrame>, ControlError> {
        self.tools
            .release(&self.owner, self.generation, &canonical_runtime(runtime))
            .map_err(|_| ControlError::Registry)?;
        Ok(Some(ParentFrame::RuntimeToolsReleased {
            metadata: FrameMetadata::new(self.generation),
            owner: self.owner.clone(),
            correlation_id: request_id,
        }))
    }

    /// Returns the exact owner bound to this management plane.
    #[must_use]
    pub(crate) fn owner(&self) -> String {
        self.owner.clone()
    }

    /// Returns the exact generation bound to this management plane.
    #[must_use]
    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns whether the canonical production manager currently owns this runtime.
    #[must_use]
    pub fn contains_runtime(&self, runtime: &RuntimeKey) -> bool {
        self.tools.contains(&canonical_runtime(runtime.clone()))
    }

    /// Releases all registrations for this exact terminated generation.
    #[must_use]
    pub fn release_stale(&self) -> usize {
        self.tools.release_generation(&self.owner, self.generation)
    }

    /// Registers one exact parent request before it is written to the child.
    ///
    /// # Errors
    /// Rejects invalid, replayed, over-capacity, or invalid-timeout requests.
    pub fn register_parent_request(
        &mut self,
        request_id: &str,
        timeout_ms: u64,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        if timeout_ms == 0 || timeout_ms > CONTROL_TIMEOUT_MS_MAX {
            return Err(ControlError::Timeout);
        }
        self.admit_request(
            request_id,
            ControlCorrelationDirection::ParentRequest,
            timeout_ms,
            now_ms,
        )
    }

    /// Completes the exact child request after its parent response is successfully written.
    ///
    /// # Errors
    /// Unknown, late, duplicated, wrong-owner, wrong-generation, and wrong-direction
    /// responses are terminal correlation failures.
    pub fn complete_parent_response_at(
        &mut self,
        frame: &ParentFrame,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        let (metadata, owner, correlation) = parent_response_identity(frame)?;
        if metadata.version != CONTROL_PROTOCOL_VERSION {
            return Err(ControlError::Version);
        }
        if metadata.capability != ManagementCapability::ChannelManagement {
            return Err(ControlError::Capability);
        }
        if metadata.timeout_ms == 0 || metadata.timeout_ms > CONTROL_TIMEOUT_MS_MAX {
            return Err(ControlError::Timeout);
        }
        self.complete_correlation(
            correlation,
            ControlCorrelationDirection::ChildRequest,
            owner,
            metadata.generation,
            now_ms,
        )
    }

    /// Returns the current bounded live-correlation count.
    #[must_use]
    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    fn admit_request(
        &mut self,
        request_id: &str,
        direction: ControlCorrelationDirection,
        timeout_ms: u64,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        validate_id(request_id)?;
        if self.replay.contains(request_id) || self.in_flight.len() >= CONTROL_CORRELATIONS_MAX {
            return Err(ControlError::Correlation);
        }
        self.retain_replay(request_id);
        self.in_flight.insert(
            request_id.to_owned(),
            InFlightCorrelation {
                direction,
                owner: self.owner.clone(),
                generation: self.generation,
                deadline_ms: now_ms.saturating_add(timeout_ms),
            },
        );
        Ok(())
    }

    fn complete_correlation(
        &mut self,
        request_id: &str,
        direction: ControlCorrelationDirection,
        owner: &str,
        generation: u64,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        validate_id(request_id)?;
        let pending = self
            .in_flight
            .remove(request_id)
            .ok_or(ControlError::Correlation)?;
        if pending.direction != direction
            || pending.owner != owner
            || pending.generation != generation
            || now_ms > pending.deadline_ms
        {
            return Err(if now_ms > pending.deadline_ms {
                ControlError::Timeout
            } else {
                ControlError::Correlation
            });
        }
        Ok(())
    }

    fn retain_replay(&mut self, request_id: &str) {
        if self.replay.len() >= CONTROL_REPLAY_IDS_MAX
            && let Some(oldest) = self.replay_order.pop_front()
        {
            self.replay.remove(&oldest);
        }
        self.replay.insert(request_id.to_owned());
        self.replay_order.push_back(request_id.to_owned());
    }
}

/// Reads exactly one bounded newline-delimited JSON object.
///
/// # Errors
/// Returns a byte, structure, UTF-8, JSON, discriminant, or pipe failure.
pub async fn read_line<R, T>(reader: &mut BufReader<R>) -> Result<Option<T>, ControlError>
where
    R: tokio::io::AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await.map_err(|_| ControlError::Io)?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err(ControlError::Bound);
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if bytes.len().saturating_add(consumed) > CONTROL_FRAME_BYTES_MAX + 1 {
            return Err(ControlError::Bound);
        }
        bytes.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if bytes.last() == Some(&b'\n') {
            break;
        }
    }
    bytes.pop();
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| ControlError::Malformed)?;
    let value = parse_bounded_json(text)?;
    if !value.is_object() {
        return Err(ControlError::Malformed);
    }
    serde_json::from_value(value)
        .map_err(|_| ControlError::Malformed)
        .map(Some)
}

fn parse_bounded_json(text: &str) -> Result<Value, ControlError> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = BoundedValueSeed { depth: 1 }
        .deserialize(&mut deserializer)
        .map_err(|error| map_json_decode(&error))?;
    deserializer.end().map_err(|_| ControlError::Malformed)?;
    Ok(value)
}

fn map_json_decode(error: &serde_json::Error) -> ControlError {
    let text = error.to_string();
    if text.contains("bound") {
        ControlError::Bound
    } else {
        ControlError::Malformed
    }
}

struct BoundedValueSeed {
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for BoundedValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if self.depth > CONTROL_JSON_DEPTH_MAX {
            return Err(D::Error::custom("json depth bound"));
        }
        deserializer.deserialize_any(BoundedValueVisitor { depth: self.depth })
    }
}

struct BoundedValueVisitor {
    depth: usize,
}

impl<'de> Visitor<'de> for BoundedValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded JSON value without duplicate keys")
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        self.visit_string(value.to_owned())
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        if value.len() > CONTROL_STRING_BYTES_MAX {
            return Err(E::custom("json string bound"));
        }
        Ok(Value::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(BoundedValueSeed {
            depth: self.depth + 1,
        })? {
            if values.len() >= CONTROL_ARRAY_ITEMS_MAX {
                return Err(A::Error::custom("json array bound"));
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        let mut keys = BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if key.len() > CONTROL_STRING_BYTES_MAX
                || values.len() >= CONTROL_MAP_ENTRIES_MAX
                || !keys.insert(key.clone())
            {
                return Err(A::Error::custom("json object bound or duplicate key"));
            }
            let value = object.next_value_seed(BoundedValueSeed {
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

/// Writes exactly one bounded JSON object followed by one newline.
///
/// # Errors
/// Returns an encoding, byte-bound, or pipe failure.
pub async fn write_line<W, T>(writer: &mut W, frame: &T) -> Result<(), ControlError>
where
    W: AsyncWrite + Unpin,
    T: Serialize + ?Sized,
{
    let bytes = serde_json::to_vec(frame).map_err(|_| ControlError::Malformed)?;
    if bytes.is_empty() || bytes.len() > CONTROL_FRAME_BYTES_MAX {
        return Err(ControlError::Bound);
    }
    writer.write_all(&bytes).await.map_err(map_io)?;
    writer.write_all(b"\n").await.map_err(map_io)?;
    writer.flush().await.map_err(map_io)
}

fn validate_structure(value: &Value) -> Result<(), ControlError> {
    let bounds = lotta_extensions::sidecar::framing::JsonStructureBounds {
        depth_max: CONTROL_JSON_DEPTH_MAX,
        string_bytes_max: CONTROL_STRING_BYTES_MAX,
        array_items_max: CONTROL_ARRAY_ITEMS_MAX,
        map_entries_max: CONTROL_MAP_ENTRIES_MAX,
    };
    lotta_extensions::sidecar::framing::validate_json_structure(value, bounds)
        .map_err(|_| ControlError::Bound)
}

fn validate_frame(frame: &ChildFrame, channels: &[ChannelState]) -> Result<(), ControlError> {
    match frame {
        ChildFrame::PublishRuntimeTools { runtime, tools, .. } => {
            validate_runtime(runtime)?;
            validate_tools(tools)
        }
        ChildFrame::ReleaseRuntimeTools { runtime, .. } => validate_runtime(runtime),
        ChildFrame::Channels { .. } => (channels.len() <= CHANNEL_STATE_ROWS_MAX)
            .then_some(())
            .ok_or(ControlError::Bound),
        ChildFrame::Ready { pid, .. } => (*pid != 0).then_some(()).ok_or(ControlError::Correlation),
        ChildFrame::ShutdownComplete { .. } => Ok(()),
    }
}

fn validate_tools(tools: &[RuntimeTool]) -> Result<(), ControlError> {
    if tools.is_empty() || tools.len() > RUNTIME_TOOLS_PER_PUBLICATION_MAX {
        return Err(ControlError::Registry);
    }
    let mut names = BTreeSet::new();
    for tool in tools {
        validate_id(&tool.name)?;
        if tool.description.is_empty()
            || tool.description.len() > CONTROL_STRING_BYTES_MAX
            || RESERVED_TOOL_NAMES.contains(&tool.name.as_str())
            || !names.insert(&tool.name)
        {
            return Err(ControlError::Registry);
        }
        validate_structure(&tool.parameters)?;
    }
    Ok(())
}

fn canonical_runtime(runtime: RuntimeKey) -> lotta_tools::external::ChannelRuntimeKey {
    lotta_tools::external::ChannelRuntimeKey {
        agent_id: runtime.agent_id,
        conversation_id: runtime.conversation_id,
    }
}

fn canonical_tool(tool: RuntimeTool) -> lotta_tools::external::ChannelToolDescriptor {
    lotta_tools::external::ChannelToolDescriptor {
        name: tool.name,
        description: tool.description,
        parameters: tool.parameters,
    }
}

fn validate_runtime(runtime: &RuntimeKey) -> Result<(), ControlError> {
    validate_id(&runtime.agent_id)?;
    validate_id(&runtime.conversation_id)
}

fn validate_id(value: &str) -> Result<(), ControlError> {
    if value.is_empty()
        || value.len() > CONTROL_ID_BYTES_MAX
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        Err(ControlError::Correlation)
    } else {
        Ok(())
    }
}

fn current_millis() -> Result<u64, ControlError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ControlError::Timeout)?
        .as_millis();
    u64::try_from(millis).map_err(|_| ControlError::Timeout)
}

fn parent_response_identity(
    frame: &ParentFrame,
) -> Result<(FrameMetadata, &str, &str), ControlError> {
    match frame {
        ParentFrame::ChannelsResult {
            metadata,
            owner,
            correlation_id,
            ..
        }
        | ParentFrame::RuntimeToolsPublished {
            metadata,
            owner,
            correlation_id,
        }
        | ParentFrame::RuntimeToolsReleased {
            metadata,
            owner,
            correlation_id,
        } => Ok((*metadata, owner, correlation_id)),
        ParentFrame::Bootstrap { .. } | ParentFrame::Shutdown { .. } => {
            Err(ControlError::Correlation)
        }
    }
}

fn frame_identity(frame: &ChildFrame) -> (FrameMetadata, &str, Option<&str>) {
    match frame {
        ChildFrame::Ready {
            metadata,
            owner,
            correlation_id,
            ..
        }
        | ChildFrame::ShutdownComplete {
            metadata,
            owner,
            correlation_id,
        } => (*metadata, owner, Some(correlation_id)),
        ChildFrame::PublishRuntimeTools {
            metadata,
            owner,
            request_id,
            ..
        }
        | ChildFrame::ReleaseRuntimeTools {
            metadata,
            owner,
            request_id,
            ..
        }
        | ChildFrame::Channels {
            metadata,
            owner,
            request_id,
        } => (*metadata, owner, Some(request_id)),
    }
}

fn map_io(error: std::io::Error) -> ControlError {
    let _closed = matches!(
        error.kind(),
        ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof
    );
    drop(error);
    ControlError::Io
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane() -> ControlPlane {
        let registry = Arc::new(lotta_tools::ToolRegistry::new([]).unwrap());
        let tools = Arc::new(lotta_tools::external::ChannelExternalToolManager::new(
            registry,
        ));
        ControlPlane::new("owner".into(), 1, tools).unwrap()
    }

    fn runtime() -> RuntimeKey {
        RuntimeKey {
            agent_id: "agent".into(),
            conversation_id: "conversation".into(),
        }
    }
    fn tool() -> RuntimeTool {
        RuntimeTool {
            name: "channel_send".into(),
            description: "Send a channel reply".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }

    #[test]
    fn publish_release_channels_registry_effects() {
        let mut plane = plane();
        let publish = ChildFrame::PublishRuntimeTools {
            metadata: FrameMetadata::new(1),
            request_id: "p1".into(),
            owner: "owner".into(),
            runtime: runtime(),
            tools: vec![tool()],
        };
        assert!(matches!(
            plane.dispatch(publish, &[]),
            Ok(Some(ParentFrame::RuntimeToolsPublished { .. }))
        ));
        assert!(plane.contains_runtime(&runtime()));
        let channels = vec![ChannelState {
            id: "telegram".into(),
            enabled: true,
            accounts: 1,
            routes: 1,
            pending_pairings: 0,
            targets: 1,
        }];
        let slash = ChildFrame::Channels {
            metadata: FrameMetadata::new(1),
            request_id: "c1".into(),
            owner: "owner".into(),
        };
        let slash_result = plane.dispatch(slash, &channels);
        assert!(matches!(
            slash_result,
            Ok(Some(ParentFrame::ChannelsResult { channels: rows, .. })) if rows == channels
        ));
        let release = ChildFrame::ReleaseRuntimeTools {
            metadata: FrameMetadata::new(1),
            request_id: "r1".into(),
            owner: "owner".into(),
            runtime: runtime(),
        };
        assert!(matches!(
            plane.dispatch(release, &[]),
            Ok(Some(ParentFrame::RuntimeToolsReleased { .. }))
        ));
        assert!(!plane.contains_runtime(&runtime()));
    }

    #[test]
    fn validates_version_owner_capability_timeout_before_dispatch() {
        let make = |metadata, owner: &str| ChildFrame::Channels {
            metadata,
            request_id: "ordered".into(),
            owner: owner.into(),
        };
        let mut metadata = FrameMetadata::new(1);
        metadata.version += 1;
        assert!(matches!(
            plane().dispatch(make(metadata, "other"), &[]),
            Err(ControlError::Version)
        ));
        let mut metadata = FrameMetadata::new(2);
        metadata.capability = ManagementCapability::Provider;
        metadata.timeout_ms = 0;
        assert!(matches!(
            plane().dispatch(make(metadata, "other"), &[]),
            Err(ControlError::Owner)
        ));
        let mut metadata = FrameMetadata::new(1);
        metadata.capability = ManagementCapability::Provider;
        metadata.timeout_ms = 0;
        assert!(matches!(
            plane().dispatch(make(metadata, "owner"), &[]),
            Err(ControlError::Capability)
        ));
        let mut metadata = FrameMetadata::new(1);
        metadata.timeout_ms = 0;
        assert!(matches!(
            plane().dispatch(make(metadata, "owner"), &[]),
            Err(ControlError::Timeout)
        ));
        let mut metadata = FrameMetadata::new(1);
        metadata.timeout_ms = CONTROL_TIMEOUT_MS_MAX + 1;
        assert!(matches!(
            plane().dispatch(make(metadata, "owner"), &[]),
            Err(ControlError::Timeout)
        ));
    }

    #[tokio::test]
    async fn bootstrap_ndjson_round_trip() {
        let bytes = concat!(
            "{\"kind\":\"bootstrap\",\"version\":1,\"generation\":1,",
            "\"capability\":\"channel_management\",\"timeout_ms\":10000,",
            "\"request_id\":\"x\",\"owner\":\"o\",",
            "\"websocket_url\":\"ws://127.0.0.1:9/ws\",\"token\":\"t\",",
            "\"channels_root\":\"/tmp\"}\n"
        )
        .as_bytes();
        let mut reader = BufReader::new(bytes);
        assert!(matches!(
            read_line::<_, ParentFrame>(&mut reader).await,
            Ok(Some(ParentFrame::Bootstrap { .. }))
        ));
    }

    #[tokio::test]
    async fn rejects_partial_multiple_oversize_malformed_unknown_and_replay() {
        let duplicate_owner = concat!(
            "{\"kind\":\"channels\",\"request_id\":\"a\",\"owner\":\"owner\",",
            "\"owner\":\"owner\"}\n"
        );
        let nested_duplicate = concat!(
            "{\"kind\":\"channels\",\"request_id\":\"a\",\"owner\":\"owner\",",
            "\"nested\":{\"x\":1,\"x\":2}}\n"
        );
        for bytes in [
            b"{}".as_slice(),
            b"{} {}\n",
            b"{}\n{}\n",
            b"not-json\n",
            b"{\"kind\":\"unknown\"}\n",
            duplicate_owner.as_bytes(),
            nested_duplicate.as_bytes(),
            b"{\"kind\":\"channels\",\"request_id\":\"a\",\"owner\":\"\xff\"}\n",
        ] {
            let mut reader = BufReader::new(bytes);
            assert!(read_line::<_, ChildFrame>(&mut reader).await.is_err());
        }
        let mut plane = plane();
        let frame = ChildFrame::Channels {
            metadata: FrameMetadata::new(1),
            request_id: "same".into(),
            owner: "owner".into(),
        };
        assert!(plane.dispatch(frame.clone(), &[]).is_ok());
        assert!(matches!(
            plane.dispatch(frame, &[]),
            Err(ControlError::Correlation)
        ));
    }

    #[test]
    fn live_correlations_enforce_capacity_direction_deadline_and_replay_independently() {
        let mut plane = plane();
        for index in 0..CONTROL_CORRELATIONS_MAX {
            plane
                .register_parent_request(&format!("parent-{index}"), 10, 100)
                .unwrap();
        }
        assert_eq!(plane.in_flight_len(), CONTROL_CORRELATIONS_MAX);
        assert_eq!(
            plane.register_parent_request("above", 10, 100),
            Err(ControlError::Correlation)
        );

        let terminal = |id: &str, owner: &str| ChildFrame::Ready {
            metadata: FrameMetadata::new(1),
            correlation_id: id.into(),
            owner: owner.into(),
            pid: 7,
            channels_probe_success: true,
            parent_sibling_probe_denied: true,
        };
        assert!(
            plane
                .dispatch_at(terminal("parent-0", "owner"), &[], 110)
                .is_ok()
        );
        assert_eq!(plane.in_flight_len(), CONTROL_CORRELATIONS_MAX - 1);
        assert!(matches!(
            plane.dispatch_at(terminal("parent-1", "owner"), &[], 111),
            Err(ControlError::Timeout)
        ));
        assert!(matches!(
            plane.dispatch_at(terminal("parent-0", "owner"), &[], 109),
            Err(ControlError::Correlation)
        ));
        assert_eq!(
            plane.register_parent_request("parent-0", 10, 100),
            Err(ControlError::Correlation),
            "replay retention is independent from live completion"
        );
    }

    #[test]
    fn child_request_completes_only_with_exact_parent_response() {
        let mut plane = plane();
        let request = ChildFrame::Channels {
            metadata: FrameMetadata {
                timeout_ms: 5,
                ..FrameMetadata::new(1)
            },
            request_id: "child-live".into(),
            owner: "owner".into(),
        };
        let response = plane.dispatch_at(request, &[], 50).unwrap().unwrap();
        assert_eq!(plane.in_flight_len(), 1);
        assert_eq!(plane.complete_parent_response_at(&response, 55), Ok(()));
        assert_eq!(plane.in_flight_len(), 0);
        assert_eq!(
            plane.complete_parent_response_at(&response, 55),
            Err(ControlError::Correlation)
        );

        let late = ChildFrame::Channels {
            metadata: FrameMetadata {
                timeout_ms: 5,
                ..FrameMetadata::new(1)
            },
            request_id: "child-late".into(),
            owner: "owner".into(),
        };
        let response = plane.dispatch_at(late, &[], 50).unwrap().unwrap();
        assert_eq!(
            plane.complete_parent_response_at(&response, 56),
            Err(ControlError::Timeout)
        );
    }

    #[tokio::test]
    async fn exact_ndjson_structure_frame_and_order_boundaries() {
        for length in [CONTROL_STRING_BYTES_MAX - 1, CONTROL_STRING_BYTES_MAX] {
            let text = serde_json::to_string(&"x".repeat(length)).unwrap();
            assert!(parse_bounded_json(&text).is_ok());
        }
        let text = serde_json::to_string(&"x".repeat(CONTROL_STRING_BYTES_MAX + 1)).unwrap();
        assert_eq!(parse_bounded_json(&text), Err(ControlError::Bound));

        for length in [CONTROL_ARRAY_ITEMS_MAX - 1, CONTROL_ARRAY_ITEMS_MAX] {
            let text = serde_json::to_string(&vec![0; length]).unwrap();
            assert!(parse_bounded_json(&text).is_ok());
        }
        let text = serde_json::to_string(&vec![0; CONTROL_ARRAY_ITEMS_MAX + 1]).unwrap();
        assert_eq!(parse_bounded_json(&text), Err(ControlError::Bound));

        for length in [CONTROL_MAP_ENTRIES_MAX - 1, CONTROL_MAP_ENTRIES_MAX] {
            let map = (0..length)
                .map(|index| (format!("k{index}"), 0))
                .collect::<std::collections::BTreeMap<_, _>>();
            assert!(parse_bounded_json(&serde_json::to_string(&map).unwrap()).is_ok());
        }
        let map = (0..=CONTROL_MAP_ENTRIES_MAX)
            .map(|index| (format!("k{index}"), 0))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            parse_bounded_json(&serde_json::to_string(&map).unwrap()),
            Err(ControlError::Bound)
        );

        for length in [CONTROL_FRAME_BYTES_MAX - 1, CONTROL_FRAME_BYTES_MAX] {
            let mut line = b"{}".to_vec();
            line.resize(length, b' ');
            line.push(b'\n');
            let mut reader = BufReader::new(line.as_slice());
            assert!(read_line::<_, Value>(&mut reader).await.is_ok());
        }
        let mut above = b"{}".to_vec();
        above.resize(CONTROL_FRAME_BYTES_MAX + 1, b' ');
        above.push(b'\n');
        let mut reader = BufReader::new(above.as_slice());
        assert_eq!(
            read_line::<_, Value>(&mut reader).await,
            Err(ControlError::Bound)
        );

        let first = serde_json::to_string(&ChildFrame::Channels {
            metadata: FrameMetadata::new(1),
            request_id: "first".into(),
            owner: "owner".into(),
        })
        .unwrap();
        let ordered = format!("{first}\n{}\n", first.replace("first", "second"));
        let mut reader = BufReader::new(ordered.as_bytes());
        for expected in ["first", "second"] {
            let Some(ChildFrame::Channels { request_id, .. }) =
                read_line::<_, ChildFrame>(&mut reader).await.unwrap()
            else {
                panic!("ordered channel frame");
            };
            assert_eq!(request_id, expected);
        }
        assert!(
            read_line::<_, ChildFrame>(&mut reader)
                .await
                .unwrap()
                .is_none()
        );
    }
}
