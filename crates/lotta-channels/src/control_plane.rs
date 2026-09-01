//! Strict newline-delimited JSON management plane.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::ErrorKind,
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
    /// Number of persisted accounts, when available.
    pub accounts: usize,
}

/// Parent-to-child bootstrap, command, and response frame.
#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug)]
struct OwnedTools {
    owner: String,
    tools: Vec<RuntimeTool>,
}

/// Parent-owned canonical runtime external-tool registry.
#[derive(Default)]
pub struct RuntimeToolRegistry {
    entries: BTreeMap<RuntimeKey, OwnedTools>,
}

impl RuntimeToolRegistry {
    /// Atomically replaces tools for one runtime after owner and collision checks.
    ///
    /// # Errors
    /// Returns an owner, identity, collision, reserved-name, or bound failure.
    pub fn publish(
        &mut self,
        owner: &str,
        runtime: RuntimeKey,
        tools: Vec<RuntimeTool>,
    ) -> Result<(), ControlError> {
        validate_id(owner)?;
        validate_runtime(&runtime)?;
        validate_tools(&tools)?;
        let retained = self
            .entries
            .iter()
            .filter(|(key, _)| **key != runtime)
            .map(|(_, value)| value.tools.len())
            .sum::<usize>();
        if retained.saturating_add(tools.len()) > RUNTIME_TOOLS_PER_OWNER_MAX {
            return Err(ControlError::Registry);
        }
        if self
            .entries
            .get(&runtime)
            .is_some_and(|entry| entry.owner != owner)
        {
            return Err(ControlError::Registry);
        }
        self.entries.insert(
            runtime,
            OwnedTools {
                owner: owner.to_owned(),
                tools,
            },
        );
        Ok(())
    }

    /// Registers the supervisor-blessed `MessageChannel` capability for one routed runtime.
    ///
    /// # Errors
    /// Returns an owner, identity, collision, or bound failure.
    pub fn register_message_channel(
        &mut self,
        owner: &str,
        runtime: RuntimeKey,
    ) -> Result<(), ControlError> {
        validate_id(owner)?;
        validate_runtime(&runtime)?;
        if self
            .entries
            .get(&runtime)
            .is_some_and(|entry| entry.owner != owner)
        {
            return Err(ControlError::Registry);
        }
        let tool = RuntimeTool {
            name: "MessageChannel".into(),
            description: "Send a visible reply through the originating channel".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"message": {"type": "string"}},
                "required": ["message"]
            }),
        };
        self.entries.insert(
            runtime,
            OwnedTools {
                owner: owner.to_owned(),
                tools: vec![tool],
            },
        );
        Ok(())
    }

    /// Releases one exact runtime only when the owner matches.
    ///
    /// # Errors
    /// Returns when a different child owns the runtime.
    pub fn release(&mut self, owner: &str, runtime: &RuntimeKey) -> Result<bool, ControlError> {
        match self.entries.get(runtime) {
            Some(entry) if entry.owner == owner => Ok(self.entries.remove(runtime).is_some()),
            Some(_) => Err(ControlError::Owner),
            None => Ok(false),
        }
    }

    /// Releases every runtime owned by one exact child and returns the count.
    pub fn release_owner(&mut self, owner: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, value| value.owner != owner);
        before - self.entries.len()
    }

    /// Returns an owned snapshot for tests and composition observers.
    #[must_use]
    pub fn snapshot(&self, runtime: &RuntimeKey) -> Option<Vec<RuntimeTool>> {
        self.entries.get(runtime).map(|entry| entry.tools.clone())
    }
}

/// Stateful owner/correlation validator and command dispatcher.
pub struct ControlPlane {
    owner: String,
    replay_order: VecDeque<String>,
    replay: BTreeSet<String>,
    registry: RuntimeToolRegistry,
}

impl ControlPlane {
    /// Creates a plane bound to one exact supervised child identity.
    ///
    /// # Errors
    /// Returns when the owner identity is invalid.
    pub fn new(owner: String) -> Result<Self, ControlError> {
        validate_id(&owner)?;
        Ok(Self {
            owner,
            replay_order: VecDeque::new(),
            replay: BTreeSet::new(),
            registry: RuntimeToolRegistry::default(),
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
                self.registry.publish(&self.owner, runtime, tools)?;
                Ok(Some(ParentFrame::RuntimeToolsPublished {
                    correlation_id: request_id,
                }))
            }
            ChildFrame::ReleaseRuntimeTools {
                request_id,
                runtime,
                ..
            } => {
                self.registry.release(&self.owner, &runtime)?;
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

    /// Registers the built-in `MessageChannel` capability after authenticated startup.
    ///
    /// # Errors
    /// Returns an identity, owner, collision, or bound failure.
    pub fn register_message_channel(&mut self, runtime: RuntimeKey) -> Result<(), ControlError> {
        self.registry.register_message_channel(&self.owner, runtime)
    }

    /// Returns the canonical registry owned by this plane.
    #[must_use]
    pub const fn registry(&self) -> &RuntimeToolRegistry {
        &self.registry
    }

    /// Releases all stale registrations exactly once for terminal cleanup.
    pub fn release_stale(&mut self) -> usize {
        self.registry.release_owner(&self.owner)
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
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = Value::deserialize(&mut deserializer).map_err(|_| ControlError::Malformed)?;
    deserializer.end().map_err(|_| ControlError::Malformed)?;
    validate_json(&value, 0)?;
    serde_json::from_value(value)
        .map_err(|_| ControlError::Malformed)
        .map(Some)
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

fn validate_json(value: &Value, depth: usize) -> Result<(), ControlError> {
    if depth > CONTROL_JSON_DEPTH_MAX {
        return Err(ControlError::Bound);
    }
    match value {
        Value::String(text) if text.len() > CONTROL_STRING_BYTES_MAX => Err(ControlError::Bound),
        Value::Array(values) if values.len() > CONTROL_ARRAY_ITEMS_MAX => Err(ControlError::Bound),
        Value::Object(values) if values.len() > CONTROL_MAP_ENTRIES_MAX => Err(ControlError::Bound),
        Value::Array(values) => values
            .iter()
            .try_for_each(|item| validate_json(item, depth + 1)),
        Value::Object(values) => values.iter().try_for_each(|(key, item)| {
            if key.len() > CONTROL_STRING_BYTES_MAX {
                return Err(ControlError::Bound);
            }
            validate_json(item, depth + 1)
        }),
        _ => Ok(()),
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
        validate_json(&tool.parameters, 0)?;
    }
    Ok(())
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
        ChildFrame::Ready { owner, .. } | ChildFrame::ShutdownComplete { owner, .. } => {
            (owner, None)
        }
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
        let mut plane = ControlPlane::new("owner".into()).unwrap();
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
        assert_eq!(plane.registry().snapshot(&runtime()).unwrap().len(), 1);
        let channels = vec![ChannelState {
            id: "telegram".into(),
            enabled: true,
            accounts: 1,
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
        assert!(plane.registry().snapshot(&runtime()).is_none());
    }

    #[tokio::test]
    async fn rejects_partial_multiple_oversize_malformed_unknown_and_replay() {
        for bytes in [
            b"{}".as_slice(),
            b"{}\n{}\n",
            b"not-json\n",
            b"{\"kind\":\"unknown\"}\n",
        ] {
            let mut reader = BufReader::new(bytes);
            assert!(read_line::<_, ChildFrame>(&mut reader).await.is_err());
        }
        let mut plane = ControlPlane::new("owner".into()).unwrap();
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
