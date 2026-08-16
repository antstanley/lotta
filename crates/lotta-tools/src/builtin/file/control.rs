use super::fs::FileError;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) struct OperationControl {
    cancellation: CancellationToken,
    deadline: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControlError {
    Interrupted,
    Timeout,
}

impl OperationControl {
    pub(super) fn new(
        cancellation: CancellationToken,
        duration: Duration,
    ) -> Result<Self, FileError> {
        let deadline = Instant::now()
            .checked_add(duration)
            .ok_or(FileError::Tool)?;
        Ok(Self {
            cancellation,
            deadline,
        })
    }

    pub(super) fn check(&self) -> Result<(), FileError> {
        if self.cancellation.is_cancelled() {
            Err(FileError::Control(ControlError::Interrupted))
        } else if Instant::now() >= self.deadline {
            Err(FileError::Control(ControlError::Timeout))
        } else {
            Ok(())
        }
    }
}
