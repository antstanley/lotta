//! Prompt-hook execution over the runtime model capability.

use super::events::{HookEvent, HookOutcome, HookPayload, HookReason};
use lotta_runtime::{
    RuntimeError,
    boundary::{ProviderName, ProviderText},
    ports::{ModelCapabilityPort, ModelCapabilityRequest},
};
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

/// Fixed server-owned prompt hook timeout.
pub const PROMPT_HOOK_TIMEOUT_MS_DEFAULT: u64 = 30_000;
/// Maximum configured prompt bytes.
pub const PROMPT_HOOK_TEXT_BYTES_MAX: usize = 262_144;
/// Maximum model response bytes parsed by the hook executor.
pub const PROMPT_HOOK_RESPONSE_BYTES_MAX: usize = 16_384;
/// Pinned `$ARGUMENTS` placeholder.
pub const PROMPT_ARGUMENTS_PLACEHOLDER: &str = "$ARGUMENTS";

const PROMPT_HOOK_SYSTEM: &str = concat!(
    "You are a hook evaluator for a coding assistant. Your job is to evaluate whether an ",
    "action should be allowed or blocked based on the provided context and criteria.\n\n",
    "You will receive:\n",
    "1. Hook input JSON containing context about the action (event type, tool info, etc.)\n",
    "2. A user-defined prompt with evaluation criteria\n\n",
    "You must respond with ONLY a valid JSON object (no markdown, no explanation) with the ",
    "following fields:\n",
    "- \"ok\": true to allow the action, false to prevent it\n",
    "- \"reason\": Required when ok is false. Explanation for your decision.\n\n",
    "Example responses:\n",
    "- To allow: {\"ok\": true}\n",
    "- To block: {\"ok\": false, \"reason\": \"This action violates the security policy\"}\n\n",
    "Respond with JSON only. No markdown code blocks. No explanation outside the JSON."
);

/// Pinned prompt hook configuration.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct PromptHookConfig {
    /// Exact pinned hook type.
    #[serde(rename = "type")]
    pub kind: PromptHookType,
    /// User-defined evaluation prompt.
    pub prompt: String,
    /// Optional model override identifier.
    #[serde(default)]
    pub model: Option<String>,
    /// Caller value retained for compatibility but cannot extend the server deadline.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Suppresses presentation output in higher layers.
    #[serde(default)]
    pub quiet: bool,
}
impl fmt::Debug for PromptHookConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PromptHookConfig")
            .field("kind", &self.kind)
            .field("prompt", &"[REDACTED]")
            .field("model", &self.model.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .field("quiet", &self.quiet)
            .finish()
    }
}

/// Exact prompt hook discriminant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PromptHookType {
    /// Model-backed prompt evaluation.
    #[serde(rename = "prompt")]
    Prompt,
}

/// Exact structured model decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PromptHookResponse {
    /// Whether the action is allowed.
    pub ok: bool,
    /// Required non-empty reason when blocked.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Concrete prompt hook executor.
pub struct PromptHookExecutor {
    model: Arc<dyn ModelCapabilityPort>,
}
impl PromptHookExecutor {
    /// Creates an executor over the canonical model capability port.
    #[must_use]
    pub fn new(model: Arc<dyn ModelCapabilityPort>) -> Self {
        Self { model }
    }
    /// Executes one supported prompt hook with fixed timeout and bounded parsing.
    pub async fn execute(
        &self,
        event: HookEvent,
        config: &PromptHookConfig,
        payload: &HookPayload,
        cancellation: CancellationToken,
    ) -> Result<HookOutcome, PromptHookError> {
        if !event.supports_prompt() || event != payload.event() {
            return Err(PromptHookError::Unsupported);
        }
        if config.prompt.is_empty() || config.prompt.len() > PROMPT_HOOK_TEXT_BYTES_MAX {
            return Err(PromptHookError::Config);
        }
        let user_prompt = build_prompt(&config.prompt, payload)?;
        let timeout_ms = config
            .timeout
            .unwrap_or(PROMPT_HOOK_TIMEOUT_MS_DEFAULT)
            .min(PROMPT_HOOK_TIMEOUT_MS_DEFAULT);
        let deadline = Duration::from_millis(timeout_ms);
        let model_cancellation = cancellation.child_token();
        let request = ModelCapabilityRequest {
            system_prompt: ProviderText::new(PROMPT_HOOK_SYSTEM.into())
                .map_err(|_| PromptHookError::Config)?,
            user_prompt: ProviderText::new(user_prompt).map_err(|_| PromptHookError::Config)?,
            model: config
                .model
                .clone()
                .map(ProviderName::new)
                .transpose()
                .map_err(|_| PromptHookError::Config)?,
            timeout: deadline,
            cancellation: model_cancellation.clone(),
        };
        let generate = tokio::time::timeout(deadline, self.model.generate(request));
        tokio::pin!(generate);
        let response = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                model_cancellation.cancel();
                return Err(PromptHookError::Cancelled);
            }
            result = &mut generate => if let Ok(result) = result {
                result.map_err(map_runtime)?
            } else {
                model_cancellation.cancel();
                return Err(PromptHookError::Timeout);
            },
        };
        parse_response(response.content.as_str())
    }
}

fn build_prompt(prompt: &str, payload: &HookPayload) -> Result<String, PromptHookError> {
    let input =
        serde_json::to_string_pretty(payload.value()).map_err(|_| PromptHookError::Payload)?;
    let built = if prompt.contains(PROMPT_ARGUMENTS_PLACEHOLDER) {
        prompt.replace(PROMPT_ARGUMENTS_PLACEHOLDER, &input)
    } else {
        format!("{prompt}\n\nHook input:\n{input}")
    };
    if built.len() > PROMPT_HOOK_TEXT_BYTES_MAX {
        return Err(PromptHookError::Payload);
    }
    Ok(built)
}
fn parse_response(response: &str) -> Result<HookOutcome, PromptHookError> {
    if response.len() > PROMPT_HOOK_RESPONSE_BYTES_MAX {
        return Err(PromptHookError::Response);
    }
    let parsed: PromptHookResponse =
        serde_json::from_str(response.trim()).map_err(|_| PromptHookError::Response)?;
    if parsed.ok {
        if parsed.reason.is_some() {
            return Err(PromptHookError::Response);
        }
        return Ok(HookOutcome::Allow);
    }
    let reason = parsed.reason.ok_or(PromptHookError::Response)?;
    Ok(HookOutcome::Block(
        HookReason::new(reason).map_err(|_| PromptHookError::Response)?,
    ))
}
fn map_runtime(error: RuntimeError) -> PromptHookError {
    match error {
        RuntimeError::Cancelled { .. } => PromptHookError::Cancelled,
        RuntimeError::Timeout { .. } => PromptHookError::Timeout,
        _ => PromptHookError::Model,
    }
}

/// Stable prompt hook failure without prompt, model, key, payload, or response text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PromptHookError {
    /// Event does not support prompt hooks.
    #[error("prompt hook unsupported for event")]
    Unsupported,
    /// Configuration was invalid or overbound.
    #[error("invalid prompt hook configuration")]
    Config,
    /// Payload or constructed prompt was invalid or overbound.
    #[error("invalid prompt hook payload")]
    Payload,
    /// Model capability failed.
    #[error("prompt hook model failed")]
    Model,
    /// Fixed server deadline elapsed.
    #[error("prompt hook timeout")]
    Timeout,
    /// Explicit cancellation won.
    #[error("prompt hook cancelled")]
    Cancelled,
    /// Model response was malformed or overbound.
    #[error("invalid prompt hook response")]
    Response,
}
