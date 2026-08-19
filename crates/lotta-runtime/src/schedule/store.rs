use lotta_domain::Schedule;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::RuntimeError;

/// Maximum schedules accepted from the canonical file.
pub const SCHEDULES_MAX: usize = 4_096;
/// Maximum encoded canonical cron file bytes.
pub const SCHEDULE_FILE_BYTES_MAX: usize = 8 * 1_024 * 1_024;

/// Unknown root fields preserved because the baseline root is extensible.
pub type ScheduleFileExtras = Map<String, Value>;

/// Canonical `crons.json` root.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScheduleFile {
    /// Exact persisted format version.
    pub version: u8,
    /// Baseline scheduler lease; retained opaquely by Task 60.
    pub scheduler_owner: Option<Value>,
    /// Canonical schedule records.
    pub tasks: Vec<Schedule>,
    /// Tolerated baseline root extensions preserved without loss.
    #[serde(flatten)]
    pub extras: ScheduleFileExtras,
}

/// Optimistic opaque revision transported by concrete store adapters.
#[derive(Clone, Debug)]
pub struct ScheduleStoreRevision(Vec<u8>);

impl ScheduleStoreRevision {
    /// Constructs a revision from adapter-owned opaque bytes.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Borrows the opaque adapter revision.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl ScheduleFile {
    /// Decodes and validates one bounded canonical schedule file.
    ///
    /// # Errors
    /// Rejects oversized, malformed, wrong-version, duplicate-ID, invalid-cron,
    /// and over-count files.
    pub fn decode(bytes: &[u8]) -> Result<Self, RuntimeError> {
        if bytes.len() > SCHEDULE_FILE_BYTES_MAX {
            return Err(limit("schedule file bytes"));
        }
        let file: Self = serde_json::from_slice(bytes).map_err(|_| invalid("schedule file"))?;
        file.validate()?;
        Ok(file)
    }

    /// Encodes stable pretty JSON with a final line feed.
    ///
    /// # Errors
    /// Rejects invalid state, serialization failure, and an oversized result.
    pub fn encode(&self) -> Result<Vec<u8>, RuntimeError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|_| invalid("schedule file"))?;
        bytes.push(b'\n');
        if bytes.len() > SCHEDULE_FILE_BYTES_MAX {
            return Err(limit("schedule file bytes"));
        }
        Ok(bytes)
    }

    fn validate(&self) -> Result<(), RuntimeError> {
        if self.version != 1 || self.tasks.len() > SCHEDULES_MAX {
            return Err(invalid("schedule file version or count"));
        }
        let mut identifiers = std::collections::BTreeSet::new();
        validate_extras(&self.extras, "schedule root extras")?;
        for task in &self.tasks {
            if !identifiers.insert(task.id.as_str()) {
                return Err(invalid("duplicate schedule id"));
            }
            super::ScheduleExpression::parse(task.cron.as_str())?;
            validate_extras(&task.extras, "schedule extras")?;
        }
        Ok(())
    }
}

fn validate_extras(extras: &Map<String, Value>, context: &str) -> Result<(), RuntimeError> {
    if extras.len() > lotta_domain::bounds::UNBOUNDED_MAP_FIELDS_MAX {
        return Err(limit(context));
    }
    let bytes = serde_json::to_vec(extras).map_err(|_| invalid(context))?;
    if bytes.len() > SCHEDULE_FILE_BYTES_MAX {
        return Err(limit(context));
    }
    Ok(())
}

fn invalid(context: &str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}

fn limit(context: &str) -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: context.into(),
    }
}
