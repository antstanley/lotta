//! WebSocket teleport command group.
//!
//! Decodes the pinned `teleport_probe`, `teleport_request`, and
//! `teleport_failed` commands and emits the two pinned messages
//! `teleport_probe_response` and `teleport_ready` through
//! [`TeleportBridge`](crate::ws::teleport::TeleportBridge). One pending
//! teleport is retained per runtime scope: a repeat request with the same
//! identifier replays the recorded ready outcome, a different identifier while
//! one is pending answers the pinned conflict failure, and `teleport_failed`
//! removes the entry so no dangling teleport outlives a failure.
//!
//! The continuation input `input.kind = teleport_continue` never becomes a new
//! admission or a second turn:
//! [`TeleportBridge::continue_input`](crate::ws::teleport::TeleportBridge::continue_input)
//! routes it as
//! [`AdmissionRoute::Continuation`](lotta_runtime::AdmissionRoute::Continuation)
//! against the caller-captured active lease, so the queue length, turn lease
//! generation, and lifecycle state are all unchanged on success, and a
//! superseded lease generation is rejected without any emission.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use lotta_domain::{
    BoundedJsonValue, EntityExtras, InputDisposition, NonEmptyString, PermissionMode,
    QueueDropReason, QueueItem, QueueItemKind, QueueItemSource, RuntimeScope, Timestamp, TurnLease,
};
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::{
    AdmissionOutcome, AdmissionRequest, AdmissionRoute, ListenerRuntime, RuntimeError,
    RuntimeHandle,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::AppServerError, errors::ProtocolErrorEnvelope, framing::DecodedFrame,
    ws::connection::ConnectionId,
};

/// Pinned failure text returned while another teleport owns the conversation.
const CONFLICT_ERROR: &str = "Conversation already has a teleport pending";

/// The three concrete teleport group commands.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TeleportCommand {
    /// Capability probe from a candidate destination device.
    #[serde(rename = "teleport_probe")]
    Probe(TeleportProbeCommand),
    /// Teleport request addressed at one target device connection.
    #[serde(rename = "teleport_request")]
    Request(Box<TeleportRequestCommand>),
    /// Teleport abort reported by the controller.
    #[serde(rename = "teleport_failed")]
    Failed(TeleportFailedCommand),
}

/// Pinned `teleport_probe` payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeleportProbeCommand {
    /// Response correlation identifier.
    pub request_id: NonEmptyString,
    /// Runtime scope probed by the candidate destination.
    pub runtime: RuntimeScope,
}

/// Pinned `teleport_request` payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeleportRequestCommand {
    /// Response correlation identifier.
    pub request_id: NonEmptyString,
    /// Caller-chosen teleport identity reused by continuations and failures.
    pub teleport_id: NonEmptyString,
    /// Runtime scope being teleported.
    pub runtime: RuntimeScope,
    /// Selected destination device connection.
    pub target: TeleportTarget,
}

/// Pinned teleport target selection; routing stays a client concern.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeleportTarget {
    /// Destination connection identifier.
    pub connection_id: String,
    /// Destination device identifier.
    pub device_id: String,
    /// Human-readable destination connection name.
    pub connection_name: String,
}

/// Pinned `teleport_failed` payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeleportFailedCommand {
    /// Teleport identity whose pending state must be dropped.
    pub teleport_id: NonEmptyString,
    /// Runtime scope the failure belongs to.
    pub runtime: RuntimeScope,
    /// Scrubbed controller-reported failure detail.
    pub error: String,
}

/// Pinned `input.kind = teleport_continue` payload carried inside an `input`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TeleportContinuePayload {
    /// Exact continuation discriminant.
    pub kind: TeleportContinueKind,
    /// Teleport identity this continuation resumes.
    pub teleport_id: NonEmptyString,
    /// Originating device attribution.
    pub source: TeleportSource,
    /// Optional approval batch replayed into the resumed turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<TeleportContinuation>,
}

/// Exact `teleport_continue` discriminant marker type.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TeleportContinueKind {
    /// Exact pinned discriminant string.
    #[serde(rename = "teleport_continue")]
    TeleportContinue,
}

/// Pinned teleport source attribution record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TeleportSource {
    /// Originating device identifier.
    pub device_id: String,
    /// Originating connection name.
    pub connection_name: String,
}

/// Pinned approval batch attached to a teleport continuation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TeleportContinuation {
    /// Approval payloads forwarded to the resumed turn.
    pub approvals: Vec<Value>,
}

/// The two outbound teleport group messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TeleportMessage {
    /// Probe answer declaring baseline teleport capabilities.
    #[serde(rename = "teleport_probe_response")]
    ProbeResponse(TeleportProbeResponseMessage),
    /// Ready or failed outcome for one teleport identity.
    #[serde(rename = "teleport_ready")]
    Ready(TeleportReadyMessage),
}

