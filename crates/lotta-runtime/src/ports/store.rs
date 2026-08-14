use super::PortFuture;
use lotta_domain::{Agent, AgentId, Conversation, ConversationId};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Persists and resolves canonical agent records.
///
/// # Preconditions
/// Inputs are domain-validated and conflicting mutations are serialized per agent. List senders are
/// bounded channels chosen by callers to impose backpressure.
///
/// # Errors
/// Reports absence, conflicts, stored-data validation, limits, permissions, cancellation, channel
/// closure, and translated storage failures as [`crate::RuntimeError`].
///
/// # Cancellation
/// Scalar operations are atomic under future cancellation. Listing observes the explicit token and
/// may have emitted a prefix before returning cancellation.
///
/// # Ownership
/// Scalar reads and streamed items are owned snapshots. Save inputs are borrowed only for the
/// returned future's lifetime.
pub trait AgentStore: Send + Sync {
    /// Loads one owned agent snapshot.
    fn load(&self, id: &AgentId) -> PortFuture<'_, Agent>;
    /// Streams agents in deterministic adapter-defined order.
    fn list(&self, items: Sender<Agent>, cancellation: CancellationToken) -> PortFuture<'_, ()>;
    /// Atomically creates or replaces one agent.
    fn save(&self, agent: &Agent) -> PortFuture<'_, ()>;
    /// Deletes one agent without cascading across ports.
    fn delete(&self, id: &AgentId) -> PortFuture<'_, ()>;
}

/// Persists and resolves agent-scoped canonical conversation records.
///
/// # Preconditions
/// Inputs are domain-validated and preserve `(AgentId, ConversationId)` scope. List senders are
/// bounded channels selected by callers.
///
/// # Errors
/// Reports absence, cross-agent access, conflicts, invalid stored data, limits, permissions,
/// cancellation, channel closure, and translated storage failures.
///
/// # Cancellation
/// Mutations preserve an old or complete new snapshot. Listing checks its explicit token and may
/// have emitted a valid prefix before cancellation.
///
/// # Ownership
/// Loads and streamed records are owned. Save inputs and IDs are borrowed only while their futures
/// remain alive.
pub trait ConversationStore: Send + Sync {
    /// Loads one conversation under its agent scope.
    fn load(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
    ) -> PortFuture<'_, Conversation>;
    /// Streams conversations for one agent in deterministic order.
    fn list_for_agent(
        &self,
        agent_id: &AgentId,
        items: Sender<Conversation>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()>;
    /// Atomically creates or replaces one scoped conversation.
    fn save(&self, conversation: &Conversation) -> PortFuture<'_, ()>;
    /// Deletes one conversation in the supplied agent scope.
    fn delete(&self, agent_id: &AgentId, conversation_id: &ConversationId) -> PortFuture<'_, ()>;
}
