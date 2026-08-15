//! Deterministic domain identifier generation.

use lotta_domain::{AgentId, ConversationId, MessageId, RunId};
use lotta_runtime::{RuntimeError, ports::IdGenerator};
use std::sync::{Mutex, MutexGuard};
use uuid::Uuid;

#[derive(Debug)]
struct State {
    next: Option<u64>,
}

/// Reproducible, thread-safe generator for every Task 06 identifier method.
#[derive(Debug)]
pub struct DeterministicIdGenerator {
    state: Mutex<State>,
}

impl Default for DeterministicIdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl DeterministicIdGenerator {
    /// Creates a fresh generator whose first sequence value is one.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(State { next: Some(1) }),
        }
    }

    /// Creates a generator at an explicit next sequence for boundary tests.
    #[must_use]
    pub const fn with_next(next: u64) -> Self {
        Self {
            state: Mutex::new(State { next: Some(next) }),
        }
    }

    fn reserve(&self) -> Result<u64, RuntimeError> {
        let mut state = self.lock();
        let value = state.next.ok_or_else(exhausted)?;
        state.next = value.checked_add(1);
        Ok(value)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

fn exhausted() -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: "deterministic_id_sequence".into(),
    }
}

fn domain(error: lotta_domain::DomainError) -> RuntimeError {
    let code = error.code();
    drop(error);
    RuntimeError::InvalidData {
        context: code.into(),
    }
}

fn uuid_v4(sequence: u64) -> Uuid {
    let payload = u128::from(sequence) & 0x0000_0000_0000_0fff_3fff_ffff_ffff_ffff;
    Uuid::from_u128(payload | 0x0000_0000_0000_4000_8000_0000_0000_0000)
}

impl IdGenerator for DeterministicIdGenerator {
    fn agent_id(&self) -> lotta_runtime::ports::PortFuture<'_, AgentId> {
        Box::pin(async move {
            let sequence = self.reserve()?;
            AgentId::generate(uuid_v4(sequence)).map_err(domain)
        })
    }

    fn conversation_id(&self) -> lotta_runtime::ports::PortFuture<'_, ConversationId> {
        Box::pin(async move { ConversationId::generate(self.reserve()?).map_err(domain) })
    }

    fn message_id(&self) -> lotta_runtime::ports::PortFuture<'_, MessageId> {
        Box::pin(async move { MessageId::generate_local(self.reserve()?).map_err(domain) })
    }

    fn run_id(&self) -> lotta_runtime::ports::PortFuture<'_, RunId> {
        Box::pin(async move { RunId::generate_uuid(uuid_v4(self.reserve()?)).map_err(domain) })
    }

    fn incident_id(&self) -> lotta_runtime::ports::PortFuture<'_, Uuid> {
        Box::pin(async move { Ok(uuid_v4(self.reserve()?)) })
    }

    fn turn_lifecycle_owner_id(&self) -> lotta_runtime::ports::PortFuture<'_, Uuid> {
        Box::pin(async move { Ok(uuid_v4(self.reserve()?)) })
    }
}

#[cfg(test)]
mod tests {
    use super::DeterministicIdGenerator;
    use lotta_runtime::ports::IdGenerator;

    #[tokio::test]
    async fn deterministic_unique_uuid_v4_and_atomic_exhaustion() {
        let first = DeterministicIdGenerator::new();
        let second = DeterministicIdGenerator::new();
        assert_eq!(
            first.agent_id().await.expect("first"),
            second.agent_id().await.expect("second")
        );
        let exhausted = DeterministicIdGenerator::with_next(u64::MAX);
        assert!(exhausted.agent_id().await.is_ok());
        assert!(exhausted.agent_id().await.is_err());
        assert!(exhausted.agent_id().await.is_err());
    }

    #[tokio::test]
    async fn incident_ids_are_unique_deterministic_uuid_v4() {
        let ids = DeterministicIdGenerator::new();
        let first = ids.incident_id().await.expect("first incident");
        let second = ids.incident_id().await.expect("second incident");
        assert_ne!(first, second);
        assert_eq!(first.get_version(), Some(uuid::Version::Random));
        assert_eq!(second.get_version(), Some(uuid::Version::Random));
    }
}
