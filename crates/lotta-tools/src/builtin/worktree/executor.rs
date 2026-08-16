use super::WorktreeManager;
use crate::pipeline::{
    ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
};
use lotta_runtime::ports::{ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage};
use std::sync::Arc;

pub(super) struct WorktreeExecutor {
    manager: Arc<WorktreeManager>,
}

impl WorktreeExecutor {
    pub(super) const fn new(manager: Arc<WorktreeManager>) -> Self {
        Self { manager }
    }
}

impl ToolExecutor for WorktreeExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let manager = Arc::clone(&self.manager);
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return Ok(RawToolOutcome::Failure(failure(
                    "interrupted",
                    "Worktree operation interrupted.",
                )?));
            }
            let result = match request.definition.internal_name.as_str() {
                "EnterWorktree" => {
                    manager
                        .enter(request.input.as_value(), request.cancellation)
                        .await
                }
                "ExitWorktree" => {
                    manager
                        .exit(request.input.as_value(), &request.cancellation)
                        .await
                }
                _ => Err(()),
            };
            match result {
                Ok(text) => Ok(RawToolOutcome::Success(text)),
                Err(()) => Ok(RawToolOutcome::Failure(failure(
                    "worktree_error",
                    "Worktree operation failed.",
                )?)),
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
