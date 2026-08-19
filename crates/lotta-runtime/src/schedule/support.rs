//! Shared deterministic fixtures for schedule tests.

use super::JitterSource;
use chrono::{TimeZone, Utc};
use lotta_domain::Schedule;
use std::collections::VecDeque;

pub(crate) struct Fixed(pub(crate) VecDeque<u64>);

impl Fixed {
    pub(crate) fn one(value: u64) -> Self {
        Self(VecDeque::from([value]))
    }
}

impl JitterSource for Fixed {
    fn next_u64(&mut self) -> Result<u64, crate::RuntimeError> {
        Ok(self.0.pop_front().unwrap_or_default())
    }
}

pub(crate) fn instant(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0).single().expect("test time")
}

pub(crate) fn schedule(
    recurring: bool,
    cron: &str,
    timezone: &str,
    created: chrono::DateTime<Utc>,
) -> Schedule {
    serde_json::from_value(serde_json::json!({
        "id":"schedule", "agent_id":"agent-local-fixture", "conversation_id":"conversation",
        "name":"name", "description":"description", "cron":cron,
        "timezone":timezone, "recurring":recurring, "prompt":"prompt", "status":"active",
        "created_at":created.to_rfc3339(), "expires_at":null, "last_fired_at":null,
        "fire_count":0, "cancel_reason":null, "jitter_offset_ms":0,
        "last_run_at":null, "last_run_outcome":null, "last_run_reason":null,
        "last_run_error":null, "last_missed_at":null, "missed_count":0, "failed_count":0,
        "scheduled_for":null, "fired_at":null, "missed_at":null
    }))
    .expect("schedule")
}
