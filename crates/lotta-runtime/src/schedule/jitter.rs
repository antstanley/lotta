use chrono::{DateTime, Timelike, Utc};
use lotta_domain::Schedule;

const PERCENT_DENOMINATOR: u64 = 10;
const RECURRING_CAP_MS: u64 = 15 * 60 * 1_000;
const SCHEDULER_TICK_MS: u64 = 59_999;
const ONE_SHOT_EARLY_MS: u64 = 90 * 1_000;
const MINUTE_MS: u64 = 60 * 1_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

/// Injectable entropy source used only by schedule jitter.
pub trait JitterSource {
    /// Returns an unbiased source word.
    ///
    /// # Errors
    /// Returns a typed failure if secure entropy is unavailable.
    fn next_u64(&mut self) -> Result<u64, crate::RuntimeError>;
}

/// Computes schedule-only jitter from the schedule and exact fire instant.
///
/// Recurring schedules derive recurrence from cron and receive late jitter in
/// `[0, min(10% period, 15m, tick))`. One-shots on local minute `:00` or `:30`
/// receive early jitter in `(-90s, 0]`, clamped to avoid preceding creation.
/// Other one-shots receive zero.
///
/// # Errors
/// Returns a typed error for invalid cron, entropy failure, or arithmetic overflow.
pub fn compute_jitter(
    schedule: &Schedule,
    fire_time: DateTime<Utc>,
    source: &mut impl JitterSource,
) -> Result<i64, crate::RuntimeError> {
    if schedule.recurring {
        let period = estimate_period_ms(schedule.cron.as_str());
        if period == 0 {
            return Ok(0);
        }
        let maximum = (period / PERCENT_DENOMINATOR)
            .min(RECURRING_CAP_MS)
            .min(SCHEDULER_TICK_MS);
        return random_below(maximum, source).and_then(to_i64);
    }
    let local = fire_time.with_timezone(&schedule.timezone.as_tz());
    if local.minute() != 0 && local.minute() != 30 {
        return Ok(0);
    }
    let early = to_i64(random_below(ONE_SHOT_EARLY_MS, source)?)?;
    let candidate = fire_time
        .checked_sub_signed(chrono::Duration::milliseconds(early))
        .ok_or_else(|| crate::RuntimeError::LimitExceeded {
            context: "schedule jitter".into(),
        })?;
    Ok(if candidate < *schedule.created_at.as_utc() {
        0
    } else {
        -early
    })
}

pub(crate) fn estimate_period_ms(expression: &str) -> u64 {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    let [minute, hour, day, month, weekday] = fields.as_slice() else {
        return 0;
    };
    if let Some(step) = minute.strip_prefix("*/")
        && [*hour, *day, *month, *weekday] == ["*", "*", "*", "*"]
    {
        return step
            .parse::<u64>()
            .ok()
            .unwrap_or(0)
            .saturating_mul(MINUTE_MS);
    }
    if let Some(step) = hour.strip_prefix("*/")
        && !minute.starts_with('*')
        && [*day, *month, *weekday] == ["*", "*", "*"]
    {
        return step
            .parse::<u64>()
            .ok()
            .unwrap_or(0)
            .saturating_mul(HOUR_MS);
    }
    if !minute.contains('*') && !hour.contains('*') && [*day, *month, *weekday] == ["*", "*", "*"] {
        return DAY_MS;
    }
    0
}

fn random_below(upper: u64, source: &mut impl JitterSource) -> Result<u64, crate::RuntimeError> {
    if upper <= 1 {
        return Ok(0);
    }
    let rejection = upper.wrapping_neg() % upper;
    loop {
        let word = source.next_u64()?;
        if word >= rejection {
            return Ok(word % upper);
        }
    }
}

fn to_i64(value: u64) -> Result<i64, crate::RuntimeError> {
    i64::try_from(value).map_err(|_| crate::RuntimeError::LimitExceeded {
        context: "schedule jitter".into(),
    })
}
