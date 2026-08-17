use super::registrations::RegistrationBatch;
use super::types::{Capability, ConversationHandle, Generation, ModError, ModOwner};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC protocol marker.
pub const JSON_RPC_VERSION: &str = "2.0";
/// Maximum pending host calls.
pub const MOD_HOST_PENDING_CALLS_MAX: usize = 128;
/// Maximum actor commands waiting for the host.
pub const MOD_HOST_QUEUE_ITEMS_MAX: usize = 128;
/// Default bounded local host operation timeout.
pub const MOD_HOST_CALL_TIMEOUT_MS_DEFAULT: u64 = 30_000;
/// Maximum JSON-RPC method name size.
pub const MOD_HOST_METHOD_BYTES_MAX: usize = 128;

/// Bounded positive JSON-RPC request identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RpcId(u64);
impl RpcId {
    /// Validates a positive request identity.
    pub fn new(value: u64) -> Result<Self, ModError> {
        if value == 0 {
            Err(ModError::Protocol)
        } else {
            Ok(Self(value))
        }
    }
    /// Returns its wire value.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Strict JSON-RPC request envelope used inside Task 42 payloads.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RpcRequest {
    /// Exact JSON-RPC version.
    pub jsonrpc: String,
    /// Correlation identity.
    pub id: RpcId,
    /// Exact typed method.
    pub method: RpcMethod,
    /// Method parameters.
    pub params: RpcParams,
}

/// Strict JSON-RPC response envelope.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RpcResponse {
    /// Exact JSON-RPC version.
    pub jsonrpc: String,
    /// Correlation identity.
    pub id: RpcId,
    /// Successful result, mutually exclusive with `error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<RpcResult>,
    /// Remote error, mutually exclusive with `result`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// Mutually exclusive JSON-RPC result or error.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RpcOutcome {
    /// Successful result.
    Result {
        /// Typed successful result.
        result: RpcResult,
    },
    /// Typed bounded error.
    Error {
        /// Typed secret-free error.
        error: RpcError,
    },
}

/// Stable secret-free RPC error.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    /// Stable JSON-RPC code.
    pub code: i32,
    /// Bounded non-sensitive message.
    pub message: String,
}

/// Exact host methods.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RpcMethod {
    /// Initialize a mod generation.
    #[serde(rename = "initialize")]
    Initialize,
    /// Atomically register one complete batch.
    #[serde(rename = "register")]
    Register,
    /// Invoke a registered tool.
    #[serde(rename = "tool.call")]
    ToolCall,
    /// Invoke a registered command.
    #[serde(rename = "command.call")]
    CommandCall,
    /// Invoke a registered lifecycle event.
    #[serde(rename = "lifecycle.call")]
    LifecycleCall,
    /// Invoke a declared runtime capability.
    #[serde(rename = "capability.call")]
    CapabilityCall,
    /// Retrieve diagnostics.
    #[serde(rename = "diagnostics")]
    Diagnostics,
    /// Dispose one generation.
    #[serde(rename = "dispose")]
    Dispose,
    /// Prepare a replacement generation.
    #[serde(rename = "reload")]
    Reload,
    /// Cancel one pending request.
    #[serde(rename = "cancel")]
    Cancel,
}

