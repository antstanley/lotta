//! Canonical schedule run-log record shapes shared with persistence adapters.

use serde::{Deserialize, Serialize};

/// Canonical run-log status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunLogStatus {
    /// Successful completion.
    Ok,
    /// Failed completion.
    Error,
    /// Intentionally skipped.
    Skipped,
}

/// Exact canonical run-log action.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunLogAction {
    /// Run processing finished.
    Finished,
}

/// Canonical JSONL run-log record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunLogEntry {
    /// Unix epoch milliseconds.
    pub ts: i64,
    /// Schedule identifier.
    pub job_id: String,
    /// Exact canonical action.
    pub action: RunLogAction,
    /// Optional status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<RunLogStatus>,
    /// Optional schedule outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<lotta_domain::ScheduleRunOutcome>,
    /// Optional canonical reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional scrubbed error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Optional summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Optional agent identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Optional conversation identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Optional run identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Optional planned epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_at_ms: Option<i64>,
    /// Optional queue item identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_item_id: Option<String>,
    /// Optional one-shot timestamp, including explicit null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<Option<String>>,
    /// Optional fire timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<String>,
    /// Optional missed count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missed_count: Option<u64>,
    /// Optional due-window start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_start: Option<String>,
    /// Optional due-window end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_end: Option<String>,
}
