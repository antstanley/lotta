use super::ExternalCallFailure;
use super::call::{
    ConnectionId, ControllerConnection, ControllerReceiver, ExternalCallRequest,
    ExternalCallResponse, ExternalRequestId, ResponseDisposition, RuntimeId, ScopeId, ToolCallId,
    validate_response_payload,
};
use crate::{
    ToolRegistration, ToolRegistry, ToolsetId,
    pipeline::{
        ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
    },
};
use lotta_domain::bounds::EXTERNAL_TOOLS_PER_RUNTIME_MAX;
use lotta_runtime::{
    bounds::{EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX},
    ports::{
        InternalToolName, ModelFacingToolName, ParallelSafety, PermissionAction,
        SecretRedactionPolicy, SecretRedactionSpec, ToolApprovalPolicy, ToolDefinition,
        ToolDescriptionAsset, ToolExecutionOwner, ToolInputSchema, ToolOutcome, ToolOutcomeCode,
        ToolOutcomeMessage, ToolOutputLimit, ToolTimeout,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;

const EXTERNAL_PENDING_CALLS_PER_RUNTIME_MAX: usize = 256;

/// Process-wide manager-instance salt keeping minted request IDs unique across
/// every manager in this process, mirroring the pinned controller's globally
/// unique `external-tool-{uuid}` wire identity.
static NEXT_MANAGER_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// Monotonic optimistic revision for one external group set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupRevision(u64);
impl GroupRevision {
    /// Creates a revision supplied by a transport adapter.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    /// Returns the numeric revision.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// One wire-compatible controller-owned external tool definition.
#[derive(Clone, Debug)]
pub struct ExternalToolMember {
    /// Exact internal and model-facing name from the controller.
    pub name: String,
    /// Optional human label retained for transport projections.
    pub label: Option<String>,
    /// Bounded model description.
    pub description: String,
    /// JSON Schema object for arguments.
    pub parameters: serde_json::Value,
}

/// Atomic registration group with optional exact selection scope.
#[derive(Clone, Debug)]
pub struct ExternalToolGroup {
    /// Optional hidden scope selector.
    pub scope_id: Option<String>,
    /// Members replaced atomically with the complete group set.
    pub tools: Vec<ExternalToolMember>,
}

/// Selection used to compose a real Task 32 registry snapshot.
#[derive(Clone, Copy)]
pub struct RegistrySelection<'a> {
    /// Task 32 toolset selection.
    pub toolset: ToolsetId,
    /// Optional exact scope; unscoped definitions are always included.
    pub scope_id: Option<&'a ScopeId>,
    /// Optional Task 32 allowlist.
    pub allowlist: Option<&'a [&'a str]>,
}

/// Fixed external registration failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalRegistrationError {
    /// Manager synchronization failed.
    Poisoned,
    /// Connection handle does not belong to this manager.
    ForeignConnection,
    /// Connection identity/generation is dead or superseded.
    OwnerDisconnected,
    /// Runtime ID does not match this manager's runtime collection.
    RuntimeMismatch,
    /// Group revision did not match the current revision.
    RevisionMismatch,
    /// Scope ID was invalid or repeated in one replacement.
    InvalidScope,
    /// A member name was invalid.
    InvalidName,
    /// A member description or label was invalid.
    InvalidDescription,
    /// A member JSON Schema was invalid.
    InvalidSchema,
    /// A selected name collides with a native registration.
    NativeCollision(String),
    /// A member contract differed from the fixed external definition contract.
    InvalidDefinition,
    /// Two selected definitions collide by internal/model name.
    DuplicateTool(String),
    /// More than the canonical per-runtime maximum unique tools were supplied.
    TooManyTools,
    /// Bounded allocation failed.
    Allocation,
    /// Task 32 rejected the composed registry candidate.
    Registry(crate::RegistryError),
}
impl fmt::Display for ExternalRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "external registration error: {self:?}")
    }
}
impl std::error::Error for ExternalRegistrationError {}

