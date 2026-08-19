//! Safe structured runtime events and bounded delivery.

use crate::ports::{StopReason, ToolCallId};
use crate::registry::RuntimeKey;
use lotta_domain::{NonEmptyString, RunId};
use serde::Serialize;
use std::fmt;
use std::sync::{Arc, Mutex};

/// Maximum events retained by the in-memory bounded sink.
pub const OBSERVE_EVENTS_MAX: usize = 1_024;

/// Stable runtime event category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKind {
    /// Input reached admission.
    Admission,
    /// Input was queued or queue depth changed.
    Queue,
    /// A provider attempt started or completed.
    ProviderAttempt,
    /// A tool call started or completed.
    Tool,
    /// Context was compacted.
    Compaction,
    /// Cancellation progressed or completed.
    Cancellation,
    /// A turn reached its terminal outcome.
    Terminal,
    /// A stale lease effect was suppressed.
    StaleSuppression,
}

/// Stable non-secret terminal reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedStopReason {
    /// Normal end turn.
    EndTurn,
    /// Output limit.
    OutputLimit,
    /// Tool use continuation.
    ToolUse,
    /// Content filter.
    ContentFilter,
    /// Provider-specific safe fallback.
    Other,
    /// User cancellation.
    Cancelled,
    /// Runtime step limit.
    StepLimit,
    /// Runtime tool-call limit.
    ToolCallLimit,
    /// Context overflow.
    ContextOverflow,
    /// Typed runtime failure.
    Failed,
    /// Stale effect suppression.
    Suppressed,
}

impl From<StopReason> for ObservedStopReason {
    fn from(value: StopReason) -> Self {
        match value {
            StopReason::EndTurn => Self::EndTurn,
            StopReason::OutputLimit => Self::OutputLimit,
            StopReason::ToolUse => Self::ToolUse,
            StopReason::ContentFilter => Self::ContentFilter,
            StopReason::Other => Self::Other,
        }
    }
}

/// Runtime identity serialized without acting-user attribution.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct ObservedRuntimeKey {
    agent_id: String,
    conversation_id: String,
}

impl From<&RuntimeKey> for ObservedRuntimeKey {
    fn from(value: &RuntimeKey) -> Self {
        Self {
            agent_id: value.agent_id().as_str().to_owned(),
            conversation_id: value.conversation_id().as_str().to_owned(),
        }
    }
}

impl fmt::Debug for ObservedRuntimeKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedRuntimeKey")
            .field("agent_id", &self.agent_id)
            .field("conversation_id", &self.conversation_id)
            .finish()
    }
}

/// Safe event fields shared by all runtime observation points.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct RuntimeEvent {
    /// Stable event category.
    pub kind: RuntimeEventKind,
    /// Runtime key excluding acting-user attribution.
    pub runtime_key: ObservedRuntimeKey,
    /// Stable connection identifier.
    pub connection_id: String,
    /// Exact turn/lease generation.
    pub turn_lease_generation: u64,
    /// Stable run identifier, when allocated.
    pub run_id: Option<String>,
    /// Bounded provider identifier, when known.
    pub provider: Option<String>,
    /// Stable tool-call identifier, when known.
    pub tool_call_id: Option<String>,
    /// One-based attempt number, when applicable.
    pub attempt: Option<u32>,
    /// Authoritative queue length, when applicable.
    pub queue_length: Option<u32>,
    /// Stable stop reason, when applicable.
    pub stop_reason: Option<ObservedStopReason>,
    /// Nonnegative operation duration in milliseconds.
    pub duration_ms: Option<u64>,
}

impl fmt::Debug for RuntimeEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeEvent")
            .field("kind", &self.kind)
            .field("runtime_key", &self.runtime_key)
            .field("connection_id", &self.connection_id)
            .field("turn_lease_generation", &self.turn_lease_generation)
            .field("run_id", &self.run_id)
            .field("provider", &self.provider)
            .field("tool_call_id", &self.tool_call_id)
            .field("attempt", &self.attempt)
            .field("queue_length", &self.queue_length)
            .field("stop_reason", &self.stop_reason)
            .field("duration_ms", &self.duration_ms)
            .finish()
    }
}

impl RuntimeEvent {
    /// Creates a safe event containing only identifiers, enums, counts, and timings.
    #[must_use]
    pub fn new(
        kind: RuntimeEventKind,
        runtime_key: &RuntimeKey,
        connection_id: &NonEmptyString,
        turn_lease_generation: u64,
    ) -> Self {
        Self {
            kind,
            runtime_key: runtime_key.into(),
            connection_id: connection_id.as_str().to_owned(),
            turn_lease_generation,
            run_id: None,
            provider: None,
            tool_call_id: None,
            attempt: None,
            queue_length: None,
            stop_reason: None,
            duration_ms: None,
        }
    }

