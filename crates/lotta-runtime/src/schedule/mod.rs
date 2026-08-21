//! Schedule parsing, timing, jitter, lifecycle, and persistence boundary types.

mod cron;
mod jitter;
mod lifecycle;
mod store;

pub use cron::{ParsedInterval, ScheduleExpression, parse_interval};
pub use jitter::{JitterSource, compute_jitter};
pub use lifecycle::{RunUpdate, apply_run_update};
pub use store::{ScheduleFile, ScheduleFileExtras, ScheduleStoreRevision};

/// Returns whether appending a record stays within both inclusive retention bounds.
#[must_use]
pub const fn run_log_append_fits(
    existing_bytes: usize,
    existing_lines: usize,
    appended_bytes: usize,
    keep_lines: usize,
    bytes_max: usize,
) -> bool {
    match (
        existing_lines.checked_add(1),
        existing_bytes.checked_add(appended_bytes),
    ) {
        (Some(lines), Some(bytes)) => lines <= keep_lines && bytes <= bytes_max,
        _ => false,
    }
}

#[cfg(test)]
mod store_round_trip;

#[cfg(test)]
mod parsing;

#[cfg(test)]
mod jitter_offset;

#[cfg(test)]
mod support;

#[cfg(test)]
mod run_log {
    use super::run_log_append_fits;

    #[test]
    fn inclusive_line_rotation_boundary() {
        assert!(run_log_append_fits(10, 1_998, 10, 2_000, 2_000_000));
        assert!(run_log_append_fits(10, 1_999, 10, 2_000, 2_000_000));
        assert!(!run_log_append_fits(10, 2_000, 10, 2_000, 2_000_000));
    }

    #[test]
    fn inclusive_byte_rotation_boundary() {
        assert!(run_log_append_fits(1_999_989, 1, 10, 2_000, 2_000_000));
        assert!(run_log_append_fits(1_999_990, 1, 10, 2_000, 2_000_000));
        assert!(!run_log_append_fits(1_999_991, 1, 10, 2_000, 2_000_000));
    }

    #[test]
    fn production_defaults_match_baseline() {
        assert_eq!(
            lotta_domain::bounds::SCHEDULE_RUN_LOG_KEEP_LINES.value,
            2_000
        );
        assert_eq!(
            lotta_domain::bounds::SCHEDULE_RUN_LOG_BYTES_MAX.value,
            2_000_000
        );
    }

    #[test]
    fn overflow_bounds_reject_appends() {
        assert!(!run_log_append_fits(usize::MAX, 0, 1, 2_000, 2_000_000));
        assert!(!run_log_append_fits(0, usize::MAX, 1, 2_000, 2_000_000));
    }
}
