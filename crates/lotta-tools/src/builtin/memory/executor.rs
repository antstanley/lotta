use super::{MemoryState, operations};
use crate::pipeline::{
    ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
};
use lotta_runtime::ports::{ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage};
use std::sync::Arc;

pub(super) struct MemoryExecutor {
    state: Arc<MemoryState>,
}

impl MemoryExecutor {
    pub(super) const fn new(state: Arc<MemoryState>) -> Self {
        Self { state }
    }
}

impl ToolExecutor for MemoryExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return Ok(RawToolOutcome::Failure(failure(
                    "interrupted",
                    "Memory operation interrupted.",
                )?));
            }
            let result = operations::execute(
                &state,
                request.definition.internal_name.as_str(),
                request.input.as_value(),
            )
            .await;
            match result {
                Ok(message) => Ok(RawToolOutcome::Success(message)),
                Err(operations::MemoryError::Invalid) => Ok(RawToolOutcome::Failure(failure(
                    "memory_error",
                    "Memory operation failed.",
                )?)),
                Err(operations::MemoryError::Infrastructure) => Ok(RawToolOutcome::Failure(
                    failure("infrastructure_error", "Memory infrastructure failed.")?,
                )),
            }
        })
    }
}

fn failure(code: &str, message: &str) -> Result<ToolOutcome, ExecutorError> {
    Ok(ToolOutcome::ToolDefinedError {
        code: ToolOutcomeCode::new(code.into()).map_err(|_| ExecutorError)?,
        message: ToolOutcomeMessage::new(message.into()).map_err(|_| ExecutorError)?,
    })
}
