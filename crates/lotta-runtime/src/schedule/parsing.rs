//! Cron and interval parsing tests mirroring the pinned baseline vectors.

use super::{ScheduleExpression, parse_interval};
use chrono::{TimeZone, Utc};

const UTC: chrono_tz::Tz = chrono_tz::UTC;

#[test]
fn parses_cron() {
    for valid in [
        "*/5 * * * *",
        "0 */2 * * *",
        "30 14 * * *",
        "0 0 * * *",
        "0 0 */3 * *",
        "0-59 * * * *",
        "0 0 */3,15 * *",
        "0 0 */3,2-3 * *",
        "0 0 * */3,6 *",
        "0 0 */32,15 * *",
        "0 0 * */13,6 *",
        "59 23 31 12 7",
        "0-59 0-23 1-31 1-12 0-7",
        "1,5,9 * * * *",
        "0 1,5,9 * * *",
        "1,5,9,13,17,21 * * * *",
        "0 0 * * 1,3,5",
        "1-21/4 * * * *",
        "0 0-23/2 * * *",
        "1-5,10-15 * * * *",
        "*/5,*/15 * * * *",
        "0,10-20,30-59/5 * * * *",
    ] {
        assert!(ScheduleExpression::parse(valid).is_ok(), "{valid}");
    }
    for invalid in [
        "",
        "* * *",
        "* * * * * *",
        "abc * * * *",
        "MON * * * *",
        "0 0 * * MON",
        "1, * * * *",
        ",5 * * * *",
        "5/10 * * * *",
        "60 * * * *",
        "0 24 * * *",
        "0 0 0 * *",
        "0 0 32 * *",
        "0 0 */32 * *",
        "0 0 1 0 *",
        "0 0 1 13 *",
        "0 0 * */13 *",
        "0 0 * * 8",
        "0-60 * * * *",
        "0 0 1-32 * *",
        "0-59/0 * * * *",
        "0,60 * * * *",
        "59-0 * * * *",
        "0 0 31-1 * *",
    ] {
        assert!(ScheduleExpression::parse(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn parses_interval() {
    let vectors = [
        (" 5 MINUTES ", Some("*/5 * * * *")),
        ("5min", Some("*/5 * * * *")),
        ("5mins", Some("*/5 * * * *")),
        ("5minutes", Some("*/5 * * * *")),
        ("1m", Some("*/1 * * * *")),
        ("7minutes", Some("*/6 * * * *")),
        ("2hr", Some("0 */2 * * *")),
        ("2hrs", Some("0 */2 * * *")),
        ("2hours", Some("0 */2 * * *")),
        ("4h", Some("0 */4 * * *")),
        ("6h", Some("0 */6 * * *")),
        ("5 HOURS", Some("0 */4 * * *")),
        ("24h", Some("0 0 * * *")),
        ("48hours", Some("0 0 * * *")),
        ("1day", Some("0 0 * * *")),
        ("3days", Some("0 0 */3 * *")),
        ("31d", Some("0 0 */31 * *")),
        ("32d", None),
        ("30SEC", Some("*/1 * * * *")),
        ("120secs", Some("*/2 * * * *")),
        ("", None),
        ("abc", None),
        ("0m", None),
        ("-5m", None),
        ("5w", None),
    ];
    for (input, expected) in vectors {
        assert_eq!(
            parse_interval(input)
                .as_ref()
                .map(|value| value.cron.as_str()),
            expected
        );
    }
    assert!(parse_interval("30s").expect("seconds").note.is_some());
    assert!(parse_interval("7m").expect("minutes").note.is_some());
    assert!(parse_interval("5h").expect("hours").note.is_some());
}

#[test]
fn normalizes_one_based_wildcard_steps() {
    let cases = [
        ("0 0 */3 * *", (2026, 3, 1), (2026, 3, 3)),
        ("0 0 */3,15 * *", (2026, 3, 1), (2026, 3, 3)),
        ("0 0 */3,15 * *", (2026, 3, 14), (2026, 3, 15)),
        ("0 0 */32,15 * *", (2026, 3, 15), (2026, 4, 15)),
        ("0 0 */32,15 * *", (2026, 3, 14), (2026, 3, 15)),
        ("0 0 * */13,6 *", (2026, 1, 1), (2026, 6, 1)),
        ("0 0 * */3,6 *", (2026, 1, 1), (2026, 3, 1)),
    ];
    for (expression, from, expected) in cases {
        let parsed = ScheduleExpression::parse(expression).expect("cron");
        let after = utc(from.0, from.1, from.2);
        let next = parsed.next_after(after, UTC).expect("next");
        assert_eq!(
            next,
            utc(expected.0, expected.1, expected.2),
            "{expression}"
        );
    }
}

#[test]
fn resolves_across_dst() {
    let timezone: chrono_tz::Tz = "America/New_York".parse().expect("timezone");
    let spring = ScheduleExpression::parse("30 2 * * *").expect("cron");
    let after = Utc
        .with_ymd_and_hms(2025, 3, 9, 6, 59, 0)
        .single()
        .expect("time");
    assert_eq!(
        spring.next_after(after, timezone).expect("next"),
        Utc.with_ymd_and_hms(2025, 3, 10, 6, 30, 0)
            .single()
            .expect("expected")
    );
    let fall = ScheduleExpression::parse("30 1 * * *").expect("cron");
    let before = Utc
        .with_ymd_and_hms(2025, 11, 2, 5, 29, 0)
        .single()
        .expect("time");
    let first = fall.next_after(before, timezone).expect("first");
    let second = fall.next_after(first, timezone).expect("second");
    assert_eq!(
        first,
        Utc.with_ymd_and_hms(2025, 11, 2, 5, 30, 0)
            .single()
            .unwrap()
    );
    assert_eq!(
        second,
        Utc.with_ymd_and_hms(2025, 11, 2, 6, 30, 0)
            .single()
            .unwrap()
    );
}

fn utc(year: i32, month: u32, day: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .expect("test time")
}
