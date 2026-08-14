//! Distinct bounded in-memory fakes for every effect port.

mod process;
mod provider;
mod stores;
mod tool;

pub use crate::clock::FakeClock;
pub use crate::ids::DeterministicIdGenerator as FakeIdGenerator;
pub use process::{FakeChildProcess, FakeSandbox};
pub use provider::FakeProvider;
pub use stores::{FakeAgentStore, FakeConversationStore, FakeMemFs, FakeTranscriptStore};
pub use tool::{FakeTool, ToolInputBytes, ToolObservation};

pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) fn not_found(context: &str) -> lotta_runtime::RuntimeError {
    lotta_runtime::RuntimeError::NotFound {
        context: context.into(),
    }
}

pub(crate) fn cancelled(context: &str) -> lotta_runtime::RuntimeError {
    lotta_runtime::RuntimeError::Cancelled {
        context: context.into(),
    }
}

pub(crate) fn limit(context: &str) -> lotta_runtime::RuntimeError {
    lotta_runtime::RuntimeError::LimitExceeded {
        context: context.into(),
    }
}
