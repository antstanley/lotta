use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use std::str::FromStr;

use crate::RuntimeError;

const CRON_FIELDS: usize = 5;
const MINUTE_SECONDS: i64 = 60;
const SEARCH_MINUTES_MAX: usize = 366 * 24 * 60 * 5;

/// A parsed baseline interval and its canonical five-field cron expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedInterval {
    /// Canonical cron expression.
    pub cron: String,
    /// Baseline rounding or clamping note.
    pub note: Option<String>,
}

/// Validated classic five-field cron expression backed by the mature `cron` crate.
#[derive(Clone, Debug)]
pub struct ScheduleExpression {
    schedule: Schedule,
}

impl ScheduleExpression {
    /// Parses the pinned baseline five-field cron dialect.
    ///
    /// # Errors
    /// Rejects extensions, out-of-range values, reversed ranges, zero steps,
    /// bare value steps, and expressions the authoritative crate rejects.
    pub fn parse(expression: &str) -> Result<Self, RuntimeError> {
        let trimmed = expression.trim();
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() != CRON_FIELDS || has_bare_value_step(&fields) {
            return Err(invalid("cron expression"));
        }
        validate_supported(trimmed)?;
        let normalized = normalize_one_based_steps(&fields)?;
        let normalized = normalize_sunday_seven(&normalized);
        let seconds = format!("0 {normalized}");
        let schedule = Schedule::from_str(&seconds).map_err(|_| invalid("cron expression"))?;
        Ok(Self { schedule })
    }

    /// Resolves the first matching UTC instant strictly after `after` in `timezone`.
    ///
    /// Nonexistent spring-forward wall minutes are skipped. Both UTC instants of an
    /// ambiguous fall-back wall minute match, preserving the pinned baseline behavior.
    ///
    /// # Errors
    /// Returns a typed error if no match is found within the bounded search horizon.
    pub fn next_after(
        &self,
        after: DateTime<Utc>,
        timezone: Tz,
    ) -> Result<DateTime<Utc>, RuntimeError> {
        let mut candidate = after + Duration::seconds(1);
        for _ in 0..SEARCH_MINUTES_MAX {
            let local = candidate.with_timezone(&timezone);
            let wall_utc = local.naive_local().and_utc();
            let previous = wall_utc - Duration::nanoseconds(1);
            if self
                .schedule
                .after(&previous)
                .next()
                .is_some_and(|next| next == wall_utc)
            {
                return Ok(candidate);
            }
            candidate = next_minute(candidate)?;
        }
        Err(invalid("cron search horizon"))
    }
}

/// Parses the exact baseline `--every` interval forms into five-field cron.
#[must_use]
pub fn parse_interval(input: &str) -> Option<ParsedInterval> {
    let trimmed = input.trim();
    let split = trimmed.find(|character: char| !character.is_ascii_digit())?;
    let value = trimmed[..split].parse::<u64>().ok()?;
    let unit = trimmed[split..].trim().to_ascii_lowercase();
    if value == 0 || !valid_unit(&unit) {
        return None;
    }
    match unit.as_bytes().first().copied()? {
        b's' => Some(seconds_interval(value)),
        b'm' => Some(minute_interval(value)),
        b'h' => Some(hour_interval(value)),
        b'd' => day_interval(value),
        _ => None,
    }
}

fn seconds_interval(value: u64) -> ParsedInterval {
    if value < 60 {
        return parsed(
            "*/1 * * * *",
            Some(format!(
                "Rounded {value}s up to 1m (minimum granularity is 1 minute)"
            )),
        );
    }
    minute_interval((value + 30) / 60)
}

fn minute_interval(value: u64) -> ParsedInterval {
    if value >= 60 {
        return parsed(&format!("0 */{} * * *", ((value + 30) / 60).max(1)), None);
    }
    if 60 % value == 0 {
        return parsed(&format!("*/{value} * * * *"), None);
    }
    let closest = nearest(value, &[1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30, 60]);
    parsed(
        &format!("*/{closest} * * * *"),
        Some(format!(
            "{value}m rounded to every {closest}m (nearest clean divisor of 60)"
        )),
    )
}

