use super::{ExternalCallFailure, registry::ManagerCore};
use lotta_domain::BoundedJsonValue;
use lotta_runtime::bounds::{
    TOOL_NAME_BYTES_MAX, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX,
};
use serde_json::Value;
use std::{
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::mpsc;

const CONTROLLER_OUTGOING_CALLS_MAX: usize = 256;

macro_rules! external_id {
    ($name:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);
        impl $name {
            /// Validates a non-empty, bounded, NUL-free opaque identifier.
            ///
            /// # Errors
            /// Returns [`ExternalCallFailure::InvalidResponse`] when invalid.
            pub fn new(value: String) -> Result<Self, ExternalCallFailure> {
                if value.is_empty()
                    || value.len() > TOOL_NAME_BYTES_MAX.value
                    || value.contains('\0')
                {
                    return Err(ExternalCallFailure::InvalidResponse);
                }
                Ok(Self(value))
            }
            /// Borrows the validated identifier.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

external_id!(RuntimeId, "Validated runtime collection identifier.");
external_id!(
    ExternalRequestId,
    "Validated external call request identifier."
);
external_id!(ToolCallId, "Validated provider tool-call identifier.");
external_id!(ConnectionId, "Validated controller transport identifier.");
external_id!(ScopeId, "Validated optional external-tool selection scope.");

/// A bounded request emitted to exactly one owning controller connection.
#[derive(Clone)]
pub struct ExternalCallRequest {
    /// Runtime collection identity.
    pub runtime_id: RuntimeId,
    /// Server-generated collision-safe request identity.
    pub request_id: ExternalRequestId,
    /// Provider tool-call identity.
    pub tool_call_id: ToolCallId,
    /// Canonical internal tool name.
    pub internal_name: lotta_runtime::ports::InternalToolName,
    /// Exposed model-facing name.
    pub model_name: lotta_runtime::ports::ModelFacingToolName,
    /// Schema-validated bounded arguments.
    pub arguments: lotta_runtime::ports::ValidatedToolInput,
    /// Exact optional registration scope.
    pub scope_id: Option<ScopeId>,
}

impl fmt::Debug for ExternalCallRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExternalCallRequest")
            .field("runtime_id", &self.runtime_id)
            .field("request_id", &self.request_id)
            .field("tool_call_id", &self.tool_call_id)
            .field("internal_name", &self.internal_name)
            .field("model_name", &self.model_name)
            .field("arguments", &"[REDACTED]")
            .field("scope_id", &self.scope_id)
            .finish()
    }
}

/// Controller response carrying all correlation fields.
#[derive(Clone, Debug)]
pub struct ExternalCallResponse {
    /// Runtime collection identity copied from the request.
    pub runtime_id: RuntimeId,
    /// Request identity copied from the request.
    pub request_id: ExternalRequestId,
    /// Tool-call identity copied from the request.
    pub tool_call_id: ToolCallId,
    /// Internal tool name copied from the request.
    pub internal_name: lotta_runtime::ports::InternalToolName,
    /// Model-facing name copied from the request.
    pub model_name: lotta_runtime::ports::ModelFacingToolName,
    /// Exact optional scope copied from the request.
    pub scope_id: Option<ScopeId>,
    /// Result payload, mutually exclusive with `error`.
    pub result: Option<Value>,
    /// Error text, mutually exclusive with `result`.
    pub error: Option<String>,
}

/// Fixed disposition of a controller response attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseDisposition {
    /// The exact pending call was resolved.
    Resolved,
    /// No pending request has this identity; late/duplicate/unknown responses are ignored.
    UnknownIgnored,
    /// The request belongs to another connection identity or generation.
    OwnerMismatch,
    /// Correlation or bounded response payload was invalid and the call was rejected.
    InvalidResponse,
}

/// Receiving half of a bounded controller call channel.
pub struct ControllerReceiver {
    receiver: mpsc::Receiver<ExternalCallRequest>,
}
impl ControllerReceiver {
    /// Receives the next bounded call request.
    pub async fn recv(&mut self) -> Option<ExternalCallRequest> {
        self.receiver.recv().await
    }
}

/// Owning handle for one exact controller connection identity and generation.
pub struct ControllerConnection {
    pub(crate) id: ConnectionId,
    pub(crate) generation: u64,
    pub(crate) sender: mpsc::Sender<ExternalCallRequest>,
    pub(crate) manager: Weak<ManagerCore>,
    closed: AtomicBool,
}

impl ControllerConnection {
    pub(crate) fn create(
        id: ConnectionId,
        generation: u64,
        manager: &Arc<ManagerCore>,
    ) -> (Arc<Self>, ControllerReceiver) {
        let (sender, receiver) = mpsc::channel(CONTROLLER_OUTGOING_CALLS_MAX);
        let handle = Arc::new(Self {
            id,
            generation,
            sender,
            manager: Arc::downgrade(manager),
            closed: AtomicBool::new(false),
        });
        (handle, ControllerReceiver { receiver })
    }

    /// Returns the stable transport identity.
    #[must_use]
    pub const fn id(&self) -> &ConnectionId {
        &self.id
    }

    /// Returns the monotonic manager-assigned connection generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Explicitly closes this exact transport generation and drains its pending calls.
    /// This is idempotent and is the production lifecycle seam used by Task 62.
    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel)
            && let Some(manager) = self.manager.upgrade()
        {
            manager.disconnect(&self.id, self.generation);
        }
    }

    /// Submits a response through this exact identity and generation.
    ///
    /// # Errors
    /// Returns owner mismatch or invalid-response failures without affecting another call.
    pub fn respond(
        &self,
        response: ExternalCallResponse,
    ) -> Result<ResponseDisposition, ExternalCallFailure> {
        let Some(manager) = self.manager.upgrade() else {
            return Ok(ResponseDisposition::UnknownIgnored);
        };
        manager.respond(self, response)
    }
}

impl Drop for ControllerConnection {
    fn drop(&mut self) {
        self.close();
    }
}

pub(crate) fn validate_response_payload(response: &ExternalCallResponse) -> bool {
    match (&response.result, &response.error) {
        (Some(value), None) => bounded_json(value),
        (None, Some(error)) => bounded_text(error),
        _ => false,
    }
}

fn bounded_json(value: &Value) -> bool {
    BoundedJsonValue::new(value.clone()).is_ok()
        && serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= TOOL_RESULT_BYTES_MAX.value)
        && bounded_text(
            &value
                .as_str()
                .map_or_else(|| value.to_string(), ToOwned::to_owned),
        )
}

fn bounded_text(value: &str) -> bool {
    value.len() <= TOOL_RESULT_BYTES_MAX.value
        && value.chars().count() <= TOOL_RESULT_MODEL_CHARS_MAX.value
}
