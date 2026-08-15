use super::PortFuture;
use lotta_domain::{AgentId, ConversationId, MessageId, RunId};
use uuid::Uuid;

/// Generates every random or monotonic identifier needed by runtime workflows.
///
/// # Preconditions
/// Calls must request an ID only when the corresponding entity is ready to be created.
///
/// # Errors
/// Futures return [`crate::RuntimeError`] when entropy, sequence persistence, or domain ID
/// validation fails.
///
/// # Cancellation
/// Generation is non-streaming and deliberately has no cancellation token; implementations must
/// complete promptly and must not perform materially blocking work on the caller's async task.
///
/// # Ownership
/// Each method returns a newly owned domain ID. Implementations retain only generator state.
pub trait IdGenerator: Send + Sync {
    /// Generates a new agent ID.
    ///
    /// The returned future borrows the generator and yields one owned, unique ID. Dropping the
    /// future cancels the request; consumed sequence or entropy values need not be reused.
    fn agent_id(&self) -> PortFuture<'_, AgentId>;

    /// Generates a new non-default conversation ID.
    ///
    /// The returned future yields one owned ID. Dropping it cancels the request; a reserved
    /// monotonic value may remain consumed.
    fn conversation_id(&self) -> PortFuture<'_, ConversationId>;

    /// Generates a new transcript-local message ID.
    ///
    /// The returned future yields one owned ID. Dropping it cancels the request; a reserved
    /// monotonic value may remain consumed.
    fn message_id(&self) -> PortFuture<'_, MessageId>;

    /// Generates a new run ID.
    ///
    /// The returned future yields one owned ID. Dropping it cancels the request; consumed entropy
    /// or sequence values need not be reused.
    fn run_id(&self) -> PortFuture<'_, RunId>;

    /// Generates a new diagnostic incident UUID.
    ///
    /// Callers must retain this identifier only in protected diagnostics, never client envelopes.
    fn incident_id(&self) -> PortFuture<'_, Uuid>;

    /// Generates the unique owner UUID injected into a new turn lifecycle.
    ///
    /// The caller must not reuse the returned UUID for another live lifecycle owner.
    fn turn_lifecycle_owner_id(&self) -> PortFuture<'_, Uuid>;
}
