//! Provider context-window resolution, estimation, and bounded overflow recovery.

use lotta_runtime::ports::{
    ProviderContentPart, ProviderContext, ProviderContextDecision, ProviderContextOverflowDetail,
    ProviderContextTokenCount, ProviderContextTokenProvenance, ProviderRequest,
};
use serde::{Deserialize, Serialize};

/// Exactly three provider-reported overflow compactions are permitted before terminal failure.
pub use lotta_runtime::bounds::CONTEXT_OVERFLOW_COMPACTIONS_MAX;
/// Conservative fallback estimates one token per this many input bytes, rounded upward.
pub const FALLBACK_BYTES_PER_TOKEN: u64 = 3;
/// Fixed image estimate inherited from the pinned local-context estimator.
pub const IMAGE_TOKENS_ESTIMATE: u64 = 1_200;
/// Maximum token estimate retained in typed diagnostics.
pub const CONTEXT_TOKENS_ESTIMATE_MAX: u64 = u32::MAX as u64;
/// Structural overhead charged for each message role/envelope.
pub const MESSAGE_STRUCTURAL_TOKENS: u64 = 8;
/// Structural overhead charged for each tool definition.
pub const TOOL_STRUCTURAL_TOKENS: u64 = 16;

/// Four independently optional context-window ceilings.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContextWindowSources {
    /// Configured server maximum.
    pub server_max: Option<u64>,
    /// Model catalog context window.
    pub model_catalog: Option<u64>,
    /// Agent-level context setting.
    pub agent: Option<u64>,
    /// Conversation-level context override.
    pub conversation: Option<u64>,
}

/// Invalid effective context-window configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ContextWindowError {
    /// One supplied source was zero.
    #[error("context window source must be positive")]
    Zero,
    /// No context-window source was configured.
    #[error("context window source missing")]
    Missing,
    /// The selected value cannot be represented by the runtime token-limit type.
    #[error("context window exceeds supported range")]
    Overflow,
}

/// Computes the effective positive window as the minimum of every present source.
///
/// # Errors
/// Rejects zero, absent, and values above the runtime's supported token range.
pub fn effective_window(sources: ContextWindowSources) -> Result<u64, ContextWindowError> {
    let values = [
        sources.server_max,
        sources.model_catalog,
        sources.agent,
        sources.conversation,
    ];
    if values.into_iter().flatten().any(|value| value == 0) {
        return Err(ContextWindowError::Zero);
    }
    let selected = values
        .into_iter()
        .flatten()
        .min()
        .ok_or(ContextWindowError::Missing)?;
    if selected > CONTEXT_TOKENS_ESTIMATE_MAX {
        Err(ContextWindowError::Overflow)
    } else {
        Ok(selected)
    }
}

/// Catalog-selected tokenizer estimate port.
pub trait TokenEstimatePort: Send + Sync {
    /// Estimates model-visible tokens with the catalog tokenizer.
    ///
    /// # Errors
    /// Returns overflow when a result exceeds the bounded diagnostic range.
    fn estimate(&self, request: &ProviderRequest) -> Result<u64, ContextWindowError>;
}

/// Tokenizer metadata supplied by a provider/model catalog when available.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenizerMetadata {
    /// Provider-declared tokenizer family identifier from model catalog metadata.
    Family(String),
    /// Provider-declared deterministic byte ratio.
    BytesPerToken(u8),
}

/// Safe source of a context token count.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenCountProvenance {
    /// Count measured by a provider response.
    Measured,
    /// Estimate used provider/model tokenizer metadata.
    ProviderTokenizer,
    /// Estimate used the named conservative fallback byte rule.
    FallbackBytesPerToken,
}

/// Typed bounded token count and its provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextTokenCount {
    /// Bounded token count.
    pub tokens: u64,
    /// How the count was obtained.
    pub provenance: TokenCountProvenance,
}

impl ContextTokenCount {
    /// Creates a bounded measured token count.
    ///
    /// # Errors
    /// Rejects values above the diagnostic bound.
    pub fn measured(tokens: u64) -> Result<Self, ContextWindowError> {
        bounded_count(tokens, TokenCountProvenance::Measured)
    }
}

