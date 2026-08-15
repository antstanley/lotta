use std::fmt::Write as _;

use lotta_domain::{Clock, NonEmptyString, RuntimeScope, Timestamp};
use serde::Serialize;
use uuid::{Uuid, Version};

use super::event::RuntimeEvent;

/// Injectable UUID source for deterministic envelope tests.
pub trait EventIdGenerator: Send + Sync {
    /// Generates one UUID v4 per delivered emission.
    ///
    /// # Errors
    /// Returns an internal error when secure randomness is unavailable.
    fn generate(&self) -> Result<Uuid, crate::error::AppServerError>;
}

/// Production random UUID v4 source.
pub struct RandomEventIdGenerator;

impl EventIdGenerator for RandomEventIdGenerator {
    fn generate(&self) -> Result<Uuid, crate::error::AppServerError> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| crate::error::AppServerError::Internal)?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Ok(Uuid::from_bytes(bytes))
    }
}

/// One independently stamped runtime event delivery.
#[derive(Clone, Debug, Serialize)]
pub struct StampedRuntimeEvent {
    /// Exact runtime event payload.
    #[serde(flatten)]
    pub event: RuntimeEvent,
    /// Runtime scope.
    pub runtime: RuntimeScope,
    /// Per-connection event sequence.
    pub event_seq: u64,
    /// Emission timestamp.
    pub emitted_at: Timestamp,
    /// Unique bounded per-emission key.
    pub idempotency_key: NonEmptyString,
}

/// Creates an envelope after the caller has checked sequence overflow.
///
/// # Errors
/// Returns an internal error for a nil/non-v4 UUID or invalid bounded key.
pub fn stamp(
    event: RuntimeEvent,
    runtime: RuntimeScope,
    event_seq: u64,
    clock: &(dyn Clock + Send + Sync),
    ids: &dyn EventIdGenerator,
) -> Result<StampedRuntimeEvent, crate::error::AppServerError> {
    let discriminant = event.discriminant();
    let uuid = ids.generate()?;
    if uuid.is_nil() || uuid.get_version() != Some(Version::Random) {
        return Err(crate::error::AppServerError::Internal);
    }
    let key_bytes = discriminant
        .len()
        .checked_add(1 + 20 + 1 + 36)
        .ok_or(crate::error::AppServerError::Internal)?;
    let mut key = String::new();
    key.try_reserve(key_bytes)
        .map_err(|_| crate::error::AppServerError::Unavailable)?;
    write!(&mut key, "{discriminant}:{event_seq}:{uuid}")
        .map_err(|_| crate::error::AppServerError::Internal)?;
    let idempotency_key =
        NonEmptyString::new(key).map_err(|_| crate::error::AppServerError::Internal)?;
    Ok(StampedRuntimeEvent {
        event,
        runtime,
        event_seq,
        emitted_at: clock.now(),
        idempotency_key,
    })
}

#[cfg(test)]
#[path = "envelope_tests.rs"]
mod tests;
