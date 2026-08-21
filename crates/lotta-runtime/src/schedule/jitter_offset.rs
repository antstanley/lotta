//! Jitter tests mirroring the pinned baseline `computeJitter` rules.

use super::compute_jitter;
use crate::schedule::support::{Fixed, instant, schedule};
use chrono::{TimeZone, Utc};
use std::collections::VecDeque;

#[test]
fn recurring_is_late_and_under_tick() {
    let schedule = schedule(true, "*/5 * * * *", "UTC", instant(0));
    let mut source = Fixed::one(29_999);
    let value = compute_jitter(&schedule, instant(300), &mut source).expect("jitter");
    assert!((0..30_000).contains(&value));
}

#[test]
fn exact_tick_maximum_is_59_998() {
    let schedule = schedule(true, "30 14 * * *", "UTC", instant(0));
    let mut source = Fixed::one(59_998);
    assert_eq!(
        compute_jitter(&schedule, instant(300), &mut source).unwrap(),
        59_998
    );
    // The tick endpoint itself is unattainable: modulo folds 59_999 back to zero.
    let mut source = Fixed::one(59_999);
    assert_eq!(
        compute_jitter(&schedule, instant(300), &mut source).unwrap(),
        0
    );
}

#[test]
fn complex_recurring_patterns_yield_zero_jitter() {
    // Baseline parity: `estimatePeriodMs` returns 0 for these, so no late jitter.
    for expression in [
        "1,5,9 * * * *",
        "1-21/4 * * * *",
        "0 0 1 * *",
        "0 0 * 1 *",
        "0 0 * * 1-5",
        "0 0 */3 * *",
    ] {
        let schedule = schedule(true, expression, "UTC", instant(0));
        let mut source = Fixed::one(u64::MAX);
        assert_eq!(
            compute_jitter(&schedule, instant(300), &mut source).expect("jitter"),
            0,
            "{expression}"
        );
    }
}

#[test]
fn pinned_period_estimator_rejects_complex_patterns() {
    assert_eq!(super::jitter::estimate_period_ms("*/5 * * * *"), 300_000);
    assert_eq!(super::jitter::estimate_period_ms("0 */2 * * *"), 7_200_000);
    assert_eq!(super::jitter::estimate_period_ms("30 14 * * *"), 86_400_000);
    for expression in [
        "1,5,9 * * * *",
        "1-21/4 * * * *",
        "0 0 1 * *",
        "0 0 * 1 *",
        "0 0 * * 1-5",
        "0 0 */3 * *",
    ] {
        assert_eq!(
            super::jitter::estimate_period_ms(expression),
            0,
            "{expression}"
        );
    }
}

#[test]
fn one_shot_jitter_reads_the_schedule_timezone_wall_clock() {
    // 00:30 UTC is 06:00 in Asia/Kolkata: local minute :00 earns early jitter.
    let kolkata = schedule(false, "30 6 * * *", "Asia/Kolkata", instant(0));
    let fire = Utc
        .with_ymd_and_hms(2025, 1, 1, 0, 30, 0)
        .single()
        .expect("fire");
    let mut source = Fixed::one(89_999);
    assert_eq!(
        compute_jitter(&kolkata, fire, &mut source).expect("jitter"),
        -89_999
    );
    // The same UTC minute is 06:15 in Asia/Kathmandu: local minute :15 earns none.
    let kathmandu = schedule(false, "15 6 * * *", "Asia/Kathmandu", instant(0));
    let mut source = Fixed::one(89_999);
    assert_eq!(
        compute_jitter(&kathmandu, fire, &mut source).expect("jitter"),
        0
    );
}

#[test]
fn one_shot_uses_arbitrary_schedule_timezone_minute() {
    let schedule = schedule(false, "0 0 * * *", "America/New_York", instant(0));
    let fire = Utc.with_ymd_and_hms(2025, 1, 1, 5, 30, 0).single().unwrap();
    let mut source = Fixed::one(89_999);
    assert_eq!(
        compute_jitter(&schedule, fire, &mut source).expect("jitter"),
        -89_999
    );
}

#[test]
fn one_shot_clamps_before_creation() {
    let schedule = schedule(false, "0 0 * * *", "UTC", instant(100));
    let mut source = Fixed::one(89_999);
    assert_eq!(
        compute_jitter(&schedule, instant(120), &mut source).expect("jitter"),
        0
    );
}

#[test]
fn other_one_shot_is_zero() {
    let schedule = schedule(false, "0 0 * * *", "UTC", instant(0));
    let mut source = Fixed::one(89_999);
    assert_eq!(
        compute_jitter(&schedule, instant(17 * 60), &mut source).expect("jitter"),
        0
    );
}

#[test]
fn rejection_sampling_discards_biased_prefix() {
    let schedule = schedule(true, "*/5 * * * *", "UTC", instant(0));
    let upper = 30_001_u64;
    let rejection = upper.wrapping_neg() % upper;
    let mut source = Fixed(VecDeque::from([rejection - 1, rejection + 12_345]));
    assert_eq!(
        compute_jitter(&schedule, instant(300), &mut source).unwrap(),
        i64::try_from((rejection + 12_345) % upper).unwrap()
    );
}