/// Estimates all model-visible request content, including system, images, tools, and tool results.
#[must_use]
pub fn estimate_request_tokens(
    request: &ProviderRequest,
    tokenizer: Option<&TokenizerMetadata>,
) -> ContextTokenCount {
    let mut bytes = request
        .system_prompt
        .as_ref()
        .map_or(0_u64, |text| saturating_len(text.as_str()));
    let mut image_tokens = 0_u64;
    let mut structural = 0_u64;
    for message in request.messages.as_slice() {
        structural = structural.saturating_add(MESSAGE_STRUCTURAL_TOKENS);
        bytes = bytes.saturating_add(
            message
                .tool_call_id
                .as_ref()
                .map_or(0, |id| saturating_len(id.as_str())),
        );
        for part in message.content.as_slice() {
            match part {
                ProviderContentPart::Text(text) => {
                    bytes = bytes.saturating_add(saturating_len(text.as_str()));
                }
                ProviderContentPart::Image { media_type, .. } => {
                    bytes = bytes.saturating_add(saturating_len(media_type.as_str()));
                    image_tokens = image_tokens.saturating_add(IMAGE_TOKENS_ESTIMATE);
                }
            }
        }
    }
    for tool in request.tools.as_slice() {
        structural = structural.saturating_add(TOOL_STRUCTURAL_TOKENS);
        bytes = bytes.saturating_add(saturating_len(tool.name.as_str()));
        bytes = bytes.saturating_add(saturating_len(tool.description.as_str()));
        bytes = bytes.saturating_add(saturating_len(&tool.input_schema.as_value().to_string()));
    }
    let (ratio, provenance) = tokenizer_rule(tokenizer);
    let text_tokens = bytes.div_ceil(ratio);
    ContextTokenCount {
        tokens: text_tokens
            .saturating_add(image_tokens)
            .saturating_add(structural)
            .min(CONTEXT_TOKENS_ESTIMATE_MAX),
        provenance,
    }
}

fn tokenizer_rule(tokenizer: Option<&TokenizerMetadata>) -> (u64, TokenCountProvenance) {
    match tokenizer {
        Some(TokenizerMetadata::BytesPerToken(value)) if *value > 0 => {
            (u64::from(*value), TokenCountProvenance::ProviderTokenizer)
        }
        Some(TokenizerMetadata::Family(_) | TokenizerMetadata::BytesPerToken(_)) | None => (
            FALLBACK_BYTES_PER_TOKEN,
            TokenCountProvenance::FallbackBytesPerToken,
        ),
    }
}

fn saturating_len(value: &str) -> u64 {
    u64::try_from(value.len()).unwrap_or(u64::MAX)
}

fn bounded_count(
    tokens: u64,
    provenance: TokenCountProvenance,
) -> Result<ContextTokenCount, ContextWindowError> {
    if tokens > CONTEXT_TOKENS_ESTIMATE_MAX {
        Err(ContextWindowError::Overflow)
    } else {
        Ok(ContextTokenCount { tokens, provenance })
    }
}

/// Public context preflight guard shared by all provider adapter boundaries.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContextGuard;

impl ContextGuard {
    /// Calculates the effective minimum, estimates use, and rejects over-limit requests.
    ///
    /// # Errors
    /// Returns typed invalid configuration or a safe terminal overflow detail.
    pub fn check(
        self,
        request: &ProviderRequest,
        tokenizer: Option<&dyn TokenEstimatePort>,
    ) -> Result<ContextTokenCount, ContextGuardError> {
        let context = request.context.unwrap_or_else(|| ProviderContext {
            catalog_max: request
                .model
                .context_window
                .or(Some(request.context_tokens_max.get())),
            ..ProviderContext::default()
        });
        let limit = effective_window(ContextWindowSources {
            server_max: context.server_max,
            model_catalog: context
                .catalog_max
                .or(request.model.context_window)
                .or(Some(request.context_tokens_max.get())),
            agent: context.agent_max,
            conversation: context.conversation_max,
        })?;
        let estimated = match tokenizer {
            Some(port) => bounded_count(
                port.estimate(request)?,
                TokenCountProvenance::ProviderTokenizer,
            )?,
            None => estimate_request_tokens(request, None),
        };
        let measured = context
            .measured_input_tokens
            .map(ContextTokenCount::measured)
            .transpose()?;
        let observed = measured.unwrap_or(estimated);
        if observed.tokens > limit {
            let detail = ProviderContextOverflowDetail {
                measured: measured.map(runtime_count),
                estimated: runtime_count(estimated),
                limit,
                provider: request.model.provider_id.as_str().to_owned(),
                model: request.model.handle.as_str().to_owned(),
                attempt: context.compactions_completed.saturating_add(1),
                compactions_completed: context.compactions_completed,
            };
            let decision = if context.compactions_completed < CONTEXT_OVERFLOW_COMPACTIONS_MAX {
                ProviderContextDecision::CompactionRequired(detail)
            } else {
                ProviderContextDecision::ContextOverflow(detail)
            };
            return Err(ContextGuardError::Overflow(decision));
        }
        Ok(observed)
    }
}

