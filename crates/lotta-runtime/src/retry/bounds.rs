use super::test_support::*;
use super::*;
use crate::ports::ProviderEvent;

#[test]
fn canonical_values_and_attempt_meaning() {
    assert_eq!(PROVIDER_RETRIES_MAX, 3);
    assert_eq!(PROVIDER_BACKOFF_MS_MAX, 60_000);
    assert_eq!(RetryPolicy::default().attempts_max(), 4);
}

#[test]
fn retry_after_zero_huge_and_cap_edges() {
    let policy = RetryPolicy::default();
    for (value, expected) in [
        (0, 0),
        (PROVIDER_BACKOFF_MS_MAX - 1, PROVIDER_BACKOFF_MS_MAX - 1),
        (PROVIDER_BACKOFF_MS_MAX, PROVIDER_BACKOFF_MS_MAX),
        (PROVIDER_BACKOFF_MS_MAX + 1, PROVIDER_BACKOFF_MS_MAX),
        (u64::MAX, PROVIDER_BACKOFF_MS_MAX),
    ] {
        let failure = ProviderFailure::new(ProviderFailureKind::Busy, "x")
            .with_retry_after(RetryAfter::Milliseconds(value));
        assert_eq!(policy.delay_ms(&failure, 1, 0), Some(expected));
    }
}

#[tokio::test]
async fn deadline_below_at_and_above_delay() {
    for (deadline_ms, succeeds) in [(999, false), (1_000, false), (1_001, true)] {
        let time = FakeTime::default();
        let events = Events::default();
        let port = ScriptedPort::new(vec![
            vec![failure(ProviderFailureKind::Transient)],
            vec![
                ProviderEvent::TextDelta {
                    text: crate::boundary::ProviderEventText::new("done".into()).unwrap(),
                },
                stop(),
            ],
        ]);
        let result = run(
            RetryPolicy {
                retries_max: 1,
                deadline_ms,
                ..RetryPolicy::default()
            },
            &time,
            &events,
            &port,
        )
        .await;
        assert_eq!(result.is_ok(), succeeds);
        assert_eq!(port.calls(), if succeeds { 2 } else { 1 });
    }
}
