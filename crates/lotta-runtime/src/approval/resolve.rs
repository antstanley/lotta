use super::{
    APPROVAL_SYNC_REPLAY_MAX, ApprovalJournal, ApprovalRequest, ApprovalState, RecoveryAction,
};
use crate::RuntimeError;
use lotta_domain::{BoundedJsonValue, NonEmptyString, RuntimeScope, Timestamp};
use tokio::sync::oneshot;

/// Typed client decision for one approval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalResolution {
    /// Permit execution once.
    Allow,
    /// Return structured user denial.
    Deny,
    /// Cancel the active turn.
    Abort,
}

/// Fully correlated resolution input.
#[derive(Clone, Debug)]
pub struct ApprovalResolutionInput {
    /// Exact runtime owner.
    pub scope: RuntimeScope,
    /// Stable request identifier.
    pub request_id: NonEmptyString,
    /// Stable tool-call identifier.
    pub tool_call_id: NonEmptyString,
    /// Exact current lease generation.
    pub lease_generation: u64,
    /// Exact pending revision observed by the client.
    pub revision: u64,
    /// Typed decision.
    pub resolution: ApprovalResolution,
    /// Optional replacement input for allow.
    pub edited_input: Option<BoundedJsonValue>,
}

/// Accepted resolution delivered to the suspended turn owner.
#[derive(Clone, Debug)]
pub struct ApprovalResolveOutcome {
    /// Claimed canonical request.
    pub request: ApprovalRequest,
    /// Typed resolution.
    pub resolution: ApprovalResolution,
    /// Schema-validated replacement input.
    pub edited_input: Option<BoundedJsonValue>,
}

/// Adapter to the canonical Task33 input validator.
pub trait EditedInputValidator: Send + Sync {
    /// Validates edited input against the request's original schema.
    ///
    /// # Errors
    /// Returns a bounded-input or original-schema validation failure.
    fn validate(
        &self,
        request: &ApprovalRequest,
        input: BoundedJsonValue,
    ) -> Result<BoundedJsonValue, RuntimeError>;
}

struct Waiter {
    revision: u64,
    sender: oneshot::Sender<ApprovalResolveOutcome>,
}

/// Durable approval owner; the transient waiter map is only a delivery optimization.
pub struct ApprovalManager {
    journal: ApprovalJournal,
    validator: std::sync::Arc<dyn EditedInputValidator>,
    waiters: std::sync::Mutex<std::collections::HashMap<(RuntimeScope, String), Waiter>>,
    transaction: std::sync::Mutex<()>,
}

impl ApprovalManager {
    /// Creates a manager over one durable journal and canonical validator.
    #[must_use]
    pub fn new(
        journal: ApprovalJournal,
        validator: std::sync::Arc<dyn EditedInputValidator>,
    ) -> Self {
        Self {
            journal,
            validator,
            waiters: std::sync::Mutex::new(std::collections::HashMap::new()),
            transaction: std::sync::Mutex::new(()),
        }
    }

    /// Stores a request before any caller emits it.
    ///
    /// # Errors
    /// Returns durable conflict, invariant, or adapter failures.
    pub fn store_request(&self, request: ApprovalRequest) -> Result<ApprovalRequest, RuntimeError> {
        let _transaction = self.transaction()?;
        self.journal.port().insert(request)
    }

    /// Loads one canonical request by exact runtime identity.
    ///
    /// # Errors
    /// Returns durable journal failures.
    pub fn get_request(
        &self,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
    ) -> Result<Option<ApprovalRequest>, RuntimeError> {
        let _transaction = self.transaction()?;
        self.journal.port().get(scope, request_id)
    }

    /// Expires one exact pending request revision.
    ///
    /// # Errors
    /// Returns revision overflow or durable journal failures.
    pub fn expire(&self, request: &ApprovalRequest) -> Result<bool, RuntimeError> {
        self.transition(request, ApprovalState::Pending, ApprovalState::Expired)
    }

    /// Atomically removes the waiter and expires the exact pending revision.
    ///
    /// # Errors
    /// Returns revision overflow, durable journal, or waiter lock failures.
    pub fn expire_waiter(&self, request: &ApprovalRequest) -> Result<bool, RuntimeError> {
        let _transaction = self.transaction()?;
        let changed =
            self.transition_unlocked(request, ApprovalState::Pending, ApprovalState::Expired)?;
        if changed {
            self.remove_waiter_unlocked(request)?;
        }
        Ok(changed)
    }

