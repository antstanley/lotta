use super::TurnProjection;
use crate::RuntimeError;
use crate::ports::{StopReason, ToolCallId, ToolOutcome};

/// One normalized result associated with its stable provider call identifier.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResultRecord {
    /// Stable call identifier from the provider stream.
    pub call_id: ToolCallId,
    /// Normalized bounded tool outcome.
    pub outcome: ToolOutcome,
}

/// Ordered event emitted by the owner-local turn boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum TurnEvent {
    /// One provider projection in arrival order.
    StreamDelta(TurnProjection),
    /// One completed tool result in tool-call end order.
    ToolResult(ToolResultRecord),
    /// Sole successful terminal event.
    Finished {
        /// Provider-normalized successful stop reason.
        reason: StopReason,
    },
}

/// Synchronous actor-local effects applied only after a live lease check.
pub trait TurnEffectPort: Send + Sync {
    /// Persists one bounded projection.
    ///
    /// # Errors
    /// Returns an owner-local effect failure.
    fn persist_projection(&self, projection: TurnProjection) -> Result<(), RuntimeError>;
    /// Emits one typed event.
    ///
    /// # Errors
    /// Returns an owner-local effect failure.
    fn emit(&self, event: TurnEvent) -> Result<(), RuntimeError>;
    /// Appends one normalized tool result to owner-local turn records.
    ///
    /// # Errors
    /// Returns an owner-local effect failure.
    fn append_tool_result(&self, result: ToolResultRecord) -> Result<(), RuntimeError>;
}
