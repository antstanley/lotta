use super::provider_contract::{ProviderError, ProviderMetadata, StopReason, ToolCallId};
use crate::boundary::{ProviderEventText, ToolArgumentChunk};

/// Monotonic provider token-accounting snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderUsage {
    /// Input tokens consumed so far.
    pub input_tokens: u64,
    /// Output tokens generated so far.
    pub output_tokens: u64,
    /// Cached input tokens reported so far.
    pub cached_input_tokens: u64,
    /// Reasoning tokens reported so far.
    pub reasoning_tokens: u64,
}

impl ProviderUsage {
    /// Reports whether this snapshot is component-wise no earlier than `previous`.
    #[must_use]
    pub const fn is_monotonic_after(self, previous: Self) -> bool {
        self.input_tokens >= previous.input_tokens
            && self.output_tokens >= previous.output_tokens
            && self.cached_input_tokens >= previous.cached_input_tokens
            && self.reasoning_tokens >= previous.reasoning_tokens
    }

    /// Returns the checked total reported token count.
    /// # Errors
    /// Rejects arithmetic overflow or component counts impossible relative to input/output totals.
    pub fn checked_total(self) -> Result<u64, crate::RuntimeError> {
        if self.cached_input_tokens > self.input_tokens
            || self.reasoning_tokens > self.output_tokens
        {
            return Err(crate::RuntimeError::InvalidData {
                context: "provider usage impossible total".into(),
            });
        }
        self.input_tokens.checked_add(self.output_tokens).ok_or(
            crate::RuntimeError::LimitExceeded {
                context: "provider usage total".into(),
            },
        )
    }
}

/// Normalized bounded event emitted by every provider adapter.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    /// Visible assistant text in provider arrival order.
    TextDelta {
        /// Bounded delta text.
        text: ProviderEventText,
    },
    /// Visible reasoning text in provider arrival order.
    ReasoningDelta {
        /// Bounded reasoning delta.
        text: ProviderEventText,
    },
    /// Provider-declared opaque reasoning that cannot be exposed.
    RedactedReasoning {
        /// Bounded non-secret marker retained in place of reasoning.
        marker: ProviderEventText,
    },
    /// Begins one tool call with its stable provider call identifier.
    ToolCallStart {
        /// Stable call identifier.
        call_id: ToolCallId,
        /// Bounded model-facing tool name.
        name: ProviderEventText,
    },
    /// Appends an unparsed bounded argument fragment for one tool call.
    ToolCallArgumentsDelta {
        /// Stable call identifier.
        call_id: ToolCallId,
        /// Unparsed bounded argument bytes; UTF-8 and JSON tokens may split across events.
        chunk: ToolArgumentChunk,
    },
    /// Ends one tool call; the runtime validates its complete argument JSON now.
    ToolCallEnd {
        /// Stable call identifier.
        call_id: ToolCallId,
    },
    /// Monotonic cumulative token usage.
    Usage {
        /// Cumulative snapshot.
        usage: ProviderUsage,
    },
    /// Sanitized continuation metadata suitable for persistence.
    ProviderMetadata {
        /// Sanitized metadata.
        metadata: ProviderMetadata,
    },
    /// The sole terminal stop event for a successful provider stream.
    Stop {
        /// Normalized stop semantics.
        reason: StopReason,
    },
    /// A normalized terminal provider error.
    Error {
        /// Stable provider failure.
        error: ProviderError,
    },
}
