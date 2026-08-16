//! Runtime-scoped controller-owned external tools.

mod call;
mod registry;

pub use call::{
    ConnectionId, ControllerConnection, ControllerReceiver, ExternalCallRequest,
    ExternalCallResponse, ExternalRequestId, ResponseDisposition, RuntimeId, ScopeId, ToolCallId,
};
pub use registry::{
    ExternalRegistrationError, ExternalToolGroup, ExternalToolManager, ExternalToolMember,
    GroupRevision, RegistrySelection,
};

use lotta_runtime::ports::{ToolOutcomeCode, ToolOutcomeMessage};

/// Stable external-call failures retained as tool outcomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalCallFailure {
    /// The server-owned five-minute deadline elapsed.
    Timeout,
    /// The registration's exact controller connection disconnected.
    OwnerDisconnected,
    /// A response came from a different connection identity or generation.
    OwnerMismatch,
    /// Response shape, bounds, or correlation was invalid.
    InvalidResponse,
    /// The runtime cancelled the call.
    Cancellation,
}

impl ExternalCallFailure {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Timeout => "external_timeout",
            Self::OwnerDisconnected => "external_owner_disconnected",
            Self::OwnerMismatch => "external_owner_mismatch",
            Self::InvalidResponse => "external_invalid_response",
            Self::Cancellation => "external_cancelled",
        }
    }

    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Timeout => "external tool call timed out",
            Self::OwnerDisconnected => "external tool controller disconnected",
            Self::OwnerMismatch => "external tool response owner mismatch",
            Self::InvalidResponse => "external tool response was invalid",
            Self::Cancellation => "external tool call was cancelled",
        }
    }

    pub(crate) fn outcome(self) -> lotta_runtime::ports::ToolOutcome {
        if self == Self::Timeout {
            return lotta_runtime::ports::ToolOutcome::Timeout {
                message: bounded_message(self.message()),
            };
        }
        if self == Self::Cancellation {
            return lotta_runtime::ports::ToolOutcome::Interrupted {
                message: bounded_message(self.message()),
            };
        }
        lotta_runtime::ports::ToolOutcome::ToolDefinedError {
            code: ToolOutcomeCode::new(self.code().to_owned()).unwrap_or_else(|_| unreachable!()),
            message: bounded_message(self.message()),
        }
    }
}

fn bounded_message(value: &'static str) -> ToolOutcomeMessage {
    match ToolOutcomeMessage::new(value.to_owned()) {
        Ok(message) => message,
        Err(_) => unreachable!("fixed external outcome messages satisfy canonical bounds"),
    }
}

#[cfg(test)]
mod tests;