fn hour_interval(value: u64) -> ParsedInterval {
    if value >= 24 {
        return parsed("0 0 * * *", Some(format!("{value}h clamped to daily")));
    }
    if 24 % value == 0 {
        return parsed(&format!("0 */{value} * * *"), None);
    }
    let closest = nearest(value, &[1, 2, 3, 4, 6, 8, 12, 24]);
    parsed(
        &format!("0 */{closest} * * *"),
        Some(format!(
            "{value}h rounded to every {closest}h (nearest clean divisor of 24)"
        )),
    )
}

fn day_interval(value: u64) -> Option<ParsedInterval> {
    let expression = if value == 1 {
        "0 0 * * *".into()
    } else {
        format!("0 0 */{value} * *")
    };
    ScheduleExpression::parse(&expression)
        .ok()
        .map(|_| parsed(&expression, None))
}

fn normalize_one_based_steps(fields: &[&str]) -> Result<String, RuntimeError> {
    let mut output = fields.iter().map(ToString::to_string).collect::<Vec<_>>();
    output[2] = normalize_field(fields[2], 1, 31)?;
    output[3] = normalize_field(fields[3], 1, 12)?;
    Ok(output.join(" "))
}

fn normalize_field(field: &str, minimum: u8, maximum: u8) -> Result<String, RuntimeError> {
    let mut values = Vec::new();
    for part in field.split(',') {
        let Some(step) = part.strip_prefix("*/") else {
            values.push(part.to_owned());
            continue;
        };
        let step = step.parse::<u8>().map_err(|_| invalid("cron step"))?;
        if step == 0 {
            return Err(invalid("cron step"));
        }
        values.extend(
            (minimum..=maximum)
                .filter(|value| *value % step == 0)
                .map(|value| value.to_string()),
        );
    }
    Ok(values.join(","))
}

fn normalize_sunday_seven(expression: &str) -> String {
    let mut fields = expression
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let Some(weekday) = fields.get_mut(4) else {
        return expression.to_owned();
    };
    *weekday = weekday
        .split(',')
        .map(|part| match part {
            "7" => "SUN".to_owned(),
            "0-7" => "*".to_owned(),
            other => other.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(",");
    fields.join(" ")
}

fn validate_supported(expression: &str) -> Result<(), RuntimeError> {
    if expression.chars().all(|character| {
        character.is_ascii_digit() || character.is_ascii_whitespace() || "*,/-".contains(character)
    }) {
        Ok(())
    } else {
        Err(invalid("cron expression"))
    }
}

fn has_bare_value_step(fields: &[&str]) -> bool {
    fields.iter().any(|field| {
        field.split(',').any(|part| {
            part.split_once('/').is_some_and(|(base, step)| {
                base.bytes().all(|byte| byte.is_ascii_digit())
                    && step.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
    })
}

fn next_minute(candidate: DateTime<Utc>) -> Result<DateTime<Utc>, RuntimeError> {
    let seconds = candidate.timestamp();
    let next = seconds
        .div_euclid(MINUTE_SECONDS)
        .checked_add(1)
        .and_then(|value| value.checked_mul(MINUTE_SECONDS))
        .ok_or_else(|| invalid("cron instant"))?;
    DateTime::from_timestamp(next, 0).ok_or_else(|| invalid("cron instant"))
}

fn nearest(value: u64, values: &[u64]) -> u64 {
    values.iter().copied().fold(values[0], |previous, current| {
        if current.abs_diff(value) < previous.abs_diff(value) {
            current
        } else {
            previous
        }
    })
}

fn parsed(cron: &str, note: Option<String>) -> ParsedInterval {
    ParsedInterval {
        cron: cron.into(),
        note,
    }
}

fn valid_unit(unit: &str) -> bool {
    matches!(
        unit,
        "s" | "sec"
            | "secs"
            | "second"
            | "seconds"
            | "m"
            | "min"
            | "mins"
            | "minute"
            | "minutes"
            | "h"
            | "hr"
            | "hrs"
            | "hour"
            | "hours"
            | "d"
            | "day"
            | "days"
    )
}

fn invalid(context: &str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}