    /// Adds the stable run identifier.
    #[must_use]
    pub fn with_run(mut self, value: &RunId) -> Self {
        self.run_id = Some(value.as_str().to_owned());
        self
    }

    /// Adds the bounded provider identifier.
    #[must_use]
    pub fn with_provider(mut self, value: &NonEmptyString) -> Self {
        self.provider = Some(value.as_str().to_owned());
        self
    }

    /// Adds the stable tool-call identifier.
    #[must_use]
    pub fn with_tool_call(mut self, value: &ToolCallId) -> Self {
        self.tool_call_id = Some(value.as_str().to_owned());
        self
    }

    /// Adds a one-based attempt number.
    #[must_use]
    pub const fn with_attempt(mut self, value: u32) -> Self {
        self.attempt = Some(value);
        self
    }

    /// Adds an authoritative queue length, saturated for stable wire width.
    #[must_use]
    pub fn with_queue_length(mut self, value: usize) -> Self {
        self.queue_length = Some(u32::try_from(value).unwrap_or(u32::MAX));
        self
    }

    /// Adds a stable stop reason.
    #[must_use]
    pub const fn with_stop_reason(mut self, value: ObservedStopReason) -> Self {
        self.stop_reason = Some(value);
        self
    }

    /// Adds a nonnegative operation duration.
    #[must_use]
    pub const fn with_duration_ms(mut self, value: u64) -> Self {
        self.duration_ms = Some(value);
        self
    }
}

/// Bounded non-blocking fanout; each child failure is isolated from runtime behavior.
#[derive(Default)]
pub struct FanoutRuntimeEventSink {
    sinks: Vec<Arc<dyn RuntimeEventSink>>,
}

impl FanoutRuntimeEventSink {
    /// Creates a fanout from at most eight explicitly configured sinks.
    #[must_use]
    pub fn new(mut sinks: Vec<Arc<dyn RuntimeEventSink>>) -> Self {
        sinks.truncate(8);
        Self { sinks }
    }
}

impl RuntimeEventSink for FanoutRuntimeEventSink {
    fn try_emit(&self, event: RuntimeEvent) -> Result<(), EventSinkError> {
        let mut failure = None;
        for sink in &self.sinks {
            if let Err(error) = sink.try_emit(event.clone()) {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

/// Non-blocking event sink; implementations must bound retained work.
pub trait RuntimeEventSink: Send + Sync {
    /// Attempts to retain one event without changing runtime behavior on failure.
    ///
    /// # Errors
    /// Returns `Full` for backpressure or `Unavailable` for inaccessible sink state.
    fn try_emit(&self, event: RuntimeEvent) -> Result<(), EventSinkError>;
}

/// Stable event sink failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EventSinkError {
    /// Sink capacity is exhausted and backpressure was applied.
    #[error("runtime event sink full")]
    Full,
    /// Sink state is unavailable.
    #[error("runtime event sink unavailable")]
    Unavailable,
}

/// Inert sink used when no adapter is configured.
#[derive(Debug, Default)]
pub struct InertRuntimeEventSink;

impl RuntimeEventSink for InertRuntimeEventSink {
    fn try_emit(&self, _: RuntimeEvent) -> Result<(), EventSinkError> {
        Ok(())
    }
}

/// Concurrent bounded event sink useful for adapters and deterministic tests.
#[derive(Debug)]
pub struct BoundedEventSink {
    capacity: usize,
    events: Mutex<Vec<RuntimeEvent>>,
}

impl BoundedEventSink {
    /// Creates a sink whose capacity cannot exceed the runtime safety maximum.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.min(OBSERVE_EVENTS_MAX),
            events: Mutex::new(Vec::new()),
        }
    }

    /// Returns a deterministic retained snapshot in emission order.
    ///
    /// # Errors
    /// Returns `Unavailable` when concurrent sink state is inaccessible.
    pub fn snapshot(&self) -> Result<Vec<RuntimeEvent>, EventSinkError> {
        self.events
            .lock()
            .map(|events| events.clone())
            .map_err(|_| EventSinkError::Unavailable)
    }
}

impl RuntimeEventSink for BoundedEventSink {
    fn try_emit(&self, event: RuntimeEvent) -> Result<(), EventSinkError> {
        let mut events = self
            .events
            .lock()
            .map_err(|_| EventSinkError::Unavailable)?;
        if events.len() >= self.capacity {
            return Err(EventSinkError::Full);
        }
        events.push(event);
        Ok(())
    }
}

pub(crate) fn inert_sink() -> Arc<dyn RuntimeEventSink> {
    Arc::new(InertRuntimeEventSink)
}
