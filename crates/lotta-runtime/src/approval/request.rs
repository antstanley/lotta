use crate::RuntimeError;
use crate::boundary::ProviderName;
use crate::ports::{ToolCallId, ToolInputSchema, ValidatedToolInput};
use lotta_domain::{NonEmptyString, RunId, RuntimeScope, Timestamp};
use serde::{Deserialize, Serialize};

/// Durable lifecycle of one approval request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    /// Awaiting an exact resolution.
    Pending,
    /// Execution was claimed by an allow resolution.
    Executing,
    /// Execution completed and the call is allowed.
    Allowed,
    /// The user denied execution.
    Denied,
    /// The approval reached its deadline.
    Expired,
    /// Recovery could not safely reconstruct continuation.
    Interrupted,
    /// Cancellation aborted the active turn.
    Aborted,
}

/// Canonical bounded durable approval record.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApprovalRequest {
    /// Stable request identifier.
    pub request_id: NonEmptyString,
    /// Provider tool-call identifier.
    pub tool_call_id: NonEmptyString,
    /// Exact runtime owner.
    pub scope: RuntimeScope,
    /// Exact provider run.
    pub run_id: RunId,
    /// Stable turn identity.
    pub turn_id: NonEmptyString,
    /// Stable input identity.
    pub input_id: NonEmptyString,
    /// Captured lease generation.
    pub lease_generation: u64,
    /// Original model-facing tool name.
    pub tool_name: NonEmptyString,
    /// Original validated input.
    pub original_input: ValidatedToolInput,
    /// Original registration schema.
    pub original_schema: ToolInputSchema,
    /// Durable creation timestamp.
    pub created_at: Timestamp,
    /// Durable expiry timestamp.
    pub expires_at: Timestamp,
    /// Current lifecycle state.
    pub state: ApprovalState,
    /// Monotonic optimistic-concurrency revision.
    pub revision: u64,
}

impl ApprovalRequest {
    /// Returns the typed provider call identifier.
    ///
    /// # Errors
    /// Rejects an invalid stored identifier.
    pub fn call_id(&self) -> Result<ToolCallId, RuntimeError> {
        ProviderName::new(self.tool_call_id.as_str().to_owned()).map(ToolCallId::from_name)
    }
}

/// Atomic durable journal indexed by runtime scope and request identifier.
pub trait ApprovalJournalPort: Send + Sync {
    /// Inserts a new request idempotently, enforcing the per-runtime pending bound.
    ///
    /// # Errors
    /// Returns a durable, conflict, or invariant failure.
    fn insert(&self, request: ApprovalRequest) -> Result<ApprovalRequest, RuntimeError>;
    /// Loads one exact request.
    ///
    /// # Errors
    /// Returns a durable journal failure.
    fn get(
        &self,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
    ) -> Result<Option<ApprovalRequest>, RuntimeError>;
    /// Atomically replaces `expected_revision` with `next`.
    ///
    /// # Errors
    /// Returns a durable journal failure.
    fn compare_and_set(
        &self,
        expected_revision: u64,
        next: ApprovalRequest,
    ) -> Result<bool, RuntimeError>;
    /// Lists bounded records for one runtime.
    ///
    /// # Errors
    /// Returns a durable journal failure.
    fn list(&self, scope: &RuntimeScope) -> Result<Vec<ApprovalRequest>, RuntimeError>;
    /// Removes one finalized record from the journal.
    ///
    /// # Errors
    /// Returns a durable journal failure.
    fn remove(
        &self,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
    ) -> Result<bool, RuntimeError>;
}

/// Dependency-neutral approval journal handle.
#[derive(Clone)]
pub struct ApprovalJournal {
    port: std::sync::Arc<dyn ApprovalJournalPort>,
}

impl ApprovalJournal {
    /// Wraps a production or test journal implementation.
    #[must_use]
    pub fn new(port: std::sync::Arc<dyn ApprovalJournalPort>) -> Self {
        Self { port }
    }

    pub(crate) fn port(&self) -> &dyn ApprovalJournalPort {
        self.port.as_ref()
    }
}
