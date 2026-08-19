#![allow(clippy::option_option)] // Three-state schema fields require absent/null/value.

use crate::{AgentId, ConversationId, NonEmptyString, Timestamp};
use chrono_tz::Tz;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value};
use std::{fmt, str::FromStr};

/// Validated IANA timezone identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IanaTimezone(Tz);

impl IanaTimezone {
    /// Parses an identifier from the IANA timezone database.
    ///
    /// # Errors
    /// Returns a typed error when the identifier is not present in the database.
    pub fn new(value: &str) -> Result<Self, crate::DomainError> {
        Tz::from_str(value)
            .map(Self)
            .map_err(|_| crate::DomainError::InvalidTimezone {
                value: value.into(),
            })
    }

    /// Returns the canonical IANA identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.name()
    }

    /// Returns the parsed timezone value.
    #[must_use]
    pub const fn as_tz(&self) -> Tz {
        self.0
    }
}

impl fmt::Display for IanaTimezone {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for IanaTimezone {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for IanaTimezone {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(de::Error::custom)
    }
}

/// Schedule lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleStatus {
    /// Eligible to fire.
    Active,
    /// One-shot schedule fired.
    Fired,
    /// One-shot schedule missed its window.
    Missed,
    /// Schedule was cancelled.
    Cancelled,
}

/// Terminal cancellation reason.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleCancelReason {
    /// Target conversation was unavailable.
    ConversationNotFound,
    /// Schedule expired.
    Expired,
}

/// Last schedule run outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleRunOutcome {
    /// Prompt entered the queue.
    Queued,
    /// Fire window was missed.
    Missed,
    /// Execution failed.
    Failed,
    /// Scheduler intentionally skipped execution.
    Skipped,
}

/// Canonical persisted schedule with all 26 observed properties.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Schedule {
    /// Schedule identifier.
    pub id: NonEmptyString,
    /// Target agent.
    pub agent_id: AgentId,
    /// Target conversation.
    pub conversation_id: ConversationId,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Cron expression.
    pub cron: NonEmptyString,
    /// Validated IANA timezone.
    pub timezone: IanaTimezone,
    /// Whether the schedule recurs.
    pub recurring: bool,
    /// Prompt to enqueue.
    pub prompt: NonEmptyString,
    /// Lifecycle status.
    pub status: ScheduleStatus,
    /// Creation timestamp.
    pub created_at: Timestamp,
    /// Expiry timestamp; required and nullable.
    pub expires_at: Option<Timestamp>,
    /// Latest successful fire; required and nullable.
    pub last_fired_at: Option<Timestamp>,
    /// Successful fire count.
    pub fire_count: u64,
    /// Cancellation reason; required and nullable.
    pub cancel_reason: Option<ScheduleCancelReason>,
    /// Signed schedule jitter offset in milliseconds.
    pub jitter_offset_ms: i64,
    /// Latest run timestamp; optional and nullable.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_run_at: Option<Option<Timestamp>>,
    /// Latest run outcome; optional and nullable.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_run_outcome: Option<Option<ScheduleRunOutcome>>,
    /// Latest run reason; optional and nullable.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_run_reason: Option<Option<String>>,
    /// Latest run error; optional and nullable.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_run_error: Option<Option<String>>,
    /// Latest missed-run timestamp; optional and nullable.
    #[serde(
        default,
        deserialize_with = "super::nullable::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_missed_at: Option<Option<Timestamp>>,
    /// Missed-run count; optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missed_count: Option<u64>,
    /// Failed-run count; optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_count: Option<u64>,
    /// One-shot target timestamp; required and nullable.
    pub scheduled_for: Option<Timestamp>,
    /// One-shot fire timestamp; required and nullable.
    pub fired_at: Option<Timestamp>,
    /// One-shot missed timestamp; required and nullable.
    pub missed_at: Option<Timestamp>,
    /// Schema-open per-schedule extensions preserved without loss.
    #[serde(flatten)]
    pub extras: Map<String, Value>,
}