/// Context preflight failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ContextGuardError {
    /// Invalid context policy or estimate.
    #[error(transparent)]
    Window(#[from] ContextWindowError),
    /// Typed preflight overflow decision.
    #[error("provider context window exceeded")]
    Overflow(ProviderContextDecision),
}

fn runtime_count(value: ContextTokenCount) -> ProviderContextTokenCount {
    ProviderContextTokenCount {
        tokens: value.tokens,
        provenance: match value.provenance {
            TokenCountProvenance::Measured => ProviderContextTokenProvenance::Measured,
            TokenCountProvenance::ProviderTokenizer => {
                ProviderContextTokenProvenance::ProviderTokenizer
            }
            TokenCountProvenance::FallbackBytesPerToken => {
                ProviderContextTokenProvenance::FallbackBytesPerToken
            }
        },
    }
}

/// State-machine decision after a provider reports context overflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextOverflowAction {
    /// Compact, rebuild the request, and retry once.
    CompactAndRetry {
        /// One-based provider attempt to execute after compaction.
        attempt: u8,
    },
    /// Repeated overflow is terminal with safe structured details.
    Terminal(ProviderContextOverflowDetail),
}

/// Bounded context-overflow retry state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContextOverflowTracker {
    compactions: u8,
}

impl ContextOverflowTracker {
    /// Handles one provider overflow without inspecting or retaining message content.
    #[must_use]
    pub fn on_overflow(
        &mut self,
        measured: Option<ContextTokenCount>,
        estimated: ContextTokenCount,
        limit: u64,
        provider: &str,
        model: &str,
    ) -> ContextOverflowAction {
        if self.compactions < CONTEXT_OVERFLOW_COMPACTIONS_MAX {
            self.compactions += 1;
            return ContextOverflowAction::CompactAndRetry {
                attempt: self.compactions.saturating_add(1),
            };
        }
        ContextOverflowAction::Terminal(ProviderContextOverflowDetail {
            measured: measured.map(runtime_count),
            estimated: runtime_count(estimated),
            limit,
            provider: provider.to_owned(),
            model: model.to_owned(),
            attempt: self.compactions.saturating_add(1),
            compactions_completed: self.compactions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_window_uses_four_source_minimum_and_validates() {
        let sources = ContextWindowSources {
            server_max: Some(128_000),
            model_catalog: Some(64_000),
            agent: Some(32_000),
            conversation: Some(16_000),
        };
        assert_eq!(effective_window(sources), Ok(16_000));
        assert_eq!(
            effective_window(ContextWindowSources::default()),
            Err(ContextWindowError::Missing)
        );
        assert_eq!(
            effective_window(ContextWindowSources {
                server_max: Some(0),
                ..sources
            }),
            Err(ContextWindowError::Zero)
        );
        assert_eq!(
            effective_window(ContextWindowSources {
                server_max: Some(u64::MAX),
                model_catalog: None,
                agent: None,
                conversation: None
            }),
            Err(ContextWindowError::Overflow)
        );
    }

    #[test]
    fn repeated_overflow_terminal_is_safe_and_typed() {
        let mut state = ContextOverflowTracker::default();
        let estimated = ContextTokenCount {
            tokens: 9_000,
            provenance: TokenCountProvenance::FallbackBytesPerToken,
        };
        let measured = ContextTokenCount::measured(8_500).unwrap();
        for attempt in 2..=4 {
            assert_eq!(
                state.on_overflow(Some(measured), estimated, 8_000, "openai", "gpt"),
                ContextOverflowAction::CompactAndRetry { attempt }
            );
        }
        let ContextOverflowAction::Terminal(detail) =
            state.on_overflow(Some(measured), estimated, 8_000, "openai", "gpt")
        else {
            panic!("terminal");
        };
        let serialized = serde_json::to_string(&detail).unwrap();
        let debug = format!("{detail:?}");
        assert!(serialized.contains("measured") && serialized.contains("estimated"));
        assert!(serialized.contains("provider") && serialized.contains("model"));
        for secret in ["message", "content", "api_key", "authorization", "secret"] {
            assert!(!serialized.contains(secret));
            assert!(!debug.contains(secret));
        }
        assert_eq!(detail.attempt, 4);
        assert_eq!(
            detail.compactions_completed,
            CONTEXT_OVERFLOW_COMPACTIONS_MAX
        );
    }
}
