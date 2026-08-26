use super::{ApprovalJournal, ApprovalRequest, ApprovalState};
use crate::RuntimeError;
use lotta_domain::RuntimeScope;

use super::APPROVAL_SYNC_REPLAY_MAX;

/// Explicit restart/reconnect action for one durable approval.
#[derive(Clone, Debug)]
pub enum RecoveryAction {
    /// Re-emit a still-pending request on reconnect.
    Replay(ApprovalRequest),
    /// Emit an explicit expired terminal state.
    Expired(ApprovalRequest),
    /// Emit an explicit interrupted terminal state while retaining the original durable state.
    Interrupted {
        /// Exact record before restart recovery mutated it.
        original: ApprovalRequest,
        /// Durable interrupted record published to clients.
        interrupted: Box<ApprovalRequest>,
    },
}

/// Recovery policy over the durable journal.
pub struct ApprovalRecovery {
    journal: ApprovalJournal,
}

impl ApprovalRecovery {
    /// Creates recovery over one durable journal.
    #[must_use]
    pub fn new(journal: ApprovalJournal) -> Self {
        Self { journal }
    }

    /// Replays unresolved and explicit terminal approval states on reconnect.
    ///
    /// # Errors
    /// Returns a durable journal failure.
    pub fn reconnect(&self, scope: &RuntimeScope) -> Result<Vec<RecoveryAction>, RuntimeError> {
        self.journal.port().list(scope).map(|mut requests| {
            requests.sort_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then_with(|| left.request_id.as_str().cmp(right.request_id.as_str()))
            });
            requests
                .into_iter()
                .filter_map(|request| match request.state {
                    ApprovalState::Pending => Some(RecoveryAction::Replay(request)),
                    ApprovalState::Expired => Some(RecoveryAction::Expired(request)),
                    ApprovalState::Interrupted => Some(RecoveryAction::Interrupted {
                        original: request.clone(),
                        interrupted: Box::new(request),
                    }),
                    _ => None,
                })
                .take(APPROVAL_SYNC_REPLAY_MAX)
                .collect()
        })
    }

    /// Recovers restart state without guessing whether an executing call ran.
    ///
    /// # Errors
    /// Returns durable conflict or journal failures.
    pub fn restart(&self, scope: &RuntimeScope) -> Result<Vec<RecoveryAction>, RuntimeError> {
        let mut actions = Vec::new();
        for request in self.journal.port().list(scope)? {
            match request.state {
                ApprovalState::Pending | ApprovalState::Executing => {
                    let interrupted = self.interrupt(&request)?;
                    actions.push(RecoveryAction::Interrupted {
                        original: request,
                        interrupted: Box::new(interrupted),
                    });
                }
                _ => {}
            }
        }
        Ok(actions)
    }

    fn interrupt(&self, request: &ApprovalRequest) -> Result<ApprovalRequest, RuntimeError> {
        let mut next = request.clone();
        next.state = ApprovalState::Interrupted;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Conflict {
                context: "approval revision".into(),
            })?;
        if self
            .journal
            .port()
            .compare_and_set(request.revision, next.clone())?
        {
            Ok(next)
        } else {
            Err(RuntimeError::Conflict {
                context: "approval recovery revision".into(),
            })
        }
    }

    /// Marks one pending request explicitly expired at its cancellation-aware deadline.
    ///
    /// # Errors
    /// Returns a stale-state, revision, or durable journal failure.
    pub fn expire(&self, request: &ApprovalRequest) -> Result<ApprovalRequest, RuntimeError> {
        if request.state != ApprovalState::Pending {
            return Err(RuntimeError::Conflict {
                context: "approval expiry state".into(),
            });
        }
        let mut next = request.clone();
        next.state = ApprovalState::Expired;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Conflict {
                context: "approval revision".into(),
            })?;
        if self
            .journal
            .port()
            .compare_and_set(request.revision, next.clone())?
        {
            Ok(next)
        } else {
            Err(RuntimeError::Conflict {
                context: "approval expiry revision".into(),
            })
        }
    }
}
