//! Deterministic clock implementation.

use crate::TestkitError;
use chrono::{Datelike, Duration};
use lotta_domain::{Clock, DomainError, Timestamp};
use std::sync::{Mutex, MutexGuard};

/// Thread-safe clock whose only time source is a caller-supplied timestamp.
#[derive(Debug)]
pub struct FakeClock {
    current: Mutex<Timestamp>,
}

impl FakeClock {
    /// Creates a clock at the exact supplied UTC timestamp.
    #[must_use]
    pub const fn new(initial: Timestamp) -> Self {
        Self {
            current: Mutex::new(initial),
        }
    }

    /// Advances by an explicit signed duration using checked timestamp arithmetic.
    ///
    /// The persisted timestamp range is years 0000 through 9999, inclusive.
    ///
    /// # Errors
    /// Returns [`TestkitError::ClockOverflow`] if the target timestamp is unrepresentable.
    pub fn advance(&self, amount: Duration) -> Result<Timestamp, TestkitError> {
        let mut current = self.lock();
        let next = current
            .checked_add(amount)
            .map_err(|_| TestkitError::ClockOverflow)?;
        if !(0..=9_999).contains(&next.as_utc().year()) {
            return Err(TestkitError::ClockOverflow);
        }
        *current = next;
        Ok(next)
    }

    fn lock(&self) -> MutexGuard<'_, Timestamp> {
        match self.current.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        *self.lock()
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
        let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(|_| {
            DomainError::InvalidTimestamp {
                value: value.into(),
            }
        })?;
        Timestamp::parse_persisted_rfc3339(&parsed.with_timezone(&chrono::Utc).to_rfc3339())
    }
}

/// Runs exact fake-clock advancement and atomic overflow assertions.
///
/// # Panics
/// Panics when the supplied fake clock violates deterministic clock semantics.
pub fn fake_clock_contract(factory: impl Fn(Timestamp) -> FakeClock) {
    let initial = timestamp("2025-02-03T04:05:06.123456789Z");
    let clock = factory(initial);
    assert_advance(&clock, 1, "2025-02-03T04:05:06.123456790Z");
    assert_advance(&clock, 999_999_999, "2025-02-03T04:05:07.123456789Z");
    assert_advance(&clock, -2_000_000_001, "2025-02-03T04:05:05.123456788Z");
    assert_overflow(
        &factory(timestamp("9999-12-31T23:59:59.999999999Z")),
        Duration::nanoseconds(1),
    );
    assert_overflow(
        &factory(timestamp("0000-01-01T00:00:00Z")),
        Duration::nanoseconds(-1),
    );
}

fn timestamp(value: &str) -> Timestamp {
    Timestamp::parse_persisted_rfc3339(value).expect("valid fake clock contract timestamp")
}

fn assert_advance(clock: &FakeClock, nanos: i64, expected: &str) {
    let advanced = clock
        .advance(Duration::nanoseconds(nanos))
        .expect("representable clock advance");
    assert_eq!(advanced.to_string(), expected);
    assert_eq!(clock.now(), advanced);
}

fn assert_overflow(clock: &FakeClock, amount: Duration) {
    let before = clock.now();
    assert!(matches!(
        clock.advance(amount),
        Err(TestkitError::ClockOverflow)
    ));
    assert_eq!(clock.now(), before);
}

#[cfg(test)]
mod tests {
    use super::{FakeClock, fake_clock_contract};

    #[test]
    fn deterministic_and_checked() {
        fake_clock_contract(FakeClock::new);
    }
}
