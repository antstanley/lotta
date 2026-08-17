//! Runtime-owned provider retry and explicit fallback execution.

mod executor;
/// Explicit immutable provider routes and bounded retry notices.
pub mod fallback;
/// Deterministic retry classification, delays, and deadline arithmetic.
pub mod policy;

pub use executor::{RETRY_EVENT_CHANNEL_CAPACITY, RetryExecutor, RetryTerminal};
pub use fallback::{EventSink, FallbackRoute, ProviderRoute, RetryEvent, RetryReason};
pub use policy::{
    Clock, PROVIDER_BACKOFF_MS_MAX, PROVIDER_BUSY_BACKOFF_MS_BASE,
    PROVIDER_EMPTY_BACKOFF_MS_LINEAR, PROVIDER_FALLBACK_RETRY_ATTEMPT, PROVIDER_RETRIES_MAX,
    PROVIDER_RETRY_DEADLINE_MS_DEFAULT, ProviderFailure, ProviderFailureKind, RetryAfter,
    RetryPolicy, Sleeper, SystemClock, TokioSleeper,
};

#[cfg(test)]
#[path = "retry/bounds.rs"]
mod bounds;
#[cfg(test)]
#[path = "retry/delay_shapes.rs"]
mod delay_shapes;
#[cfg(test)]
#[path = "retry/deterministic_delays.rs"]
mod deterministic_delays;
#[cfg(test)]
#[path = "retry/executor_behavior.rs"]
mod executor_behavior;
#[cfg(test)]
#[path = "retry/fallback_tests.rs"]
mod fallback_tests;
#[cfg(test)]
#[path = "retry/non_retryable.rs"]
mod non_retryable;
#[cfg(test)]
#[path = "retry/test_support.rs"]
mod test_support;
