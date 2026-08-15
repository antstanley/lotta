//! Injected-clock WebSocket heartbeat state machine.

use lotta_domain::{Clock, Timestamp};

use crate::bounds::WS_PONG_TIMEOUT_MS;

/// Pure heartbeat state derived only from an injected clock.
pub struct Heartbeat {
    last_pong: Timestamp,
}

impl Heartbeat {
    /// Seeds a newly accepted socket with its acceptance time.
    #[must_use]
    pub fn new(clock: &dyn Clock) -> Self {
        Self {
            last_pong: clock.now(),
        }
    }

    /// Records the most recent pong through the same clock boundary.
    pub fn record_pong(&mut self, clock: &dyn Clock) {
        self.last_pong = clock.now();
    }

    /// Returns true only strictly after the canonical timeout.
    #[must_use]
    pub fn is_expired(&self, clock: &dyn Clock) -> bool {
        let elapsed = clock
            .now()
            .as_utc()
            .signed_duration_since(*self.last_pong.as_utc());
        elapsed.num_milliseconds() > WS_PONG_TIMEOUT_MS.cast_signed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use lotta_domain::Timestamp;
    use lotta_testkit::clock::FakeClock;

    fn clock() -> FakeClock {
        let time =
            Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z").expect("fixed timestamp");
        FakeClock::new(time)
    }

    #[test]
    fn stays_open_before_timeout() {
        let clock = clock();
        let heartbeat = Heartbeat::new(&clock);
        clock
            .advance(Duration::milliseconds(89_999))
            .expect("advance");
        assert!(!heartbeat.is_expired(&clock));
    }

    #[test]
    fn stays_open_at_timeout() {
        let clock = clock();
        let heartbeat = Heartbeat::new(&clock);
        clock
            .advance(Duration::milliseconds(90_000))
            .expect("advance");
        assert!(!heartbeat.is_expired(&clock));
    }

    #[test]
    fn closes_after_pong_timeout() {
        let clock = clock();
        let heartbeat = Heartbeat::new(&clock);
        clock
            .advance(Duration::milliseconds(90_001))
            .expect("advance");
        assert!(heartbeat.is_expired(&clock));
    }

    #[test]
    fn pong_refreshes_deadline() {
        let clock = clock();
        let mut heartbeat = Heartbeat::new(&clock);
        clock
            .advance(Duration::milliseconds(80_000))
            .expect("advance");
        heartbeat.record_pong(&clock);
        clock
            .advance(Duration::milliseconds(80_000))
            .expect("advance");
        assert!(!heartbeat.is_expired(&clock));
    }

    #[test]
    fn accepted_socket_is_seeded_open() {
        let clock = clock();
        assert!(!Heartbeat::new(&clock).is_expired(&clock));
    }
}
