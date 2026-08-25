use super::{ApprovalJournal, ApprovalRequest, ApprovalState};
use crate::RuntimeError;
use lotta_domain::{BoundedJsonValue, NonEmptyString, RuntimeScope};
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
        if waiters
            .insert(
                key,
                Waiter {
                    revision: canonical.revision,
                    sender,
                },
            )
            .is_some()
        {
            return Err(conflict("approval waiter exists"));
        }
        Ok(receiver)
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

    fn remove_waiter(&self, request: &ApprovalRequest) -> Result<(), RuntimeError> {
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
        let _transaction = self.transaction()?;
        Ok(self
            .list_requests_unlocked(scope)?
            .into_iter()
            .filter(|request| {
                request.state == ApprovalState::Pending
                    && lease_generation.is_none_or(|lease| request.lease_generation == lease)
            })
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

    /// Returns the number of pending or executing approvals retaining this runtime.
    ///
    /// # Errors
    /// Returns durable journal failures.
    pub fn residency_count(&self, scope: &RuntimeScope) -> Result<usize, RuntimeError> {
        self.list_requests(scope).map(|requests| {
            requests
                .iter()
                .filter(|request| {
                    matches!(
                        request.state,
                        ApprovalState::Pending | ApprovalState::Executing
                    )
                })
                .count()
        })
    }

    /// Recovers one newly-created runtime exactly once.
    ///
    /// Pending requests are safe only when their exact continuation can be reconstructed for the
    /// same live lease. Production runtime recreation cannot do so, and executing requests are
    /// always uncertain.
    ///
    /// # Errors
    /// Returns revision overflow or durable journal failures.
    pub fn restart(&self, scope: &RuntimeScope) -> Result<Vec<ApprovalRequest>, RuntimeError> {
        let _transaction = self.transaction()?;
        let mut interrupted = Vec::new();
        for request in self.journal.port().list(scope)? {
            if !matches!(
                request.state,
                ApprovalState::Pending | ApprovalState::Executing
            ) {
                continue;
            }
            let mut next = request.clone();
            next.state = ApprovalState::Interrupted;
            next.revision = next
                .revision
                .checked_add(1)
                .ok_or_else(|| conflict("approval revision"))?;
            if self
                .journal
                .port()
                .compare_and_set(request.revision, next.clone())?
            {
                interrupted.push(next);
            }
        }
        Ok(interrupted)
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
        if request.state != expected {
            return Ok(false);
        }
        let mut next = request.clone();
        next.state = state;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| conflict("approval revision"))?;
        let _transaction = self.transaction()?;
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
