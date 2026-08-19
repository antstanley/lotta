//! Durable approval ownership, resolution, timeout, and recovery.

mod recovery;
mod request;
mod resolve;

#[cfg(test)]
mod tests;

pub use recovery::{ApprovalRecovery, RecoveryAction};
pub use request::{ApprovalJournal, ApprovalJournalPort, ApprovalRequest, ApprovalState};
pub use resolve::{
    ApprovalManager, ApprovalResolution, ApprovalResolutionInput, ApprovalResolveOutcome,
    EditedInputValidator,
};

/// Maximum pending approvals retained by one conversation runtime.
pub const PENDING_APPROVALS_PER_RUNTIME_MAX: usize = 128;
/// Maximum durable replay terminal approvals retained by the journal.
pub const APPROVAL_TERMINAL_REPLAY_MAX: usize = 128;
/// Maximum approval events replayed by one synchronization.
pub const APPROVAL_SYNC_REPLAY_MAX: usize = 256;
/// Maximum time an approval may remain pending before explicit interruption.
pub use crate::bounds::APPROVAL_WAIT_MS_MAX;
