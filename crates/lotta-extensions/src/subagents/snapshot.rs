//! Bounded parent status snapshots and ordered child stream events.

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc;

/// Maximum serialized parent status snapshot.
pub const SUBAGENT_SNAPSHOT_BYTES_MAX: usize = 64 * 1_024;
/// Maximum serialized stream event.
pub const SUBAGENT_STREAM_EVENT_BYTES_MAX: usize = 64 * 1_024;
/// Maximum retained events in one snapshot.
pub const SUBAGENT_SNAPSHOT_EVENTS_MAX: usize = 128;
/// Maximum queued status operations per parent.
pub const SUBAGENT_STATUS_QUEUE_ITEMS_MAX: usize = 128;

/// Immutable task lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Accepted but waiting for the concurrent permit.
    Queued,
    /// Child is active.
    Running,
    /// Child completed successfully.
    Completed,
    /// Child failed.
    Failed,
    /// Child was cancelled and reaped.
    Cancelled,
}

impl TaskState {
    /// Returns whether process cleanup and terminal publication completed.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// One bounded ordered child stream delta.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEvent {
    /// Parent-scoped task ID.
    pub task_id: u64,
    /// Monotonic per-task sequence.
    pub sequence: u64,
    /// Bounded event bytes.
    pub bytes: Vec<u8>,
}

/// Canonical bounded parent status payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentSnapshot {
    /// Task identity.
    pub task_id: u64,
    /// Immutable owner attribution.
    pub owner_agent_id: String,
    /// Current task state.
    pub state: TaskState,
    /// Total accepted tasks for this parent.
    pub task_count: usize,
    /// Currently active children.
    pub active_task_count: usize,
    /// Recent ordered stream events.
    pub events: Vec<StreamEvent>,
}

/// Stable status serialization or delivery failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StatusError {
    /// Payload exceeds its canonical bound or allocation failed.
    #[error("subagent status bound exceeded")]
    Bound,
    /// Parent status receiver is unavailable or backpressured.
    #[error("subagent status unavailable")]
    Unavailable,
}

/// Parent runtime status capability.
pub trait StatusPort: Send + Sync {
    /// Port future.
    type Update<'a>: Future<Output = Result<(), StatusError>> + Send + 'a
    where
        Self: 'a;

    /// Applies one already-bounded authoritative snapshot payload.
    fn update_subagent_state(&self, payload: Vec<u8>) -> Self::Update<'_>;

    /// Applies one already-bounded ordered stream event payload.
    fn publish_subagent_event(&self, payload: Vec<u8>) -> Self::Update<'_> {
        self.update_subagent_state(payload)
    }
}

/// Serializes only after structural event bounds pass and before sending.
pub fn serialize_snapshot(snapshot: &SubagentSnapshot) -> Result<Vec<u8>, StatusError> {
    if snapshot.events.len() > SUBAGENT_SNAPSHOT_EVENTS_MAX {
        return Err(StatusError::Bound);
    }
    for event in &snapshot.events {
        validate_event(event)?;
    }
    serialize_bounded(snapshot, SUBAGENT_SNAPSHOT_BYTES_MAX)
}

/// Serializes one stream event before queue admission.
pub fn serialize_event(event: &StreamEvent) -> Result<Vec<u8>, StatusError> {
    validate_event(event)?;
    serialize_bounded(event, SUBAGENT_STREAM_EVENT_BYTES_MAX)
}

fn validate_event(event: &StreamEvent) -> Result<(), StatusError> {
    if event.task_id == 0
        || event.sequence == 0
        || event.bytes.is_empty()
        || event.bytes.len() > SUBAGENT_STREAM_EVENT_BYTES_MAX
    {
        Err(StatusError::Bound)
    } else {
        Ok(())
    }
}

fn serialize_bounded<T: Serialize>(value: &T, bytes_max: usize) -> Result<Vec<u8>, StatusError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(bytes_max)
        .map_err(|_| StatusError::Bound)?;
    let writer = BoundedWriter {
        output: &mut output,
        bytes_max,
    };
    serde_json::to_writer(writer, value).map_err(|_| StatusError::Bound)?;
    output.shrink_to_fit();
    Ok(output)
}

struct BoundedWriter<'a> {
    output: &'a mut Vec<u8>,
    bytes_max: usize,
}
impl std::io::Write for BoundedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .output
            .len()
            .checked_add(bytes.len())
            .filter(|next| *next <= self.bytes_max)
            .ok_or_else(|| std::io::Error::other("snapshot bound"))?;
        self.output
            .try_reserve(next - self.output.len())
            .map_err(|_| std::io::Error::other("snapshot allocation"))?;
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Bounded queue-backed status adapter useful at the runtime composition boundary.
pub struct ChannelStatusPort {
    sender: mpsc::Sender<Vec<u8>>,
}
impl ChannelStatusPort {
    /// Creates the adapter and its bounded receiver.
    #[must_use]
    pub fn channel() -> (Self, mpsc::Receiver<Vec<u8>>) {
        let (sender, receiver) = mpsc::channel(SUBAGENT_STATUS_QUEUE_ITEMS_MAX);
        (Self { sender }, receiver)
    }
}
impl StatusPort for ChannelStatusPort {
    type Update<'a> = Pin<Box<dyn Future<Output = Result<(), StatusError>> + Send + 'a>>;

    fn update_subagent_state(&self, payload: Vec<u8>) -> Self::Update<'_> {
        Box::pin(async move {
            let permit = self
                .sender
                .reserve()
                .await
                .map_err(|_| StatusError::Unavailable)?;
            permit.send(payload);
            Ok(())
        })
    }
}