struct ValidatedMember {
    scope_id: Option<ScopeId>,
    registration: ToolRegistration,
}
struct OwnerIdentity {
    id: ConnectionId,
    generation: u64,
    sender: tokio::sync::mpsc::Sender<ExternalCallRequest>,
}
struct State {
    revision: GroupRevision,
    members: Vec<ValidatedMember>,
    live: BTreeSet<(ConnectionId, u64)>,
    pending: BTreeMap<String, PendingCall>,
}
struct PendingCall {
    owner: (ConnectionId, u64),
    correlation: CallCorrelation,
    completion: oneshot::Sender<Result<ResponseValue, ExternalCallFailure>>,
}
#[derive(Clone)]
struct CallCorrelation {
    runtime_id: RuntimeId,
    request_id: ExternalRequestId,
    tool_call_id: ToolCallId,
    internal_name: InternalToolName,
    model_name: ModelFacingToolName,
    scope_id: Option<ScopeId>,
}
enum ResponseValue {
    Result(serde_json::Value),
    Error(String),
}

pub(crate) struct ManagerCore {
    instance: u64,
    runtime_id: RuntimeId,
    registry: Arc<ToolRegistry>,
    mutation: Mutex<()>,
    state: Mutex<State>,
    next_connection_generation: AtomicU64,
    next_request_sequence: AtomicU64,
}

/// Explicit manager scoped to exactly one runtime collection.
pub struct ExternalToolManager {
    core: Arc<ManagerCore>,
}

impl ExternalToolManager {
    /// Production factory binding one runtime collection to its Task 32 registry.
    #[must_use]
    pub fn production(runtime_id: RuntimeId, registry: Arc<ToolRegistry>) -> Self {
        let instance = NEXT_MANAGER_INSTANCE.fetch_add(1, Ordering::Relaxed);
        Self {
            core: Arc::new(ManagerCore {
                instance,
                runtime_id,
                registry,
                mutation: Mutex::new(()),
                state: Mutex::new(State {
                    revision: GroupRevision(0),
                    members: Vec::new(),
                    live: BTreeSet::new(),
                    pending: BTreeMap::new(),
                }),
                next_connection_generation: AtomicU64::new(1),
                next_request_sequence: AtomicU64::new(1),
            }),
        }
    }

    /// Opens one bounded controller connection and registers its exact generation as live.
    ///
    /// # Errors
    /// Returns synchronization or generation exhaustion failures.
    pub fn connect(
        &self,
        id: ConnectionId,
    ) -> Result<(Arc<ControllerConnection>, ControllerReceiver), ExternalRegistrationError> {
        let generation = self
            .core
            .next_connection_generation
            .fetch_add(1, Ordering::Relaxed);
        if generation == u64::MAX {
            return Err(ExternalRegistrationError::Allocation);
        }
        let (handle, receiver) = ControllerConnection::create(id.clone(), generation, &self.core);
        let mut state = self
            .core
            .state
            .lock()
            .map_err(|_| ExternalRegistrationError::Poisoned)?;
        state.live.insert((id, generation));
        Ok((handle, receiver))
    }

    /// Replaces all groups for this runtime atomically and composes the selected Task 32 snapshot.
    ///
    /// # Errors
    /// Validates every member before mutating manager state or Task 32 registry revision.
    pub fn runtime_start(
        &self,
        connection: &Arc<ControllerConnection>,
        runtime_id: &RuntimeId,
        expected: GroupRevision,
        groups: &[ExternalToolGroup],
        selection: RegistrySelection<'_>,
    ) -> Result<GroupRevision, ExternalRegistrationError> {
        self.replace(connection, runtime_id, expected, groups, selection)
    }

