use crate::ports::ProviderError;
use crate::retry::{ProviderFailure, ProviderFailureKind};
use lotta_domain::{NonEmptyString, RunId};
use serde::{Deserialize, Serialize};

/// Exact stable terminal reason retained for unsuccessful provider turns.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStopReason {
    /// Provider completed without usable model output.
    EmptyResponse,
    /// Request exceeded the provider context capacity.
    ContextOverflow,
    /// Provider transport or another non-quota provider operation failed.
    TransportFailure,
    /// Account quota was exhausted.
    ProviderQuotaError,
    /// User cancellation interrupted the current lease.
    UserCancellation,
}

/// Bounded, scrubbed provider failure detail safe for durable storage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderFailureDetail {
    /// Stable normalized failure kind.
    pub kind: NonEmptyString,
    /// Optional stable machine-readable provider code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<NonEmptyString>,
    /// Number of attempts represented by this terminal failure.
    pub attempt_count: u32,
    /// Number of context compactions completed before failure.
    pub compaction_count: u8,
}

/// Durable terminal failure record correlated to one admitted input and run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TurnStopRecord {
    /// Stable turn identifier.
    pub turn_id: NonEmptyString,
    /// Stable run identifier.
    pub run_id: RunId,
    /// Stable admitted input identifier.
    pub input_id: NonEmptyString,
    /// Exact typed terminal reason.
    pub reason: TurnStopReason,
    /// Optional scrubbed provider detail containing kind, code, and counts only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_failure: Option<ProviderFailureDetail>,
}

impl TurnStopReason {
    /// Maps a normalized provider error without collapsing rate limiting into quota.
    #[must_use]
    pub const fn from_provider(error: &ProviderError) -> Self {
        match error {
            ProviderError::Quota(_) => Self::ProviderQuotaError,
            ProviderError::ContextOverflow(_) => Self::ContextOverflow,
            ProviderError::Cancelled(_) => Self::UserCancellation,
            ProviderError::Authentication(_)
            | ProviderError::Authorization(_)
            | ProviderError::InvalidRequest(_)
            | ProviderError::RateLimit(_)
            | ProviderError::Timeout(_)
            | ProviderError::Overloaded(_)
            | ProviderError::Unavailable(_)
            | ProviderError::Protocol(_)
            | ProviderError::Unknown(_) => Self::TransportFailure,
        }
    }

    pub(crate) const fn from_failure(failure: &ProviderFailure) -> Self {
        match failure.kind {
            ProviderFailureKind::Empty => Self::EmptyResponse,
            ProviderFailureKind::ContextOverflow => Self::ContextOverflow,
            ProviderFailureKind::Quota => Self::ProviderQuotaError,
            ProviderFailureKind::Cancelled => Self::UserCancellation,
            ProviderFailureKind::Transient
            | ProviderFailureKind::Busy
            | ProviderFailureKind::Auth
            | ProviderFailureKind::Invalid
            | ProviderFailureKind::Unsupported
            | ProviderFailureKind::Schema
            | ProviderFailureKind::Terminal => Self::TransportFailure,
        }
    }

    /// Returns the exact stable client and lifecycle wire value.
    #[must_use]
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::EmptyResponse => "empty_response",
            Self::ContextOverflow => "context_overflow",
            Self::TransportFailure => "transport_failure",
            Self::ProviderQuotaError => "provider_quota_error",
            Self::UserCancellation => "user_cancellation",
        }
    }
}

impl ProviderFailureDetail {
    pub(crate) fn from_failure(
        failure: &ProviderFailure,
        attempt_count: u32,
        compaction_count: u8,
    ) -> Self {
        Self {
            kind: bounded_kind(failure.kind),
            code: scrubbed_code(&failure.reason),
            attempt_count,
            compaction_count,
        }
    }
}

fn bounded_kind(kind: ProviderFailureKind) -> NonEmptyString {
    let wire = match kind {
        ProviderFailureKind::Transient => "transient",
        ProviderFailureKind::Busy => "busy",
        ProviderFailureKind::Empty => "empty",
        ProviderFailureKind::Auth => "auth",
        ProviderFailureKind::Invalid => "invalid",
        ProviderFailureKind::Unsupported => "unsupported",
        ProviderFailureKind::Schema => "schema",
        ProviderFailureKind::ContextOverflow => "context_overflow",
        ProviderFailureKind::Quota => "quota",
        ProviderFailureKind::Cancelled => "cancelled",
        ProviderFailureKind::Terminal => "terminal",
    };
    NonEmptyString::new(wire).expect("static provider failure kind is valid")
}

fn scrubbed_code(reason: &str) -> Option<NonEmptyString> {
    let code = reason
        .split_once(':')
        .map_or(reason, |(code, _)| code)
        .trim();
    (!code.is_empty() && code.len() <= 128)
        .then(|| NonEmptyString::new(code.to_owned()).ok())
        .flatten()
}
