use std::{future::Future, pin::Pin, sync::Arc};

use lotta_domain::{BoundedJsonValue, BoundedVec, InputDisposition, NonEmptyString, RuntimeScope};
use lotta_runtime::{CompactionMode, ports::ProviderRequest, turn::CompactionProgress};
use tokio_util::sync::CancellationToken;

use super::{
    command::{
        AbortMessageCommand, ChangeDeviceStateCommand, InputCommand, RuntimeStartCommand,
        SyncCommand,
    },
    event::RuntimeEvent,
};

/// Maximum events returned by one Runtime service phase.
pub const WS_RUNTIME_ROUTE_EVENTS_MAX: usize = 256;
/// Bounded Runtime service event batch.
pub type RuntimeEventBatch = BoundedVec<RuntimeEvent, WS_RUNTIME_ROUTE_EVENTS_MAX>;
/// Object-safe future returned by Runtime command service methods.
pub type ServiceFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, crate::error::AppServerError>> + Send + 'a>>;

/// Successful runtime resolution and initial snapshots.
pub struct RuntimeStartOutcome {
    /// Resolved runtime scope.
    pub runtime: RuntimeScope,
    /// Whether an agent was created.
    pub created_agent: bool,
    /// Whether a conversation was created.
    pub created_conversation: bool,
    /// Optional bounded agent response body.
    pub agent: Option<BoundedJsonValue>,
    /// Optional bounded conversation response body.
    pub conversation: Option<BoundedJsonValue>,
    /// Initial frames emitted after the response.
    pub broadcasts: RuntimeEventBatch,
}

/// Admission result with all execution effects deferred until after acknowledgement.
pub struct InputAdmission {
    /// Admission disposition.
    pub disposition: InputDisposition,
    /// Optional rejection detail.
    pub error: Option<NonEmptyString>,
    /// Opaque bounded continuation state.
    pub continuation: Option<BoundedJsonValue>,
    /// Immediate events emitted after acknowledgement and before continuation.
    pub after_ack: RuntimeEventBatch,
}

/// Sync action result.
pub struct SyncOutcome {
    /// Authoritative replay events.
    pub broadcasts: RuntimeEventBatch,
}

/// Abort action result.
pub struct AbortOutcome {
    /// Whether active work was interrupted.
    pub aborted: bool,
}

/// Device-state action result.
pub struct DeviceStateOutcome {
    /// State frames caused by the change.
    pub broadcasts: RuntimeEventBatch,
}

/// Synchronous sink used by continuation work to stamp and fan out immediately.
pub trait RuntimeEventSink: Send + Sync {
    /// Emits one event to current subscribers without awaiting router ownership.
    ///
    /// # Errors
    /// Returns a visible routing or outbound-capacity failure.
    fn emit(
        &self,
        scope: &RuntimeScope,
        event: RuntimeEvent,
    ) -> Result<(), crate::error::AppServerError>;
}

/// Object-safe canonical turn submission port.
pub trait TurnController: Send + Sync {
    /// Returns whether this command is a control continuation rather than a new turn.
    ///
    /// Approval responses and teleport continuations both resume the captured
    /// active lease through [`RuntimeCommandService::continue_input`] instead
    /// of starting a second turn; every other payload kind is a new turn.
    fn is_control_continuation(&self, deferred: &super::router::DeferredInput) -> bool {
        matches!(
            deferred
                .continuation
                .as_ref()
                .and_then(|value| value.as_value().get("kind"))
                .and_then(serde_json::Value::as_str),
            Some("approval_response" | "teleport_continue")
        )
    }
    /// Submits one admitted canonical user message to the production turn pipeline.
    fn submit_turn(
        &self,
        command: InputCommand,
        deferred: super::router::DeferredInput,
        cancellation: CancellationToken,
        sink: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()>;
}

/// Injectable application seam for the five Runtime-group commands.
pub trait RuntimeCommandService: Send + Sync {
    /// Resolves or creates a runtime.
    fn runtime_start(&self, command: RuntimeStartCommand)
    -> ServiceFuture<'_, RuntimeStartOutcome>;
    /// Performs bounded input admission only.
    fn admit_input(&self, command: InputCommand) -> ServiceFuture<'_, InputAdmission>;
    /// Continues admitted input after its acknowledgement was dispatched.
    fn continue_input(
        &self,
        scope: RuntimeScope,
        continuation: Option<BoundedJsonValue>,
        sink: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()>;
    /// Manually compacts an active runtime through its canonical production coordinator.
    fn compact(
        &self,
        scope: RuntimeScope,
        mode: CompactionMode,
        request_id: NonEmptyString,
        request: ProviderRequest,
    ) -> ServiceFuture<'_, CompactionProgress>;
    /// Replays runtime state.
    fn sync(&self, command: SyncCommand) -> ServiceFuture<'_, SyncOutcome>;
    /// Aborts runtime work.
    fn abort_message(&self, command: AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome>;
    /// Applies a device-state change.
    fn change_device_state(
        &self,
        command: ChangeDeviceStateCommand,
    ) -> ServiceFuture<'_, DeviceStateOutcome>;
}

/// Documented inert service used by the compatibility listener entry point.
pub struct UnsupportedRuntimeCommandService;

impl TurnController for UnsupportedRuntimeCommandService {
    fn submit_turn(
        &self,
        _: InputCommand,
        _: super::router::DeferredInput,
        _: CancellationToken,
        _: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        unsupported()
    }
}

/// Adapter preserving the canonical service continuation for compatibility listeners.
pub struct ServiceBackedTurnController {
    service: Arc<dyn RuntimeCommandService>,
}

impl ServiceBackedTurnController {
    /// Creates an adapter over the listener's runtime service.
    #[must_use]
    pub fn new(service: Arc<dyn RuntimeCommandService>) -> Self {
        Self { service }
    }
}

impl TurnController for ServiceBackedTurnController {
    fn submit_turn(
        &self,
        _: InputCommand,
        deferred: super::router::DeferredInput,
        _: CancellationToken,
        sink: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        self.service
            .continue_input(deferred.scope, deferred.continuation, sink)
    }
}

fn unsupported<'a, T>() -> ServiceFuture<'a, T> {
    Box::pin(async { Err(crate::error::AppServerError::Unavailable) })
}

impl RuntimeCommandService for UnsupportedRuntimeCommandService {
    fn runtime_start(&self, _: RuntimeStartCommand) -> ServiceFuture<'_, RuntimeStartOutcome> {
        unsupported()
    }
    fn admit_input(&self, _: InputCommand) -> ServiceFuture<'_, InputAdmission> {
        unsupported()
    }
    fn continue_input(
        &self,
        _: RuntimeScope,
        _: Option<BoundedJsonValue>,
        _: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        unsupported()
    }
    fn compact(
        &self,
        _: RuntimeScope,
        _: CompactionMode,
        _: NonEmptyString,
        _: ProviderRequest,
    ) -> ServiceFuture<'_, CompactionProgress> {
        unsupported()
    }
    fn sync(&self, _: SyncCommand) -> ServiceFuture<'_, SyncOutcome> {
        unsupported()
    }
    fn abort_message(&self, _: AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome> {
        unsupported()
    }
    fn change_device_state(
        &self,
        _: ChangeDeviceStateCommand,
    ) -> ServiceFuture<'_, DeviceStateOutcome> {
        unsupported()
    }
}