    /// Applies the protocol's complete atomic replacement/removal update.
    ///
    /// # Errors
    /// Has the same all-or-nothing validation as [`Self::runtime_start`].
    pub fn update(
        &self,
        connection: &Arc<ControllerConnection>,
        runtime_id: &RuntimeId,
        expected: GroupRevision,
        groups: &[ExternalToolGroup],
        selection: RegistrySelection<'_>,
    ) -> Result<GroupRevision, ExternalRegistrationError> {
        self.replace(connection, runtime_id, expected, groups, selection)
    }

    fn replace(
        &self,
        connection: &Arc<ControllerConnection>,
        runtime_id: &RuntimeId,
        expected: GroupRevision,
        groups: &[ExternalToolGroup],
        selection: RegistrySelection<'_>,
    ) -> Result<GroupRevision, ExternalRegistrationError> {
        self.verify_connection(connection, runtime_id)?;
        let candidate = validate_groups(connection, groups)?;
        let selected = select_registrations(&candidate, selection.scope_id)?;
        let _mutation = self
            .core
            .mutation
            .lock()
            .map_err(|_| ExternalRegistrationError::Poisoned)?;
        let mut state = self
            .core
            .state
            .lock()
            .map_err(|_| ExternalRegistrationError::Poisoned)?;
        if state.revision != expected {
            return Err(ExternalRegistrationError::RevisionMismatch);
        }
        let next = expected
            .0
            .checked_add(1)
            .ok_or(ExternalRegistrationError::Allocation)?;
        let registry_revision = self.core.registry.revision().map_err(map_registry_error)?;
        self.core
            .registry
            .compose(selection.toolset, &selected, selection.allowlist)
            .map_err(map_registry_error)?;
        let snapshot = self
            .core
            .registry
            .publish(
                registry_revision,
                selection.toolset,
                &selected,
                selection.allowlist,
            )
            .map_err(map_registry_error)?;
        assert_eq!(snapshot.len(), selected.len());
        state.members = candidate;
        state.revision = GroupRevision(next);
        Ok(state.revision)
    }

    fn verify_connection(
        &self,
        connection: &Arc<ControllerConnection>,
        runtime_id: &RuntimeId,
    ) -> Result<(), ExternalRegistrationError> {
        if runtime_id != &self.core.runtime_id {
            return Err(ExternalRegistrationError::RuntimeMismatch);
        }
        let Some(manager) = connection.manager.upgrade() else {
            return Err(ExternalRegistrationError::OwnerDisconnected);
        };
        if !Arc::ptr_eq(&manager, &self.core) {
            return Err(ExternalRegistrationError::ForeignConnection);
        }
        let state = self
            .core
            .state
            .lock()
            .map_err(|_| ExternalRegistrationError::Poisoned)?;
        if !state
            .live
            .contains(&(connection.id.clone(), connection.generation))
        {
            return Err(ExternalRegistrationError::OwnerDisconnected);
        }
        Ok(())
    }

    /// Composes an immutable unscoped plus exact-scope snapshot without publishing it.
    /// Runtime scopes therefore cannot overwrite shared registry state.
    ///
    /// # Errors
    /// Leaves Task 32 unchanged on collision or registry failure.
    pub fn select(
        &self,
        selection: RegistrySelection<'_>,
    ) -> Result<Arc<crate::RegistrySnapshot>, ExternalRegistrationError> {
        let state = self
            .core
            .state
            .lock()
            .map_err(|_| ExternalRegistrationError::Poisoned)?;
        let selected = select_registrations(&state.members, selection.scope_id)?;
        self.core
            .registry
            .compose(selection.toolset, &selected, selection.allowlist)
            .map_err(map_registry_error)
    }

    /// Returns the current external group revision.
    ///
    /// # Errors
    /// Returns a synchronization failure if manager state was poisoned.
    pub fn revision(&self) -> Result<GroupRevision, ExternalRegistrationError> {
        self.core
            .state
            .lock()
            .map(|state| state.revision)
            .map_err(|_| ExternalRegistrationError::Poisoned)
    }