    /// Aborts one exact pending request without permitting a later resolution.
    ///
    /// # Errors
    /// Returns revision overflow or durable journal failures.
    pub fn abort(&self, request: &ApprovalRequest) -> Result<bool, RuntimeError> {
        let changed = self.transition(request, ApprovalState::Pending, ApprovalState::Aborted)?;
        if changed {
            self.remove_waiter(request)?;
        }
        Ok(changed)
    }

    /// Marks an executing request interrupted during recovery.
    ///
    /// # Errors
    /// Returns revision overflow or durable journal failures.
    pub fn interrupt(&self, request: &ApprovalRequest) -> Result<bool, RuntimeError> {
        self.transition(
            request,
            ApprovalState::Executing,
            ApprovalState::Interrupted,
        )
    }

    /// Interrupts one exact pending request and removes its transient waiter.
    ///
    /// # Errors
    /// Returns revision overflow or durable journal failures.
    pub fn interrupt_pending(&self, request: &ApprovalRequest) -> Result<bool, RuntimeError> {
        let changed =
            self.transition(request, ApprovalState::Pending, ApprovalState::Interrupted)?;
        self.remove_waiter(request)?;
        Ok(changed)
    }

    /// Registers one continuation delivery channel after durable storage.
    ///
    /// # Errors
    /// Rejects missing, non-pending, duplicate, or poisoned state.
    pub fn register_waiter(
        &self,
        request: &ApprovalRequest,
    ) -> Result<oneshot::Receiver<ApprovalResolveOutcome>, RuntimeError> {
        let _transaction = self.transaction()?;
        let canonical = self
            .journal
            .port()
            .get(&request.scope, &request.request_id)?
            .ok_or_else(|| not_found("approval request"))?;
        if canonical.state != ApprovalState::Pending
            || canonical.revision != request.revision
            || canonical.tool_call_id != request.tool_call_id
            || canonical.lease_generation != request.lease_generation
        {
            return Err(conflict("approval not pending"));
        }
        let key = (
            request.scope.clone(),
            request.request_id.as_str().to_owned(),
        );
        let (sender, receiver) = oneshot::channel();
        let mut waiters = self
            .waiters
            .lock()
            .map_err(|_| adapter("approval waiter lock"))?;
        match waiters.entry(key) {
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(conflict("approval waiter exists"))
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Waiter {
                    revision: canonical.revision,
                    sender,
                });
                Ok(receiver)
            }
        }
    }

    /// Removes one exact unpublished pending request and its transient waiter.
    ///
    /// # Errors
    /// Returns a stale request, durable journal, or waiter lock failure.
    pub fn rollback_request(&self, request: &ApprovalRequest) -> Result<(), RuntimeError> {
        let _transaction = self.transaction()?;
        let canonical = self
            .journal
            .port()
            .get(&request.scope, &request.request_id)?
            .ok_or_else(|| not_found("approval request"))?;
        if canonical.state != ApprovalState::Pending
            || canonical.revision != request.revision
            || canonical.tool_call_id != request.tool_call_id
            || canonical.lease_generation != request.lease_generation
        {
            return Err(conflict("approval rollback request"));
        }
        self.remove_waiter_unlocked(request)?;
        if !self
            .journal
            .port()
            .remove(&request.scope, &request.request_id)?
        {
            return Err(conflict("approval rollback remove"));
        }
        Ok(())
    }

    /// Validates one exact pending revision without mutating durable or transient state.
    ///
    /// # Errors
    /// Wrong, stale, duplicate, or invalid resolutions return typed errors without mutation.
    pub fn validate_resolution(
        &self,
        input: &ApprovalResolutionInput,
        current_lease_generation: u64,
    ) -> Result<ApprovalResolveOutcome, RuntimeError> {
        let _transaction = self.transaction()?;
        let request = self.load_and_validate_unlocked(input, current_lease_generation)?;
        let edited_input = self.validate_edit(&request, input)?;
        Ok(ApprovalResolveOutcome {
            request,
            resolution: input.resolution,
            edited_input,
        })
    }

    /// Validates and atomically claims one exact pending revision.
    ///
    /// # Errors
    /// Wrong, stale, duplicate, or invalid resolutions return typed errors without mutation.
    pub fn resolve(
        &self,
        input: &ApprovalResolutionInput,
        current_lease_generation: u64,
    ) -> Result<ApprovalResolveOutcome, RuntimeError> {
        let _transaction = self.transaction()?;
        let request = self.load_and_validate_unlocked(input, current_lease_generation)?;
        let edited_input = self.validate_edit(&request, input)?;
        let mut claimed = request.clone();
        claimed.state = match input.resolution {
            ApprovalResolution::Allow => ApprovalState::Executing,
            ApprovalResolution::Deny => ApprovalState::Denied,
            ApprovalResolution::Abort => ApprovalState::Aborted,
        };
        claimed.revision = claimed
            .revision
            .checked_add(1)
            .ok_or_else(|| conflict("approval revision"))?;
        if !self
            .journal
            .port()
            .compare_and_set(request.revision, claimed.clone())?
        {
            return Err(conflict("approval revision stale"));
        }
        let outcome = ApprovalResolveOutcome {
            request: claimed,
            resolution: input.resolution,
            edited_input,
        };
        self.deliver(&outcome)?;
        Ok(outcome)
    }

    fn load_and_validate_unlocked(
        &self,
        input: &ApprovalResolutionInput,
        current_lease_generation: u64,
    ) -> Result<ApprovalRequest, RuntimeError> {
        let request = self
            .journal
            .port()
            .get(&input.scope, &input.request_id)?
            .ok_or_else(|| not_found("approval request"))?;
        if request.scope != input.scope || request.request_id != input.request_id {
            return Err(conflict("approval scope or request"));
        }
        if request.tool_call_id != input.tool_call_id {
            return Err(conflict("approval tool call"));
        }
        if request.lease_generation != input.lease_generation
            || request.lease_generation != current_lease_generation
        {
            return Err(conflict("approval lease"));
        }
        if request.state != ApprovalState::Pending || request.revision != input.revision {
            return Err(conflict("approval state or revision"));
        }
        Ok(request)
    }

    fn validate_edit(
        &self,
        request: &ApprovalRequest,
        input: &ApprovalResolutionInput,
    ) -> Result<Option<BoundedJsonValue>, RuntimeError> {
        if input.edited_input.is_some() && input.resolution != ApprovalResolution::Allow {
            return Err(invalid("approval edit requires allow"));
        }
        input
            .edited_input
            .clone()
            .map(|value| self.validator.validate(request, value))
            .transpose()
    }

    /// Removes the transient waiter for one exact request.
    ///
    /// Timeout and cancellation owners call this before returning so a later
    /// continuation may register without colliding with an abandoned receiver.
    ///
    /// # Errors
    /// Returns a poisoned waiter-map error.
    pub fn remove_waiter(&self, request: &ApprovalRequest) -> Result<(), RuntimeError> {
        let _transaction = self.transaction()?;
        self.remove_waiter_unlocked(request)
    }

    fn remove_waiter_unlocked(&self, request: &ApprovalRequest) -> Result<(), RuntimeError> {
        let key = (
            request.scope.clone(),
            request.request_id.as_str().to_owned(),
        );
        self.waiters
            .lock()
            .map_err(|_| adapter("approval waiter lock"))?
            .remove(&key);
        Ok(())
    }

    fn deliver(&self, outcome: &ApprovalResolveOutcome) -> Result<(), RuntimeError> {
        let key = (
            outcome.request.scope.clone(),
            outcome.request.request_id.as_str().to_owned(),
        );
        let waiter = self
            .waiters
            .lock()
            .map_err(|_| adapter("approval waiter lock"))?
            .remove(&key);
        if let Some(waiter) = waiter {
            if waiter.revision.checked_add(1) != Some(outcome.request.revision) {
                return Err(conflict("approval waiter revision"));
            }
            let _ = waiter.sender.send(outcome.clone());
        }
        Ok(())
    }

    /// Lists durable requests for one runtime in creation and request order.
    ///
    /// # Errors
    /// Returns durable journal failures.
    pub fn list_requests(
        &self,
        scope: &RuntimeScope,
    ) -> Result<Vec<ApprovalRequest>, RuntimeError> {
        let _transaction = self.transaction()?;
        self.list_requests_unlocked(scope)
    }

    /// Atomically snapshots requests that remain pending for the current lease.
    ///
    /// # Errors
    /// Returns durable journal failures.
    pub fn pending_snapshot(
        &self,
        scope: &RuntimeScope,
        lease_generation: Option<u64>,
    ) -> Result<Vec<ApprovalRequest>, RuntimeError> {
        let Some(lease_generation) = lease_generation else {
            return Ok(Vec::new());
        };
        let _transaction = self.transaction()?;
        Ok(self
            .list_requests_unlocked(scope)?
            .into_iter()
            .filter(|request| {
                request.state == ApprovalState::Pending
                    && request.lease_generation == lease_generation
            })
            .collect())
    }

    /// Snapshots replayable approval state for one reconnect.
    ///
    /// # Errors
    /// Returns durable journal failures.
    pub fn recovery_snapshot(
        &self,
        scope: &RuntimeScope,
        lease_generation: Option<u64>,
    ) -> Result<Vec<RecoveryAction>, RuntimeError> {
        let _transaction = self.transaction()?;
        Ok(self
            .list_requests_unlocked(scope)?
            .into_iter()
            .filter_map(|request| match request.state {
                ApprovalState::Pending if lease_generation == Some(request.lease_generation) => {
                    Some(RecoveryAction::Replay(request))
                }
                ApprovalState::Expired => Some(RecoveryAction::Expired(request)),
                ApprovalState::Interrupted => Some(RecoveryAction::Interrupted {
                    original: request.clone(),
                    interrupted: Box::new(request),
                }),
                _ => None,
            })
            .take(APPROVAL_SYNC_REPLAY_MAX)
            .collect())
    }

    fn list_requests_unlocked(
        &self,
        scope: &RuntimeScope,
    ) -> Result<Vec<ApprovalRequest>, RuntimeError> {
        let mut requests = self.journal.port().list(scope)?;
        requests.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.request_id.as_str().cmp(right.request_id.as_str()))
        });
        Ok(requests)
    }

    /// Returns the number of pending approvals retaining this runtime.
    ///
    /// # Errors
    /// Returns durable journal failures.
    pub fn residency_count(&self, scope: &RuntimeScope) -> Result<usize, RuntimeError> {
        self.list_requests(scope).map(|requests| {
            requests
                .iter()
                .filter(|request| request.state == ApprovalState::Pending)
                .count()
        })
    }

    /// Returns the number of pending approvals whose durable deadline has not elapsed.
    ///
    /// # Errors
    /// Returns durable journal failures.
    pub fn actionable_pending_count(
        &self,
        scope: &RuntimeScope,
        now: Timestamp,
    ) -> Result<usize, RuntimeError> {
        self.list_requests(scope).map(|requests| {
            requests
                .iter()
                .filter(|request| {
                    request.state == ApprovalState::Pending && request.expires_at > now
                })
                .count()
        })
    }

    /// Returns whether durable interrupted approval evidence retains this runtime.
    ///
    /// # Errors
    /// Returns durable journal failures.
    pub fn interrupted_result_present(&self, scope: &RuntimeScope) -> Result<bool, RuntimeError> {
        self.list_requests(scope).map(|requests| {
            requests
                .iter()
                .any(|request| request.state == ApprovalState::Interrupted)
        })
    }

    /// Recovers one newly-created runtime exactly once.
    ///
    /// Without a reconstructed owner lease, pending work becomes explicitly interrupted.
    /// Executing work is always uncertain and never reruns.
    ///
    /// # Errors
    /// Returns revision overflow or durable journal failures.
    pub fn restart(&self, scope: &RuntimeScope) -> Result<Vec<RecoveryAction>, RuntimeError> {
        self.restart_inner(scope, None, None)
    }

    /// Recovers restart state and rebinds pending requests to a reconstructed owner lease.
    ///
    /// # Errors
    /// Returns revision overflow or durable journal failures.
    pub fn restart_with_pending_lease(
        &self,
        scope: &RuntimeScope,
        pending_lease_generation: Option<u64>,
    ) -> Result<Vec<RecoveryAction>, RuntimeError> {
        self.restart_inner(scope, pending_lease_generation, None)
    }

    /// Recovers restart state while expiring pending requests at the supplied durable time.
    ///
    /// # Errors
    /// Returns revision overflow or durable journal failures.
    pub fn restart_with_pending_lease_at(
        &self,
        scope: &RuntimeScope,
        pending_lease_generation: Option<u64>,
        now: Timestamp,
    ) -> Result<Vec<RecoveryAction>, RuntimeError> {
        self.restart_inner(scope, pending_lease_generation, Some(now))
    }

    fn restart_inner(
        &self,
        scope: &RuntimeScope,
        generation: Option<u64>,
        now: Option<Timestamp>,
    ) -> Result<Vec<RecoveryAction>, RuntimeError> {
        let _transaction = self.transaction()?;
        let mut actions = Vec::new();
        for request in self.sorted_requests(scope)? {
            match request.state {
                ApprovalState::Pending => {
                    actions.push(self.recover_pending(&request, generation, now)?);
                }
                ApprovalState::Executing => {
                    let interrupted =
                        self.set_state_for_restart(&request, ApprovalState::Interrupted)?;
                    actions.push(RecoveryAction::Interrupted {
                        original: request,
                        interrupted: Box::new(interrupted),
                    });
                }
                ApprovalState::Expired => actions.push(RecoveryAction::Expired(request)),
                ApprovalState::Interrupted => actions.push(RecoveryAction::Interrupted {
                    original: request.clone(),
                    interrupted: Box::new(request),
                }),
                _ => {}
            }
        }
        Ok(actions)
    }

    fn recover_pending(
        &self,
        request: &ApprovalRequest,
        generation: Option<u64>,
        now: Option<Timestamp>,
    ) -> Result<RecoveryAction, RuntimeError> {
        if now.is_some_and(|now| request.expires_at <= now) {
            return self
                .set_state_for_restart(request, ApprovalState::Expired)
                .map(RecoveryAction::Expired);
        }
        let Some(generation) = generation else {
            let interrupted = self.set_state_for_restart(request, ApprovalState::Interrupted)?;
            return Ok(RecoveryAction::Interrupted {
                original: request.clone(),
                interrupted: Box::new(interrupted),
            });
        };
        self.rebind_pending_for_restart(request, generation)
            .map(RecoveryAction::Replay)
    }

    fn sorted_requests(&self, scope: &RuntimeScope) -> Result<Vec<ApprovalRequest>, RuntimeError> {
        let mut requests = self.journal.port().list(scope)?;
        requests.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.request_id.as_str().cmp(right.request_id.as_str()))
        });
        Ok(requests)
    }

    fn rebind_pending_for_restart(
        &self,
        request: &ApprovalRequest,
        generation: u64,
    ) -> Result<ApprovalRequest, RuntimeError> {
        let mut next = request.clone();
        next.lease_generation = generation;
        self.replace_for_restart(request, next)
    }

    fn set_state_for_restart(
        &self,
        request: &ApprovalRequest,
        state: ApprovalState,
    ) -> Result<ApprovalRequest, RuntimeError> {
        let mut next = request.clone();
        next.state = state;
        self.replace_for_restart(request, next)
    }

    fn replace_for_restart(
        &self,
        request: &ApprovalRequest,
        mut next: ApprovalRequest,
    ) -> Result<ApprovalRequest, RuntimeError> {
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| conflict("approval revision"))?;
        if self
            .journal
            .port()
            .compare_and_set(request.revision, next.clone())?
        {
            Ok(next)
        } else {
            Err(conflict("approval restart revision"))
        }
    }

    /// Makes rebound pending requests terminal when runtime initialization is rolled back.
    ///
    /// Safety recovery transitions are never reversed into possibly executable states.
    ///
    /// # Errors
    /// Returns a durable conflict or journal failure.
    pub fn rollback_restart(&self, actions: &[RecoveryAction]) -> Result<(), RuntimeError> {
        let _transaction = self.transaction()?;
        for action in actions.iter().rev() {
            let RecoveryAction::Replay(recovered) = action else {
                continue;
            };
            self.set_state_for_restart(recovered, ApprovalState::Interrupted)?;
        }
        Ok(())
    }

    /// Marks an executing request allowed after exactly-once execution completes.
    ///
    /// # Errors
    /// Returns a revision or durable journal failure.
    pub fn mark_allowed(&self, request: &ApprovalRequest) -> Result<bool, RuntimeError> {
        let changed = self.transition(request, ApprovalState::Executing, ApprovalState::Allowed)?;
        if changed {
            let _transaction = self.transaction()?;
            self.journal
                .port()
                .remove(&request.scope, &request.request_id)?;
        }
        Ok(changed)
    }

    fn transition(
        &self,
        request: &ApprovalRequest,
        expected: ApprovalState,
        state: ApprovalState,
    ) -> Result<bool, RuntimeError> {
        let _transaction = self.transaction()?;
        self.transition_unlocked(request, expected, state)
    }

    fn transition_unlocked(
        &self,
        request: &ApprovalRequest,
        expected: ApprovalState,
        state: ApprovalState,
    ) -> Result<bool, RuntimeError> {
        if request.state != expected {
            return Ok(false);
        }
        let mut next = request.clone();
        next.state = state;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| conflict("approval revision"))?;
        self.journal.port().compare_and_set(request.revision, next)
    }

    fn transaction(&self) -> Result<std::sync::MutexGuard<'_, ()>, RuntimeError> {
        self.transaction
            .lock()
            .map_err(|_| adapter("approval transaction lock"))
    }
}

fn conflict(context: &'static str) -> RuntimeError {
    RuntimeError::Conflict {
        context: context.into(),
    }
}
fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}
fn not_found(context: &'static str) -> RuntimeError {
    RuntimeError::NotFound {
        context: context.into(),
    }
}
fn adapter(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "approval_manager",
        context: context.into(),
    }
}
