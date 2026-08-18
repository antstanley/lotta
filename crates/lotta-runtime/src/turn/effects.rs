use super::TurnProjection;
use super::{TurnStopReason, TurnStopRecord};
use crate::RuntimeError;
use crate::ports::{StopReason, ToolCallId, ToolInputSchema, ToolOutcome, ValidatedToolInput};
use crate::retry::RetryEvent;
use lotta_domain::{NonEmptyString, RuntimeScope};

/// One normalized result associated with its stable provider call identifier.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResultRecord {
    /// Stable call identifier from the provider stream.
    pub call_id: ToolCallId,
    /// Normalized bounded tool outcome.
    pub outcome: ToolOutcome,
}

/// Durable, bounded request for Task58-owned transcript compaction mechanics.
#[derive(Clone, Debug, PartialEq)]
pub struct CompactionRequest {
    /// Exact runtime owner scope.
    pub scope: RuntimeScope,
    /// Captured lease generation.
    pub lease_generation: u64,
    /// Stable bounded reason.
    pub reason: NonEmptyString,
    /// Estimated tokens before compaction.
    pub tokens_before: u64,
    /// Model-visible message count before compaction.
    pub messages_before: usize,
}

/// Ordered event emitted by the owner-local turn boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum TurnEvent {
    /// A durable approval request ready for its runtime owner.
    ControlRequest(ControlRequest),
    /// One retry/fallback status before its associated action.
    Retry(RetryEvent),
    /// One provider projection in arrival order.
    StreamDelta(TurnProjection),
    /// One completed tool result in tool-call end order.
    ToolResult(ToolResultRecord),
    /// Sole successful terminal event.
    Finished {
        /// Provider-normalized successful stop reason.
        reason: StopReason,
    },
    /// Sole canonical cancellation terminal event.
    Cancelled,
    /// Sole unsuccessful typed terminal event.
    Failed {
        /// Stable persisted failure reason.
        reason: TurnStopReason,
    },
}

/// Durable external-controller request correlated to exact owner and invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct ControllerToolRequestRecord {
    /// Exact runtime owner scope.
    pub scope: RuntimeScope,
    /// Exact run identity.
    pub run_id: lotta_domain::RunId,
    /// Captured lifecycle lease generation.
    pub lease_generation: u64,
    /// Provider call identifier.
    pub call_id: ToolCallId,
    /// Model-facing tool name.
    pub tool_name: NonEmptyString,
    /// Original validated bounded input.
    pub input: ValidatedToolInput,
}

/// Durable approval request correlated to the exact turn lease and tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlRequest {
    /// Stable request identifier.
    pub request_id: NonEmptyString,
    /// Provider tool-call identifier.
    pub call_id: ToolCallId,
    /// Captured lifecycle lease generation.
    pub lease_generation: u64,
    /// Exact model-facing tool name.
    pub tool_name: NonEmptyString,
    /// Original validated invocation.
    pub input: ValidatedToolInput,
    /// Registration schema used to validate edited approval input.
    pub schema: ToolInputSchema,
}

/// Synchronous actor-local effects applied only after a live lease check.
pub trait TurnEffectPort: Send + Sync {
    /// Persists one bounded projection.
    ///
    /// # Errors
    /// Returns an owner-local effect failure.
    fn persist_projection(&self, projection: TurnProjection) -> Result<(), RuntimeError>;
    /// Persists one typed terminal record before its client terminal event.
    ///
    /// # Errors
    /// Returns an owner-local effect failure.
    fn persist_stop_reason(&self, record: TurnStopRecord) -> Result<(), RuntimeError>;
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
    /// Persists a control request before it is emitted.
    ///
    /// # Errors
    /// Returns an owner-local effect failure.
    fn persist_control_request(&self, request: &ControlRequest) -> Result<(), RuntimeError>;
    /// Persists an external-controller request before emit, waiter registration, or wait.
    ///
    /// # Errors
    /// Returns an owner-local effect failure.
    fn persist_controller_request(
        &self,
        request: ControllerToolRequestRecord,
    ) -> Result<(), RuntimeError>;
    /// Persists a compaction request before broker emission.
    ///
    /// # Errors
    /// Returns an owner-local effect failure.
    fn persist_compaction_request(&self, request: &CompactionRequest) -> Result<(), RuntimeError>;
}