/// Pinned `teleport_probe_response` message; all three flags are literal true.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeleportProbeResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// Runtime scope that was probed.
    pub runtime: RuntimeScope,
    /// Whether teleport is supported (always true when answered).
    pub supported: bool,
    /// Whether accepted inputs drain before handoff (always true).
    pub drains_accepted_inputs: bool,
    /// Whether continuations are idempotent (always true).
    pub idempotent_continuation: bool,
}

/// Pinned `teleport_ready` message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeleportReadyMessage {
    /// Teleport identity this outcome resolves.
    pub teleport_id: String,
    /// Runtime scope that was teleported.
    pub runtime: RuntimeScope,
    /// Whether the teleport succeeded.
    pub success: bool,
    /// Whether an active provider turn was handed over.
    pub active_turn: bool,
    /// Current permission mode of the destination scope, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PermissionMode>,
    /// Approval batch replayed with the resumed turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<TeleportContinuation>,
    /// Scrubbed failure detail, omitted on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known teleport commands.
pub fn decode(frame: &DecodedFrame) -> Result<Option<TeleportCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    match tag {
        Tag::TeleportProbe => serde_json::from_value::<TeleportProbeCommand>(frame.value.clone())
            .map(TeleportCommand::Probe)
            .map(Some),
        Tag::TeleportRequest => {
            serde_json::from_value::<TeleportRequestCommand>(frame.value.clone())
                .map(Box::new)
                .map(TeleportCommand::Request)
                .map(Some)
        }
        Tag::TeleportFailed => serde_json::from_value::<TeleportFailedCommand>(frame.value.clone())
            .map(TeleportCommand::Failed)
            .map(Some),
        _ => Ok(None),
    }
    .map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "teleport_command_invalid",
        "invalid teleport command",
        frame.request_id.clone(),
    )
}

/// Push callback delivering one outbound message to one connection.
pub type TeleportForwarder =
    Arc<dyn Fn(ConnectionId, TeleportMessage) -> Result<(), AppServerError> + Send + Sync>;

struct PendingTeleport {
    teleport_id: String,
    ready: bool,
    success: bool,
    active_turn: bool,
    error: Option<String>,
}

/// Per-scope pending teleports; bounded by resident runtime scopes.
#[derive(Default)]
struct BridgeState {
    pending: HashMap<RuntimeScope, PendingTeleport>,
}

/// Applies wire teleport commands to per-scope pending state and emits the two
/// outbound teleport messages through [`TeleportForwarder`].
pub struct TeleportBridge {
    state: Mutex<BridgeState>,
    forward: TeleportForwarder,
}

impl TeleportBridge {
    /// Creates a bridge pushing ready outcomes through `forward`.
    #[must_use]
    pub fn new(forward: TeleportForwarder) -> Self {
        Self {
            state: Mutex::new(BridgeState::default()),
            forward,
        }
    }

    /// Answers a capability probe with the pinned affirmative response.
    pub fn probe(&self, connection: ConnectionId, command: &TeleportProbeCommand) {
        let _ = (self.forward)(
            connection,
            TeleportMessage::ProbeResponse(TeleportProbeResponseMessage {
                request_id: command.request_id.as_str().to_owned(),
                runtime: command.runtime.clone(),
                supported: true,
                drains_accepted_inputs: true,
                idempotent_continuation: true,
            }),
        );
    }

    /// Registers a teleport request and emits its immediate outcome.
    ///
    /// An idle scope answers ready immediately; an occupied scope stays
    /// pending until the owning turn pipeline claims it at a boundary. A
    /// repeated identical identifier replays the recorded outcome, and a
    /// different identifier while one is pending answers the pinned conflict
    /// failure without displacing the pending entry.
    pub fn request(
        &self,
        connection: ConnectionId,
        command: &TeleportRequestCommand,
        processing: bool,
    ) {
        let teleport_id = command.teleport_id.as_str().to_owned();
        let mut state = lock(&self.state);
        let outcome = match state.pending.get(&command.runtime) {
            Some(pending) if pending.teleport_id == teleport_id => {
                if pending.ready {
                    Some(ready_message(command.runtime.clone(), pending))
                } else {
                    None
                }
            }
            Some(_) => Some(failed_ready(
                teleport_id,
                command.runtime.clone(),
                CONFLICT_ERROR,
            )),
            None => {
                let pending = PendingTeleport {
                    teleport_id,
                    ready: !processing,
                    success: !processing,
                    active_turn: false,
                    error: None,
                };
                let message = if pending.ready {
                    Some(ready_message(command.runtime.clone(), &pending))
                } else {
                    None
                };
                state.pending.insert(command.runtime.clone(), pending);
                message
            }
        };
        drop(state);
        if let Some(message) = outcome {
            let _ = (self.forward)(connection, message);
        }
    }

    /// Drops the pending teleport named by a failure report.
    ///
    /// Returns whether a matching pending teleport was removed; unknown or
    /// mismatched identities change nothing.
    pub fn failed(&self, command: &TeleportFailedCommand) -> bool {
        let mut state = lock(&self.state);
        let matches = state
            .pending
            .get(&command.runtime)
            .is_some_and(|pending| pending.teleport_id == command.teleport_id.as_str());
        if matches {
            state.pending.remove(&command.runtime);
        }
        matches
    }

