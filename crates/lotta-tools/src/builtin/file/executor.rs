use super::{
    FileState,
    control::{ControlError, OperationControl},
    fs::FileError,
    operations,
};
use crate::pipeline::{
    ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
};
use lotta_runtime::ports::{ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage};
use std::sync::Arc;

pub(super) struct FileExecutor {
    state: Arc<FileState>,
}

impl FileExecutor {
    pub(super) const fn new(state: Arc<FileState>) -> Self {
        Self { state }
    }
}

impl ToolExecutor for FileExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let control =
                OperationControl::new(request.cancellation.clone(), request.deadline.get())
                    .map_err(|_| ExecutorError)?;
            let name = request.definition.internal_name.as_str().to_owned();
            let input = request.input.as_value().clone();
            match tokio::task::spawn_blocking(move || {
                operations::execute(&state, &name, &input, &control)
            })
            .await
            {
                Ok(Ok(output)) => Ok(RawToolOutcome::Success(output)),
                Ok(Err(FileError::Control(ControlError::Interrupted))) => Ok(
                    RawToolOutcome::Failure(failure("interrupted", "File operation interrupted.")?),
                ),
                Ok(Err(FileError::Control(ControlError::Timeout))) => Ok(RawToolOutcome::Failure(
                    failure("timeout", "File operation timed out.")?,
                )),
                Ok(Err(FileError::Tool)) => Ok(RawToolOutcome::Failure(failure(
                    "file_error",
                    "File operation failed.",
                )?)),
                Ok(Err(FileError::Infrastructure)) => Ok(RawToolOutcome::Failure(failure(
                    "infrastructure_error",
                    "File operation infrastructure failed.",
                )?)),
                Err(_) => Err(ExecutorError),
            }
        })
    }
}

fn failure(code: &str, message: &str) -> Result<ToolOutcome, ExecutorError> {
    Ok(ToolOutcome::ToolDefinedError {
        code: ToolOutcomeCode::new(code.to_owned()).map_err(|_| ExecutorError)?,
        message: ToolOutcomeMessage::new(message.to_owned()).map_err(|_| ExecutorError)?,
    })
}
