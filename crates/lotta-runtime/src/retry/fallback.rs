use super::policy::{PROVIDER_FALLBACK_RETRY_ATTEMPT, ProviderFailure, ProviderFailureKind};
use crate::RuntimeError;
use lotta_domain::ModelDescriptor;

/// Maximum UTF-8 bytes retained for one retry event reason.
pub const RETRY_EVENT_REASON_BYTES_MAX: usize = 1_024;

/// Immutable transport/provider target for one attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRoute {
    /// Stable transport identifier.
    pub transport: String,
    /// Stable provider identifier.
    pub provider: String,
}

impl ProviderRoute {
    /// Creates a route from stable transport and provider identifiers.
    #[must_use]
    pub fn new(transport: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            transport: transport.into(),
            provider: provider.into(),
        }
    }
}

/// Explicit immutable one-way fallback route.
#[derive(Clone, Debug, PartialEq)]
pub struct FallbackRoute {
    source: ProviderRoute,
    destination: ProviderRoute,
    destination_model: ModelDescriptor,
}

impl FallbackRoute {
    /// Validates a non-cyclic one-way route.
    ///
    /// # Errors
    /// Rejects an identical source and destination.
    pub fn new(
        source: ProviderRoute,
        destination: ProviderRoute,
        destination_model: ModelDescriptor,
    ) -> Result<Self, RuntimeError> {
        if source == destination {
            return Err(RuntimeError::InvalidData {
                context: "provider fallback cycle".into(),
            });
        }
        Ok(Self {
            source,
            destination,
            destination_model,
        })
    }

    /// Returns the required initial route.
    #[must_use]
    pub const fn source(&self) -> &ProviderRoute {
        &self.source
    }

    /// Returns the validated fallback destination route.
    #[must_use]
    pub const fn destination(&self) -> &ProviderRoute {
        &self.destination
    }

    /// Returns the request-scoped fallback model.
    #[must_use]
    pub const fn destination_model(&self) -> &ModelDescriptor {
        &self.destination_model
    }

    pub(crate) fn eligible(failure: &ProviderFailure, retry_attempt: u32) -> bool {
        retry_attempt >= PROVIDER_FALLBACK_RETRY_ATTEMPT
            && matches!(
                failure.kind,
                ProviderFailureKind::Transient | ProviderFailureKind::Busy
            )
    }
}

/// Retry notice reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryReason {
    /// A central retry against the same route.
    ProviderRetry,
    /// An explicit transport/provider switch.
    TransportFallback,
}

/// Bounded observable retry notice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetryEvent {
    /// Exact source route.
    pub source: ProviderRoute,
    /// Exact destination route; equal to source for ordinary retries.
    pub destination: ProviderRoute,
    /// Stable retry reason discriminant.
    pub reason: RetryReason,
    /// One-based retry attempt after the initial provider call.
    pub attempt: u32,
    /// Selected wait before the next attempt.
    pub delay_ms: u64,
    /// Scrubbed bounded provider reason.
    pub detail: String,
}

impl RetryEvent {
    pub(crate) fn new(
        source: ProviderRoute,
        destination: ProviderRoute,
        reason: RetryReason,
        attempt: u32,
        delay_ms: u64,
        detail: &str,
    ) -> Self {
        Self {
            source,
            destination,
            reason,
            attempt,
            delay_ms,
            detail: bounded_detail(detail),
        }
    }
}

fn bounded_detail(detail: &str) -> String {
    if detail.len() <= RETRY_EVENT_REASON_BYTES_MAX {
        return detail.to_owned();
    }
    let mut end = RETRY_EVENT_REASON_BYTES_MAX;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_owned()
}

/// Awaitable retry-event effect boundary.
///
/// Failure is terminal: execution does not sleep, retry, or switch route after a notice is lost.
pub trait EventSink: Send + Sync {
    /// Emits one retry notice before the associated retry/fallback action.
    ///
    /// # Errors
    /// Returns a typed effect failure; the executor stops immediately.
    fn emit(&self, event: RetryEvent) -> crate::ports::PortFuture<'_, ()>;
}
