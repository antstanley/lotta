use super::*;
#[test]
fn retry_after_precedence() {
    let policy = RetryPolicy::default();
    let failure = ProviderFailure::new(ProviderFailureKind::Transient, "x")
        .with_retry_after(RetryAfter::Milliseconds(7));
    assert_eq!(policy.delay_ms(&failure, 3, 0), Some(7));
    let dated = ProviderFailure::new(ProviderFailureKind::Busy, "x")
        .with_retry_after(RetryAfter::DateMilliseconds(1_007));
    assert_eq!(policy.delay_ms(&dated, 1, 1_000), Some(7));
    assert_eq!(policy.delay_ms(&dated, 1, 2_000), Some(0));
}

#[test]
fn transient_exponential() {
    let policy = RetryPolicy::default();
    let failure = ProviderFailure::new(ProviderFailureKind::Transient, "x");
    assert_eq!(
        (1..=3)
            .map(|attempt| policy.delay_ms(&failure, attempt, 0).unwrap())
            .collect::<Vec<_>>(),
        [1_000, 2_000, 4_000]
    );
}

#[test]
fn busy_exponential_and_capped() {
    let policy = RetryPolicy {
        retries_max: u32::MAX,
        ..RetryPolicy::default()
    };
    let failure = ProviderFailure::new(ProviderFailureKind::Busy, "x");
    assert_eq!(policy.delay_ms(&failure, 1, 0), Some(1_000));
    assert_eq!(
        policy.delay_ms(&failure, 63, 0),
        Some(PROVIDER_BACKOFF_MS_MAX)
    );
}

#[test]
fn empty_linear() {
    let policy = RetryPolicy::default();
    let failure = ProviderFailure::new(ProviderFailureKind::Empty, "x");
    assert_eq!(
        (1..=2)
            .map(|attempt| policy.delay_ms(&failure, attempt, 0).unwrap())
            .collect::<Vec<_>>(),
        [500, 1_000]
    );
    assert_eq!(policy.delay_ms(&failure, 3, 0), Some(1_500));
}