/// Typed parameters for every method.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RpcParams {
    /// Initialization declaration.
    Initialize {
        /// Exact owner.
        /// Typed method field.
        owner: ModOwner,
        /// Declared capabilities.
        capabilities: Vec<Capability>,
        /// Opaque scoped conversation handle.
        /// Typed method field.
        conversation_handle: ConversationHandle,
    },
    /// Complete registration batch.
    Register {
        /// Typed method field.
        owner: ModOwner,
        /// Typed method field.
        registrations: RegistrationBatch,
    },
    /// Tool invocation.
    ToolCall {
        /// Typed method field.
        owner: ModOwner,
        /// Typed method field.
        name: String,
        /// Typed method field.
        tool_call_id: String,
        /// Typed method field.
        input: Value,
    },
    /// Command invocation.
    CommandCall {
        /// Typed method field.
        owner: ModOwner,
        /// Typed method field.
        name: String,
        /// Typed method field.
        arguments: Value,
    },
    /// Lifecycle invocation.
    LifecycleCall {
        /// Typed method field.
        owner: ModOwner,
        /// Typed method field.
        name: String,
        /// Typed method field.
        payload: Value,
    },
    /// Capability invocation.
    CapabilityCall {
        /// Typed method field.
        owner: ModOwner,
        /// Typed method field.
        conversation_handle: ConversationHandle,
        /// Typed method field.
        capability: Capability,
        /// Typed method field.
        operation: String,
        /// Typed method field.
        params: Value,
    },
    /// Diagnostic request.
    Diagnostics {
        /// Exact owner.
        owner: ModOwner,
    },
    /// Disposal request.
    Dispose {
        /// Exact owner.
        owner: ModOwner,
    },
    /// Reload request.
    Reload {
        /// Typed method field.
        owner: ModOwner,
        /// Typed method field.
        generation: Generation,
    },
    /// Cancellation request.
    Cancel {
        /// Typed method field.
        owner: ModOwner,
        /// Pending request identity.
        request_id: RpcId,
    },
}

/// Typed results for every request.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RpcResult {
    /// Initialization acknowledgement.
    Initialized,
    /// Registration acknowledgement.
    Registered,
    /// Generic JSON value returned by a call.
    Value {
        /// Returned bounded value.
        value: Value,
    },
    /// Diagnostics list.
    Diagnostics {
        /// Bounded safe diagnostics.
        diagnostics: Vec<String>,
    },
    /// Disposal acknowledgement.
    Disposed,
    /// Reload acknowledgement.
    Reloaded,
    /// Cancellation acknowledgement.
    Cancelled,
    /// Complete registration batch produced by the loaded mod API.
    RegistrationBatch {
        /// Fully attributed registrations.
        registrations: RegistrationBatch,
    },
}

impl RpcRequest {
    /// Creates a strict request.
    #[must_use]
    pub fn new(id: RpcId, method: RpcMethod, params: RpcParams) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION.into(),
            id,
            method,
            params,
        }
    }
    /// Validates version and method/parameter agreement.
    pub fn validate(&self) -> Result<(), ModError> {
        if self.jsonrpc != JSON_RPC_VERSION || !method_matches(self.method, &self.params) {
            return Err(ModError::Protocol);
        }
        Ok(())
    }
}
impl RpcResponse {
    /// Validates exact version and secret-free bounded error messages.
    pub fn validate(&self) -> Result<(), ModError> {
        if self.jsonrpc != JSON_RPC_VERSION {
            return Err(ModError::Protocol);
        }
        if self.result.is_some() == self.error.is_some() {
            return Err(ModError::Protocol);
        }
        if let Some(error) = &self.error
            && (error.message.is_empty() || error.message.len() > 1_024)
        {
            return Err(ModError::Protocol);
        }
        Ok(())
    }
}

fn method_matches(method: RpcMethod, params: &RpcParams) -> bool {
    matches!(
        (method, params),
        (RpcMethod::Initialize, RpcParams::Initialize { .. })
            | (RpcMethod::Register, RpcParams::Register { .. })
            | (RpcMethod::ToolCall, RpcParams::ToolCall { .. })
            | (RpcMethod::CommandCall, RpcParams::CommandCall { .. })
            | (RpcMethod::LifecycleCall, RpcParams::LifecycleCall { .. })
            | (RpcMethod::CapabilityCall, RpcParams::CapabilityCall { .. })
            | (RpcMethod::Diagnostics, RpcParams::Diagnostics { .. })
            | (RpcMethod::Dispose, RpcParams::Dispose { .. })
            | (RpcMethod::Reload, RpcParams::Reload { .. })
            | (RpcMethod::Cancel, RpcParams::Cancel { .. })
    )
}
