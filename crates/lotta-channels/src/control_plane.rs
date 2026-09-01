//! Strict newline-delimited JSON management plane.

use serde::{
    Deserialize, Serialize,
    de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeSet, VecDeque},
    io::ErrorKind,
    sync::Arc,
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
        /// Echoed request identifier.
        correlation_id: String,
        /// Bounded canonical state.
        channels: Vec<ChannelState>,
    },
    /// Publication acknowledgement.
    RuntimeToolsPublished {
        /// Echoed request identifier.
        correlation_id: String,
    },
    /// Release acknowledgement.
    RuntimeToolsReleased {
        /// Echoed request identifier.
        correlation_id: String,
    },
    /// Graceful child shutdown request.
    Shutdown {
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
        /// Bootstrap correlation.
        correlation_id: String,
        /// Exact owner identity.
        owner: String,
        /// Child process identifier.
        pid: u32,
    },
    /// Publish complete tools for one runtime under this child owner.
    PublishRuntimeTools {
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
        /// Unique request identifier.
        request_id: String,
        /// Exact child owner identity.
        owner: String,
        /// Exact runtime to release.
        runtime: RuntimeKey,
    },
    /// Execute `/channels` against the parent-owned canonical store view.
    Channels {
        /// Unique request identifier.
        request_id: String,
        /// Exact child owner identity.
        owner: String,
    },
    /// Graceful shutdown acknowledgement.
    ShutdownComplete {
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
    /// Frame owner did not match the supervised child.
    #[error("channel control owner mismatch")]
    Owner,
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

/// Stateful owner/correlation validator and command dispatcher.
pub struct ControlPlane {
    owner: String,
    generation: u64,
    replay_order: VecDeque<String>,
    replay: BTreeSet<String>,
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
        let (owner, request_id) = frame_identity(&frame);
        if owner != self.owner {
            return Err(ControlError::Owner);
        }
        validate_frame(&frame, channels)?;
        if let Some(id) = request_id {
            self.admit_request(id)?;
        }
        match frame {
            ChildFrame::PublishRuntimeTools {
                request_id,
                runtime,
                tools,
                ..
            } => {
                validate_runtime(&runtime)?;
                validate_tools(&tools)?;
                self.tools
                    .publish(
                        &self.owner,
                        self.generation,
                        canonical_runtime(runtime),
                        tools.into_iter().map(canonical_tool).collect(),
                    )
                    .map_err(|_| ControlError::Registry)?;
                Ok(Some(ParentFrame::RuntimeToolsPublished {
                    correlation_id: request_id,
                }))
            }
            ChildFrame::ReleaseRuntimeTools {
                request_id,
                runtime,
                ..
            } => {
                validate_runtime(&runtime)?;
                self.tools
                    .release(&self.owner, self.generation, &canonical_runtime(runtime))
                    .map_err(|_| ControlError::Registry)?;
                Ok(Some(ParentFrame::RuntimeToolsReleased {
                    correlation_id: request_id,
                }))
            }
            ChildFrame::Channels { request_id, .. } => {
                if channels.len() > CHANNEL_STATE_ROWS_MAX {
                    return Err(ControlError::Bound);
                }
                Ok(Some(ParentFrame::ChannelsResult {
                    correlation_id: request_id,
                    channels: channels.to_vec(),
                }))
            }
            ChildFrame::Ready { .. } | ChildFrame::ShutdownComplete { .. } => Ok(None),
        }
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

    fn admit_request(&mut self, request_id: &str) -> Result<(), ControlError> {
        validate_id(request_id)?;
        if self.replay.contains(request_id) {
            return Err(ControlError::Correlation);
        }
        if self.replay.len() >= CONTROL_REPLAY_IDS_MAX
            && let Some(oldest) = self.replay_order.pop_front()
        {
            self.replay.remove(&oldest);
        }
        self.replay.insert(request_id.to_owned());
        self.replay_order.push_back(request_id.to_owned());
        Ok(())
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
        .map_err(|_| ControlError::Malformed)?;
    deserializer.end().map_err(|_| ControlError::Malformed)?;
    Ok(value)
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

fn frame_identity(frame: &ChildFrame) -> (&str, Option<&str>) {
    match frame {
        ChildFrame::Ready {
            owner,
            correlation_id,
            ..
        }
        | ChildFrame::ShutdownComplete {
            owner,
            correlation_id,
        } => (owner, Some(correlation_id)),
        ChildFrame::PublishRuntimeTools {
            owner, request_id, ..
        }
        | ChildFrame::ReleaseRuntimeTools {
            owner, request_id, ..
        }
        | ChildFrame::Channels { owner, request_id } => (owner, Some(request_id)),
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
            request_id: "c1".into(),
            owner: "owner".into(),
        };
        let slash_result = plane.dispatch(slash, &channels);
        assert!(matches!(
            slash_result,
            Ok(Some(ParentFrame::ChannelsResult { channels: rows, .. })) if rows == channels
        ));
        let release = ChildFrame::ReleaseRuntimeTools {
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

    #[tokio::test]
    async fn bootstrap_ndjson_round_trip() {
        let bytes = concat!(
            "{\"kind\":\"bootstrap\",\"request_id\":\"x\",\"owner\":\"o\",",
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
            request_id: "same".into(),
            owner: "owner".into(),
        };
        assert!(plane.dispatch(frame.clone(), &[]).is_ok());
        assert!(matches!(
            plane.dispatch(frame, &[]),
            Err(ControlError::Correlation)
        ));
    }
}