    /// Returns the pending call count for bounded observability.
    ///
    /// # Errors
    /// Returns a synchronization failure if manager state was poisoned.
    pub fn pending_calls(&self) -> Result<usize, ExternalRegistrationError> {
        self.core
            .state
            .lock()
            .map(|state| state.pending.len())
            .map_err(|_| ExternalRegistrationError::Poisoned)
    }

    #[cfg(test)]
    pub(crate) fn set_group_revision_for_test(&self, revision: u64) {
        self.core.state.lock().unwrap().revision = GroupRevision(revision);
    }

    #[cfg(test)]
    pub(crate) fn set_request_sequence_for_test(&self, sequence: u64) {
        self.core
            .next_request_sequence
            .store(sequence, Ordering::Relaxed);
    }
}

impl ManagerCore {
    pub(crate) fn respond(
        &self,
        connection: &ControllerConnection,
        response: ExternalCallResponse,
    ) -> Result<ResponseDisposition, ExternalCallFailure> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ExternalCallFailure::InvalidResponse)?;
        let Some(pending) = state.pending.get(response.request_id.as_str()) else {
            return Ok(ResponseDisposition::UnknownIgnored);
        };
        if pending.owner != (connection.id.clone(), connection.generation) {
            return Ok(ResponseDisposition::OwnerMismatch);
        }
        if !correlates(&pending.correlation, &response) || !validate_response_payload(&response) {
            return Ok(ResponseDisposition::InvalidResponse);
        }
        let pending = state.pending.remove(response.request_id.as_str());
        drop(state);
        let value = match (response.result, response.error) {
            (Some(value), None) => ResponseValue::Result(value),
            (None, Some(error)) => ResponseValue::Error(error),
            _ => return Err(ExternalCallFailure::InvalidResponse),
        };
        settle(pending, Ok(value));
        Ok(ResponseDisposition::Resolved)
    }

    pub(crate) fn disconnect(&self, id: &ConnectionId, generation: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.live.remove(&(id.clone(), generation));
        let keys: Vec<String> = state
            .pending
            .iter()
            .filter(|(_, pending)| pending.owner == (id.clone(), generation))
            .map(|(key, _)| key.clone())
            .collect();
        let mut removed = Vec::new();
        for key in keys {
            if let Some(call) = state.pending.remove(&key) {
                removed.push(call);
            }
        }
        drop(state);
        for call in removed {
            let _ = call
                .completion
                .send(Err(ExternalCallFailure::OwnerDisconnected));
        }
    }

    async fn execute(
        self: Arc<Self>,
        owner: &OwnerIdentity,
        scope_id: Option<ScopeId>,
        request: RawToolExecutionRequest,
    ) -> Result<RawToolOutcome, ExecutorError> {
        let now = tokio::time::Instant::now();
        if request.cancellation.is_cancelled() {
            return Ok(RawToolOutcome::Failure(
                ExternalCallFailure::Cancellation.outcome(),
            ));
        }
        let maximum = Duration::from_millis(EXTERNAL_TOOL_CALL_TIMEOUT_MS as u64);
        let deadline = now.checked_add(maximum).unwrap_or(now);
        if deadline <= now {
            return Ok(RawToolOutcome::Failure(
                ExternalCallFailure::Timeout.outcome(),
            ));
        }
        let call = match self.prepare_call(owner, scope_id, &request) {
            Ok(call) => call,
            Err(failure) => return Ok(RawToolOutcome::Failure(failure.outcome())),
        };
        let request_id = call.correlation.request_id.clone();
        let outgoing = call.outgoing.clone();
        let receiver = call.receiver;
        let cancellation = request.cancellation;
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                self.finish(&request_id, ExternalCallFailure::Cancellation);
                Ok(RawToolOutcome::Failure(ExternalCallFailure::Cancellation.outcome()))
            }
            result = owner.sender.send(outgoing) => {
                if result.is_err() {
                    self.finish(&request_id, ExternalCallFailure::OwnerDisconnected);
                    Ok(RawToolOutcome::Failure(ExternalCallFailure::OwnerDisconnected.outcome()))
                } else {
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => {
                            self.finish(&request_id, ExternalCallFailure::Cancellation);
                            Ok(RawToolOutcome::Failure(ExternalCallFailure::Cancellation.outcome()))
                        }
                        value = receiver => Ok(map_completion(value)),
                        () = tokio::time::sleep_until(deadline) => {
                            self.finish(&request_id, ExternalCallFailure::Timeout);
                            Ok(RawToolOutcome::Failure(ExternalCallFailure::Timeout.outcome()))
                        }
                    }
                }
            },
            () = tokio::time::sleep_until(deadline) => {
                self.finish(&request_id, ExternalCallFailure::Timeout);
                Ok(RawToolOutcome::Failure(ExternalCallFailure::Timeout.outcome()))
            }
        }
    }

    fn prepare_call(
        &self,
        owner: &OwnerIdentity,
        scope_id: Option<ScopeId>,
        request: &RawToolExecutionRequest,
    ) -> Result<PreparedCall, ExternalCallFailure> {
        let sequence = self.next_request_sequence.fetch_add(1, Ordering::Relaxed);
        if sequence == u64::MAX {
            return Err(ExternalCallFailure::InvalidResponse);
        }
        let instance = self.instance;
        let request_id = ExternalRequestId::new(format!("external-tool-{instance}-{sequence}"))?;
        let tool_call_id = ToolCallId::new(request.tool_call_id.as_str().to_owned())?;
        let correlation = CallCorrelation {
            runtime_id: self.runtime_id.clone(),
            request_id: request_id.clone(),
            tool_call_id,
            internal_name: request.definition.internal_name.clone(),
            model_name: request.model_name.clone(),
            scope_id,
        };
        let outgoing = outgoing_request(&correlation, request.input.clone());
        let (completion, receiver) = oneshot::channel();
        let mut state = self
            .state
            .lock()
            .map_err(|_| ExternalCallFailure::InvalidResponse)?;
        if !state.live.contains(&(owner.id.clone(), owner.generation)) {
            return Err(ExternalCallFailure::OwnerDisconnected);
        }
        if state.pending.len() >= EXTERNAL_PENDING_CALLS_PER_RUNTIME_MAX {
            return Err(ExternalCallFailure::InvalidResponse);
        }
        state.pending.insert(
            request_id.as_str().to_owned(),
            PendingCall {
                owner: (owner.id.clone(), owner.generation),
                correlation: correlation.clone(),
                completion,
            },
        );
        Ok(PreparedCall {
            correlation,
            outgoing,
            receiver,
        })
    }

    fn finish(&self, request_id: &ExternalRequestId, failure: ExternalCallFailure) {
        let pending = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.pending.remove(request_id.as_str()));
        settle(pending, Err(failure));
    }
}

