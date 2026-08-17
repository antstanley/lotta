use super::PortFuture;
use crate::boundary::{ProviderName, ProviderText};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Structured one-shot model request for bounded internal capabilities.
#[derive(Clone, Debug)]
pub struct ModelCapabilityRequest {
    /// Bounded system instruction.
    pub system_prompt: ProviderText,
    /// Bounded user prompt.
    pub user_prompt: ProviderText,
    /// Optional bounded model override identifier.
    pub model: Option<ProviderName>,
    /// Fixed server-owned deadline.
    pub timeout: Duration,
    /// Explicit cancellation token.
    pub cancellation: CancellationToken,
}

/// Bounded model-generated text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCapabilityResponse {
    /// Generated response content.
    pub content: ProviderText,
}

/// One-shot model capability used by internal evaluators such as prompt hooks.
pub trait ModelCapabilityPort: Send + Sync {
    /// Generates one bounded response.
    fn generate(&self, request: ModelCapabilityRequest) -> PortFuture<'_, ModelCapabilityResponse>;
}