    /// Routes a teleport continuation onto the captured active lease.
    ///
    /// This is the canonical continuation branch of the admission chain, not a
    /// new admission: no queue item is retained and no second turn begins, so
    /// the responding turn keeps its exact lease generation. A superseded
    /// lease generation returns [`TeleportContinuationOutcome::StaleLease`]
    /// with zero emissions and zero lifecycle or queue mutation.
    ///
    /// # Errors
    /// Returns the admission chain error for stale registry handles, and an
    /// internal data error when the chain classifies the route unexpectedly.
    pub fn continue_input(
        &self,
        runtime: &mut ListenerRuntime,
        handle: &RuntimeHandle,
        lease: TurnLease,
        payload: &TeleportContinuePayload,
        enqueued_at: Timestamp,
    ) -> Result<TeleportContinuationOutcome, RuntimeError> {
        let item = continuation_item(payload, enqueued_at)?;
        let request = AdmissionRequest {
            item,
            route: AdmissionRoute::Continuation(lease),
        };
        match runtime.admit(handle, request)? {
            AdmissionOutcome::Continue(_) => Ok(TeleportContinuationOutcome::Continued),
            AdmissionOutcome::Rejected {
                reason: QueueDropReason::StaleGeneration,
                ..
            } => Ok(TeleportContinuationOutcome::StaleLease),
            AdmissionOutcome::Duplicate(disposition) => {
                Ok(TeleportContinuationOutcome::Duplicate(disposition))
            }
            other => Err(RuntimeError::InvalidData {
                context: format!("unexpected teleport continuation classification: {other:?}"),
            }),
        }
    }

    /// Returns how many scopes currently hold a pending teleport.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        lock(&self.state).pending.len()
    }
}

/// Outcome of one teleport continuation admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeleportContinuationOutcome {
    /// Admitted on the current lease; queue untouched, same turn continues.
    Continued,
    /// Lease generation was superseded; nothing changed and nothing emitted.
    StaleLease,
    /// Prior disposition replayed without executing twice.
    Duplicate(InputDisposition),
}

#[cfg(test)]
pub(crate) fn inert_forwarder() -> TeleportForwarder {
    Arc::new(|_, _| Ok(()))
}

fn lock(state: &Mutex<BridgeState>) -> MutexGuard<'_, BridgeState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn ready_message(runtime: RuntimeScope, pending: &PendingTeleport) -> TeleportMessage {
    TeleportMessage::Ready(TeleportReadyMessage {
        teleport_id: pending.teleport_id.clone(),
        runtime,
        success: pending.success,
        active_turn: pending.active_turn,
        mode: None,
        continuation: None,
        error: pending.error.clone(),
    })
}

fn failed_ready(teleport_id: String, runtime: RuntimeScope, error: &str) -> TeleportMessage {
    TeleportMessage::Ready(TeleportReadyMessage {
        teleport_id,
        runtime,
        success: false,
        active_turn: false,
        mode: None,
        continuation: None,
        error: Some(error.to_owned()),
    })
}

fn continuation_item(
    payload: &TeleportContinuePayload,
    enqueued_at: Timestamp,
) -> Result<QueueItem, RuntimeError> {
    let content = content_value(payload)?;
    Ok(QueueItem {
        id: item_id("teleport-item-", payload.teleport_id.as_str())?,
        client_message_id: item_id("teleport:", payload.teleport_id.as_str())?,
        kind: QueueItemKind::ApprovalResult,
        source: QueueItemSource::System,
        content,
        enqueued_at,
        extras: EntityExtras::default(),
    })
}

fn item_id(prefix: &str, teleport_id: &str) -> Result<NonEmptyString, RuntimeError> {
    NonEmptyString::new(format!("{prefix}{teleport_id}")).map_err(|_| RuntimeError::InvalidData {
        context: "teleport continuation identifiers".to_owned(),
    })
}

fn content_value(payload: &TeleportContinuePayload) -> Result<BoundedJsonValue, RuntimeError> {
    let approvals = payload
        .continuation
        .as_ref()
        .map_or(Value::Null, |continuation| {
            Value::Array(continuation.approvals.clone())
        });
    BoundedJsonValue::new(serde_json::json!({
        "kind": "teleport_continue",
        "teleport_id": payload.teleport_id.as_str(),
        "approvals": approvals,
    }))
    .map_err(|_| RuntimeError::InvalidData {
        context: "teleport continuation content".to_owned(),
    })
}

#[cfg(test)]
#[path = "teleport_continuation_tests.rs"]
mod continuation;
#[cfg(test)]
#[path = "teleport_failure_tests.rs"]
mod failure;
#[cfg(test)]
#[path = "teleport_fixture_round_trip_tests.rs"]
mod fixture_round_trip;
#[cfg(test)]
#[path = "teleport_request_tests.rs"]
mod requests;
#[cfg(test)]
#[path = "teleport_stale_lease_tests.rs"]
mod stale_lease;