struct PreparedCall {
    correlation: CallCorrelation,
    outgoing: ExternalCallRequest,
    receiver: oneshot::Receiver<Result<ResponseValue, ExternalCallFailure>>,
}
struct ExternalExecutor {
    core: Arc<ManagerCore>,
    owner: OwnerIdentity,
    scope_id: Option<ScopeId>,
}
impl ToolExecutor for ExternalExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        Box::pin(
            self.core
                .clone()
                .execute(&self.owner, self.scope_id.clone(), request),
        )
    }
}

fn validate_groups(
    connection: &Arc<ControllerConnection>,
    groups: &[ExternalToolGroup],
) -> Result<Vec<ValidatedMember>, ExternalRegistrationError> {
    let count = groups
        .iter()
        .try_fold(0usize, |total, group| total.checked_add(group.tools.len()))
        .ok_or(ExternalRegistrationError::TooManyTools)?;
    if count > EXTERNAL_TOOLS_PER_RUNTIME_MAX.value {
        return Err(ExternalRegistrationError::TooManyTools);
    }
    let mut members = Vec::new();
    members
        .try_reserve_exact(count)
        .map_err(|_| ExternalRegistrationError::Allocation)?;
    let mut scopes = BTreeSet::new();
    let mut unique = BTreeSet::new();
    for group in groups {
        let scope = group
            .scope_id
            .clone()
            .map(ScopeId::new)
            .transpose()
            .map_err(|_| ExternalRegistrationError::InvalidScope)?;
        let scope_key = scope.as_ref().map_or("", ScopeId::as_str).to_owned();
        if !scopes.insert(scope_key.clone()) {
            return Err(ExternalRegistrationError::InvalidScope);
        }
        for member in &group.tools {
            if !unique.insert((scope_key.clone(), member.name.clone())) {
                return Err(ExternalRegistrationError::DuplicateTool(
                    member.name.clone(),
                ));
            }
            members.push(validate_member(connection, scope.clone(), member)?);
        }
    }
    Ok(members)
}

