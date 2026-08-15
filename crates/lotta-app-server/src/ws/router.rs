use std::sync::{Arc, Mutex};

use lotta_domain::{BoundedJsonValue, BoundedVec, InputDisposition, RuntimeScope};
use serde::Serialize;

use super::{
    command::RuntimeCommand,
    connection::{ConnectionId, EventDelivery, RuntimeConnections},
    event::RuntimeEvent,
    service::{RuntimeCommandService, RuntimeEventBatch, RuntimeEventSink},
};

/// Maximum response frames produced by one Runtime command.
pub const WS_RUNTIME_ROUTE_RESPONSES_MAX: usize = 1;
/// Maximum stamped deliveries produced by one synchronous route application.
pub const WS_RUNTIME_ROUTE_DELIVERIES_MAX: usize =
    super::service::WS_RUNTIME_ROUTE_EVENTS_MAX * lotta_domain::bounds::CONNECTIONS_MAX.value;
/// Bounded connection-response batch.
pub type ConnectionResponseBatch = BoundedVec<ConnectionResponse, WS_RUNTIME_ROUTE_RESPONSES_MAX>;
/// Bounded stamped-delivery batch.
pub type EventDeliveryBatch = BoundedVec<EventDelivery, WS_RUNTIME_ROUTE_DELIVERIES_MAX>;

