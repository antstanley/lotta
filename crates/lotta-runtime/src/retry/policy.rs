use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

/// Maximum transient/busy retries after the initial provider attempt.
pub const PROVIDER_RETRIES_MAX: u32 = 3;
/// Maximum empty-response retries; total empty attempts are exactly three.
pub const EMPTY_RESPONSE_RETRIES_MAX: u32 = 2;
/// Maximum delay selected for any provider retry.
pub const PROVIDER_BACKOFF_MS_MAX: u64 = 60_000;
/// Exponential transient/busy delay base from the pinned provider path.
pub const PROVIDER_BUSY_BACKOFF_MS_BASE: u64 = 1_000;
/// Linear empty-response delay increment from the pinned provider path.
pub const PROVIDER_EMPTY_BACKOFF_MS_LINEAR: u64 = 500;
/// Default whole-execution deadline used when no tighter request deadline exists.
pub const PROVIDER_RETRY_DEADLINE_MS_DEFAULT: u64 = 600_000;
/// Retry number at which an eligible configured fallback is applied.
pub const PROVIDER_FALLBACK_RETRY_ATTEMPT: u32 = 2;
/// Largest safe left shift used by saturating exponential delay arithmetic.
const PROVIDER_BACKOFF_SHIFT_MAX: u32 = 63;

/// Canonical runtime retry category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderFailureKind {
    /// Temporary transport/provider failure.
    Transient,
    /// Rate limiting or temporary provider overload.
    Busy,
    /// Successful transport with no usable model content.
    Empty,
    /// Authentication or authorization failure.
    Auth,
    /// Invalid provider request.
    Invalid,
    /// Unsupported provider or model.
    Unsupported,
    /// Response schema/protocol failure.
    Schema,
    /// Other terminal failure.
    Terminal,
}

/// Retry delay supplied by the provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryAfter {
    /// Exact nonnegative duration supplied in milliseconds.
    Milliseconds(u64),
    /// Absolute Unix epoch deadline in milliseconds.
    DateMilliseconds(u64),
}

impl RetryAfter {
    pub(crate) fn delay_ms(self, wall_now_ms: u64) -> u64 {
        match self {
            Self::Milliseconds(value) => value,
            Self::DateMilliseconds(value) => value.saturating_sub(wall_now_ms),
        }
    }
}

/// One normalized terminal outcome from one adapter attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderFailure {
    /// Stable retry category.
    pub kind: ProviderFailureKind,
    /// Scrubbed stable reason for diagnostics and retry events.
    pub reason: String,
    /// Optional provider-directed retry timing.
    pub retry_after: Option<RetryAfter>,
}

impl ProviderFailure {
    /// Creates a failure without provider-directed timing.
    #[must_use]
    pub fn new(kind: ProviderFailureKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
            retry_after: None,
        }
    }

    /// Attaches exact provider-directed timing.
    #[must_use]
    pub const fn with_retry_after(mut self, retry_after: RetryAfter) -> Self {
        self.retry_after = Some(retry_after);
        self
    }

    /// Reports whether central policy may retry this category.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self.kind,
            ProviderFailureKind::Transient | ProviderFailureKind::Busy | ProviderFailureKind::Empty
        )
    }
}

/// Monotonic and wall-clock source used by retry timing.
pub trait Clock: Send + Sync {
    /// Monotonic milliseconds since an arbitrary stable origin.
    fn monotonic_ms(&self) -> u64;
    /// Unix epoch milliseconds used only for HTTP-date retry-after semantics.
    fn unix_epoch_ms(&self) -> u64;
}

/// Cancellation-aware sleep boundary.
pub trait Sleeper: Send + Sync {
    /// Sleeps for exactly `duration` unless cancellation wins.
    fn sleep<'a>(
        &'a self,
        duration: Duration,
        cancellation: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::RuntimeError>> + Send + 'a>>;
}

/// Production monotonic and wall clock.
#[derive(Clone, Copy, Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn monotonic_ms(&self) -> u64 {
        duration_ms(self.origin.elapsed())
    }

    fn unix_epoch_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, duration_ms)
    }
}

/// Production Tokio cancellation-aware sleeper.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep<'a>(
        &'a self,
        duration: Duration,
        cancellation: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => Err(crate::RuntimeError::Cancelled {
                    context: "provider retry sleep".into(),
                }),
                () = tokio::time::sleep(duration) => Ok(()),
            }
        })
    }
}

fn duration_ms(value: Duration) -> u64 {
    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
}

/// Immutable deterministic retry policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    /// Retries after the initial attempt.
    pub retries_max: u32,
    /// Maximum selected delay in milliseconds.
    pub backoff_ms_max: u64,
    /// Whole-execution deadline in milliseconds.
    pub deadline_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            retries_max: PROVIDER_RETRIES_MAX,
            backoff_ms_max: PROVIDER_BACKOFF_MS_MAX,
            deadline_ms: PROVIDER_RETRY_DEADLINE_MS_DEFAULT,
        }
    }
}

impl RetryPolicy {
    /// Returns total adapter attempts, including the initial attempt.
    #[must_use]
    pub const fn attempts_max(self) -> u32 {
        self.retries_max.saturating_add(1)
    }

    /// Computes a retry delay. `retry_attempt` starts at one after the initial failure.
    #[must_use]
    pub fn delay_ms(
        self,
        failure: &ProviderFailure,
        retry_attempt: u32,
        wall_now_ms: u64,
    ) -> Option<u64> {
        if !failure.is_retryable() || retry_attempt == 0 {
            return None;
        }
        let selected = failure.retry_after.map_or_else(
            || shaped_delay_ms(failure.kind, retry_attempt),
            |value| value.delay_ms(wall_now_ms),
        );
        Some(selected.min(self.backoff_ms_max))
    }
}

fn shaped_delay_ms(kind: ProviderFailureKind, retry_attempt: u32) -> u64 {
    match kind {
        ProviderFailureKind::Transient | ProviderFailureKind::Busy => {
            let shift = retry_attempt
                .saturating_sub(1)
                .min(PROVIDER_BACKOFF_SHIFT_MAX);
            PROVIDER_BUSY_BACKOFF_MS_BASE.saturating_mul(1_u64 << shift)
        }
        ProviderFailureKind::Empty => {
            PROVIDER_EMPTY_BACKOFF_MS_LINEAR.saturating_mul(u64::from(retry_attempt))
        }
        _ => 0,
    }
}