fn validate_member(
    connection: &Arc<ControllerConnection>,
    scope_id: Option<ScopeId>,
    member: &ExternalToolMember,
) -> Result<ValidatedMember, ExternalRegistrationError> {
    if member.description.trim().is_empty() {
        return Err(ExternalRegistrationError::InvalidDescription);
    }
    if member
        .label
        .as_ref()
        .is_some_and(|label| label.len() > lotta_runtime::bounds::TOOL_DESCRIPTION_BYTES_MAX.value)
    {
        return Err(ExternalRegistrationError::InvalidDescription);
    }
    let internal = InternalToolName::new(member.name.clone())
        .map_err(|_| ExternalRegistrationError::InvalidName)?;
    let model = ModelFacingToolName::new(member.name.clone())
        .map_err(|_| ExternalRegistrationError::InvalidName)?;
    let schema_value = lotta_domain::BoundedJsonValue::new(member.parameters.clone())
        .map_err(|_| ExternalRegistrationError::InvalidSchema)?;
    let schema =
        ToolInputSchema::new(schema_value).map_err(|_| ExternalRegistrationError::InvalidSchema)?;
    jsonschema::validator_for(schema.as_value())
        .map_err(|_| ExternalRegistrationError::InvalidSchema)?;
    let description = ToolDescriptionAsset::new(member.description.clone())
        .map_err(|_| ExternalRegistrationError::InvalidDescription)?;
    let secret_fields = lotta_domain::BoundedVec::new(Vec::new())
        .map_err(|_| ExternalRegistrationError::InvalidDefinition)?;
    let secrets = SecretRedactionSpec::new(secret_fields, SecretRedactionPolicy::Redact)
        .map_err(|_| ExternalRegistrationError::InvalidDefinition)?;
    let timeout = ToolTimeout::new(Duration::from_millis(EXTERNAL_TOOL_CALL_TIMEOUT_MS as u64))
        .map_err(|_| ExternalRegistrationError::InvalidDefinition)?;
    let output = ToolOutputLimit::new(
        TOOL_RESULT_BYTES_MAX.value,
        TOOL_RESULT_MODEL_CHARS_MAX.value,
    )
    .map_err(|_| ExternalRegistrationError::InvalidDefinition)?;
    let definition = ToolDefinition::new(
        internal,
        model,
        schema,
        description,
        ToolExecutionOwner::Controller,
        ToolApprovalPolicy::Never,
        PermissionAction::new("execute".to_owned())
            .map_err(|_| ExternalRegistrationError::InvalidDefinition)?,
        timeout,
        output,
        secrets,
    );
    if definition.parallel_safety != ParallelSafety::Sequential {
        return Err(ExternalRegistrationError::InvalidDefinition);
    }
    let owner = OwnerIdentity {
        id: connection.id.clone(),
        generation: connection.generation,
        sender: connection.sender.clone(),
    };
    let executor = ExternalExecutor {
        core: connection
            .manager
            .upgrade()
            .ok_or(ExternalRegistrationError::OwnerDisconnected)?,
        owner: OwnerIdentity {
            id: owner.id.clone(),
            generation: owner.generation,
            sender: owner.sender.clone(),
        },
        scope_id: scope_id.clone(),
    };
    Ok(ValidatedMember {
        scope_id,
        registration: ToolRegistration {
            definition: Arc::new(definition),
            executor: Arc::new(executor),
        },
    })
}