/// Connection-specific responses intentionally lacking runtime lifecycle stamps.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum ConnectionResponse {
    /// Runtime resolution result.
    #[serde(rename = "runtime_start_response")]
    RuntimeStart {
        /// Correlation identifier.
        request_id: String,
        /// Resolution success.
        success: bool,
        /// Resolved runtime, or null on failure.
        runtime: Option<RuntimeScope>,
        /// Resolved agent body, or null.
        agent: Option<BoundedJsonValue>,
        /// Resolved conversation body, or null.
        conversation: Option<BoundedJsonValue>,
        /// Creation flags.
        created: CreatedFlags,
        /// Optional scrubbed failure detail.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Input admission acknowledgement.
    #[serde(rename = "input_accepted")]
    InputAccepted {
        /// Correlation identifier.
        request_id: String,
        /// Runtime receiving input.
        runtime: RuntimeScope,
        /// Admission result.
        accepted: bool,
        /// Started/queued disposition, omitted when rejected.
        #[serde(skip_serializing_if = "Option::is_none")]
        disposition: Option<AcceptedInputDisposition>,
        /// Optional rejection detail.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Sync result.
    #[serde(rename = "sync_response")]
    SyncResponse {
        /// Correlation identifier.
        request_id: String,
        /// Runtime synchronized.
        runtime: RuntimeScope,
        /// Sync success.
        success: bool,
        /// Optional scrubbed failure detail.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Abort result.
    #[serde(rename = "abort_message_response")]
    AbortMessage {
        /// Correlation identifier.
        request_id: String,
        /// Runtime targeted.
        runtime: RuntimeScope,
        /// Whether work was interrupted.
        aborted: bool,
        /// Abort success.
        success: bool,
        /// Optional scrubbed failure detail.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Representative unstamped management response used by envelope tests.
    #[serde(rename = "app_server_info_response")]
    Management {
        /// Correlation identifier.
        request_id: String,
        /// Operation success.
        success: bool,
    },
}

/// Wire-safe accepted input disposition.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptedInputDisposition {
    /// Input started immediately.
    Started,
    /// Input was queued.
    Queued,
}

/// Runtime-start resource creation flags.
#[derive(Debug, Serialize)]
pub struct CreatedFlags {
    /// Whether an agent was created.
    pub agent: bool,
    /// Whether a conversation was created.
    pub conversation: bool,
}

/// One bounded logical event route and its actual stamped deliveries.
pub struct RoutedEventBatch {
    /// Runtime scope supplied to routing, retained even with no subscribers.
    pub scope: RuntimeScope,
    /// Actual logical event before stamping.
    pub event: RuntimeEvent,
    /// Stamped deliveries in stable target order, possibly empty.
    pub deliveries: EventDeliveryBatch,
}

/// Maximum logical event batches produced by one route.
pub const WS_RUNTIME_ROUTED_EVENT_BATCHES_MAX: usize = super::service::WS_RUNTIME_ROUTE_EVENTS_MAX;
/// Bounded logical event batches produced by one route.
pub type RoutedEventBatches = BoundedVec<RoutedEventBatch, WS_RUNTIME_ROUTED_EVENT_BATCHES_MAX>;

/// One bounded ordered routing result.
pub struct RouteOutput {
    /// Targeted connection responses.
    pub responses: ConnectionResponseBatch,
    /// Logical events paired with their stamped deliveries.
    pub event_batches: RoutedEventBatches,
}

/// Deferred phase-two input work returned after admission application.
pub struct DeferredInput {
    /// Runtime scope for continuation.
    pub scope: RuntimeScope,
    /// Opaque bounded continuation state.
    pub continuation: Option<BoundedJsonValue>,
}

/// Admission application output before phase-two continuation.
pub struct RouteAdmission {
    /// Physical frames to dispatch before continuation.
    pub output: RouteOutput,
    /// Deferred continuation descriptor.
    pub deferred: DeferredInput,
}

/// Synchronous owner-local Runtime routing state.
pub struct RuntimeRouter {
    /// Per-connection subscription and sequence state.
    pub connections: RuntimeConnections,
    clock: Arc<dyn lotta_domain::Clock + Send + Sync>,
    ids: Arc<dyn super::envelope::EventIdGenerator>,
}

impl RuntimeRouter {
    /// Creates synchronous routing state.
    #[must_use]
    pub fn new(
        clock: Arc<dyn lotta_domain::Clock + Send + Sync>,
        ids: Arc<dyn super::envelope::EventIdGenerator>,
    ) -> Self {
        Self {
            connections: RuntimeConnections::default(),
            clock,
            ids,
        }
    }

    /// Applies a successful runtime start response and initial frames.
    ///
    /// # Errors
    /// Returns a bounded routing or subscription failure.
    pub fn apply_runtime_start(
        &mut self,
        connection: ConnectionId,
        request_id: String,
        outcome: super::service::RuntimeStartOutcome,
    ) -> Result<RouteOutput, crate::error::AppServerError> {
        self.connections
            .subscribe(connection, outcome.runtime.clone())?;
        let response = ConnectionResponse::RuntimeStart {
            request_id,
            success: true,
            runtime: Some(outcome.runtime.clone()),
            agent: outcome.agent,
            conversation: outcome.conversation,
            created: CreatedFlags {
                agent: outcome.created_agent,
                conversation: outcome.created_conversation,
            },
            error: None,
        };
        self.response_then_broadcast(Some(response), &outcome.runtime, &outcome.broadcasts)
    }

    /// Applies an input admission; acknowledgement precedes every caused delivery.
    ///
    /// # Errors
    /// Returns a bounded routing failure.
    pub fn apply_input(
        &mut self,
        command: &super::command::InputCommand,
        admission: super::service::InputAdmission,
    ) -> Result<RouteAdmission, crate::error::AppServerError> {
        let response = command.request_id.as_ref().map(|request_id| {
            let rejected = admission.disposition == InputDisposition::Rejected;
            ConnectionResponse::InputAccepted {
                request_id: request_id.as_str().to_owned(),
                runtime: command.runtime.clone(),
                accepted: !rejected,
                disposition: accepted_disposition(admission.disposition),
                error: admission
                    .error
                    .as_ref()
                    .map(|value| value.as_str().to_owned()),
            }
        });
        let output =
            self.response_then_broadcast(response, &command.runtime, &admission.after_ack)?;
        Ok(RouteAdmission {
            output,
            deferred: DeferredInput {
                scope: command.runtime.clone(),
                continuation: admission.continuation,
            },
        })
    }

    /// Applies a successful sync replay and optional response.
    ///
    /// # Errors
    /// Returns a bounded routing failure.
    pub fn apply_sync(
        &mut self,
        command: &super::command::SyncCommand,
        outcome: &super::service::SyncOutcome,
    ) -> Result<RouteOutput, crate::error::AppServerError> {
        let response =
            command
                .request_id
                .as_ref()
                .map(|request_id| ConnectionResponse::SyncResponse {
                    request_id: request_id.as_str().to_owned(),
                    runtime: command.runtime.clone(),
                    success: true,
                    error: None,
                });
        self.response_then_broadcast(response, &command.runtime, &outcome.broadcasts)
    }

    /// Applies a successful abort response.
    ///
    /// # Errors
    /// Returns a bounded response allocation failure.
    pub fn apply_abort(
        &self,
        command: &super::command::AbortMessageCommand,
        outcome: &super::service::AbortOutcome,
    ) -> Result<RouteOutput, crate::error::AppServerError> {
        let response =
            command
                .request_id
                .as_ref()
                .map(|request_id| ConnectionResponse::AbortMessage {
                    request_id: request_id.as_str().to_owned(),
                    runtime: command.runtime.clone(),
                    aborted: outcome.aborted,
                    success: true,
                    error: None,
                });
        empty_output(response)
    }

    /// Applies device-state broadcasts with no response.
    ///
    /// # Errors
    /// Returns a bounded routing failure.
    pub fn apply_device(
        &mut self,
        command: &super::command::ChangeDeviceStateCommand,
        outcome: &super::service::DeviceStateOutcome,
    ) -> Result<RouteOutput, crate::error::AppServerError> {
        self.response_then_broadcast(None, &command.runtime, &outcome.broadcasts)
    }

    /// Broadcasts one event synchronously in stable ordinal order.
    ///
    /// # Errors
    /// Returns a stamping or bounded delivery failure.
    pub fn broadcast(
        &mut self,
        scope: &RuntimeScope,
        event: &RuntimeEvent,
    ) -> Result<EventDeliveryBatch, crate::error::AppServerError> {
        bounded_deliveries(self.connections.broadcast(
            scope,
            event,
            self.clock.as_ref(),
            self.ids.as_ref(),
        )?)
    }

    fn response_then_broadcast(
        &mut self,
        response: Option<ConnectionResponse>,
        runtime: &RuntimeScope,
        events: &RuntimeEventBatch,
    ) -> Result<RouteOutput, crate::error::AppServerError> {
        let mut batches = Vec::new();
        batches
            .try_reserve_exact(events.len())
            .map_err(|_| crate::error::AppServerError::Unavailable)?;
        for event in events.as_slice() {
            let deliveries = self.connections.broadcast(
                runtime,
                event,
                self.clock.as_ref(),
                self.ids.as_ref(),
            )?;
            batches.push(RoutedEventBatch {
                scope: runtime.clone(),
                event: event.clone(),
                deliveries: bounded_deliveries(deliveries)?,
            });
        }
        Ok(RouteOutput {
            responses: bounded_responses(response)?,
            event_batches: BoundedVec::new(batches)
                .map_err(|_| crate::error::AppServerError::PayloadTooLarge)?,
        })
    }
}

fn accepted_disposition(value: InputDisposition) -> Option<AcceptedInputDisposition> {
    match value {
        InputDisposition::Started => Some(AcceptedInputDisposition::Started),
        InputDisposition::Queued => Some(AcceptedInputDisposition::Queued),
        InputDisposition::Rejected => None,
    }
}

fn bounded_responses(
    response: Option<ConnectionResponse>,
) -> Result<ConnectionResponseBatch, crate::error::AppServerError> {
    BoundedVec::new(response.into_iter().collect())
        .map_err(|_| crate::error::AppServerError::PayloadTooLarge)
}

fn bounded_deliveries(
    values: Vec<EventDelivery>,
) -> Result<EventDeliveryBatch, crate::error::AppServerError> {
    BoundedVec::new(values).map_err(|_| crate::error::AppServerError::PayloadTooLarge)
}

fn empty_output(
    response: Option<ConnectionResponse>,
) -> Result<RouteOutput, crate::error::AppServerError> {
    Ok(RouteOutput {
        responses: bounded_responses(response)?,
        event_batches: BoundedVec::new(Vec::new())
            .map_err(|_| crate::error::AppServerError::PayloadTooLarge)?,
    })
}

/// Executes service work outside the router mutex, then applies it synchronously.
///
/// # Errors
/// Returns typed service, validation, locking, subscription, or bounded routing failures.
pub async fn route_command(
    router: Arc<Mutex<RuntimeRouter>>,
    service: Arc<dyn RuntimeCommandService>,
    connection: ConnectionId,
    command: RuntimeCommand,
) -> Result<(RouteOutput, Option<DeferredInput>), crate::error::AppServerError> {
    match command {
        RuntimeCommand::RuntimeStart(command) => {
            command
                .validate_choices()
                .and_then(|()| command.validate_structure())
                .map_err(|_| crate::error::AppServerError::Malformed)?;
            let request_id = command.request_id.as_str().to_owned();
            let outcome = service.runtime_start(*command).await?;
            let output =
                lock_router(&router)?.apply_runtime_start(connection, request_id, outcome)?;
            Ok((output, None))
        }
        RuntimeCommand::Input(command) => {
            let admission = service.admit_input(command.clone()).await?;
            let admission_route = lock_router(&router)?.apply_input(&command, admission)?;
            Ok((admission_route.output, Some(admission_route.deferred)))
        }
        RuntimeCommand::Sync(command) => {
            let outcome = service.sync(command.clone()).await?;
            let output = lock_router(&router)?.apply_sync(&command, &outcome)?;
            Ok((output, None))
        }
        RuntimeCommand::AbortMessage(command) => {
            let outcome = service.abort_message(command.clone()).await?;
            let output = lock_router(&router)?.apply_abort(&command, &outcome)?;
            Ok((output, None))
        }
        RuntimeCommand::ChangeDeviceState(command) => {
            let outcome = service.change_device_state(command.clone()).await?;
            let output = lock_router(&router)?.apply_device(&command, &outcome)?;
            Ok((output, None))
        }
    }
}

/// Locks owner-local routing state without poisoning leakage.
///
/// # Errors
/// Returns an internal error after a prior panic poisoned the mutex.
pub fn lock_router(
    router: &Mutex<RuntimeRouter>,
) -> Result<std::sync::MutexGuard<'_, RuntimeRouter>, crate::error::AppServerError> {
    router
        .lock()
        .map_err(|_| crate::error::AppServerError::Internal)
}

/// Mutex-backed sink that stamps events under only a brief synchronous lock.
pub struct RouterEventSink {
    router: Arc<Mutex<RuntimeRouter>>,
    dispatch: Arc<
        dyn Fn(
                RuntimeScope,
                RuntimeEvent,
                EventDeliveryBatch,
            ) -> Result<(), crate::error::AppServerError>
            + Send
            + Sync,
    >,
}

impl RouterEventSink {
    /// Creates a sink using the same delivery dispatcher as command routing.
    #[must_use]
    pub fn new(
        router: Arc<Mutex<RuntimeRouter>>,
        dispatch: Arc<
            dyn Fn(
                    RuntimeScope,
                    RuntimeEvent,
                    EventDeliveryBatch,
                ) -> Result<(), crate::error::AppServerError>
                + Send
                + Sync,
        >,
    ) -> Self {
        Self { router, dispatch }
    }
}

impl RuntimeEventSink for RouterEventSink {
    fn emit(
        &self,
        scope: &RuntimeScope,
        event: RuntimeEvent,
    ) -> Result<(), crate::error::AppServerError> {
        let deliveries = lock_router(&self.router)?.broadcast(scope, &event)?;
        (self.dispatch)(scope.clone(), event, deliveries)
    }
}