fn select_registrations(
    members: &[ValidatedMember],
    scope: Option<&ScopeId>,
) -> Result<Vec<ToolRegistration>, ExternalRegistrationError> {
    let mut selected = Vec::new();
    let mut names = BTreeSet::new();
    for member in members {
        if member.scope_id.is_none() || member.scope_id.as_ref() == scope {
            let name = member
                .registration
                .definition
                .internal_name
                .as_str()
                .to_owned();
            if !names.insert(name.clone()) {
                return Err(ExternalRegistrationError::DuplicateTool(name));
            }
            selected.push(member.registration.clone());
        }
    }
    Ok(selected)
}

fn map_registry_error(error: crate::RegistryError) -> ExternalRegistrationError {
    match error {
        crate::RegistryError::DuplicateInternal(name)
        | crate::RegistryError::DuplicateModel(name) => {
            ExternalRegistrationError::NativeCollision(name)
        }
        crate::RegistryError::RevisionMismatch => ExternalRegistrationError::RevisionMismatch,
        crate::RegistryError::RevisionExhausted => ExternalRegistrationError::Allocation,
        other => ExternalRegistrationError::Registry(other),
    }
}

fn outgoing_request(
    correlation: &CallCorrelation,
    arguments: lotta_runtime::ports::ValidatedToolInput,
) -> ExternalCallRequest {
    ExternalCallRequest {
        runtime_id: correlation.runtime_id.clone(),
        request_id: correlation.request_id.clone(),
        tool_call_id: correlation.tool_call_id.clone(),
        internal_name: correlation.internal_name.clone(),
        model_name: correlation.model_name.clone(),
        arguments,
        scope_id: correlation.scope_id.clone(),
    }
}
fn correlates(correlation: &CallCorrelation, response: &ExternalCallResponse) -> bool {
    correlation.runtime_id == response.runtime_id
        && correlation.request_id == response.request_id
        && correlation.tool_call_id == response.tool_call_id
        && correlation.internal_name == response.internal_name
        && correlation.model_name == response.model_name
        && correlation.scope_id == response.scope_id
}
fn settle(pending: Option<PendingCall>, value: Result<ResponseValue, ExternalCallFailure>) {
    if let Some(pending) = pending {
        let _ = pending.completion.send(value);
    }
}
fn map_completion(
    value: Result<Result<ResponseValue, ExternalCallFailure>, oneshot::error::RecvError>,
) -> RawToolOutcome {
    match value {
        Ok(Ok(ResponseValue::Result(value))) => RawToolOutcome::Success(response_text(&value)),
        Ok(Ok(ResponseValue::Error(error))) => RawToolOutcome::Failure(external_error(error)),
        Ok(Err(failure)) => RawToolOutcome::Failure(failure.outcome()),
        Err(_) => RawToolOutcome::Failure(ExternalCallFailure::OwnerDisconnected.outcome()),
    }
}
fn response_text(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToOwned::to_owned)
}
fn external_error(error: String) -> ToolOutcome {
    let message = ToolOutcomeMessage::new(error).unwrap_or_else(|_| {
        ToolOutcomeMessage::new("external tool error".to_owned()).unwrap_or_else(|_| unreachable!())
    });
    ToolOutcome::ToolDefinedError {
        code: ToolOutcomeCode::new("external_error".to_owned()).unwrap_or_else(|_| unreachable!()),
        message,
    }
}
