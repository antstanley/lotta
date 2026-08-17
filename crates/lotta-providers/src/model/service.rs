use super::{ModelHandle, ModelSettings};

/// Persisted model state guarded by an optimistic revision.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredModel {
    /// Stable model handle.
    pub handle: ModelHandle,
    /// Canonical open settings.
    pub settings: ModelSettings,
    /// Opaque optimistic revision.
    pub revision: u64,
}

/// Credential-free availability check failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AvailabilityError {
    /// Availability could not be established.
    #[error("model availability check failed")]
    CheckFailed,
}

/// Port for checking whether a handle can currently be selected.
pub trait ModelAvailability: Send + Sync {
    /// Checks current model availability without exposing connection credentials.
    ///
    /// # Errors
    /// Returns a credential-free failure when readiness cannot be established.
    fn is_available(&self, handle: &ModelHandle) -> Result<bool, AvailabilityError>;
}

/// Atomic optimistic model persistence port.
pub trait ModelStore: Send + Sync {
    /// Replaces model state only when `expected_revision` is current.
    ///
    /// # Errors
    /// Returns a stable conflict or persistence failure without partial mutation.
    fn replace_model(
        &self,
        expected_revision: u64,
        handle: ModelHandle,
        settings: ModelSettings,
    ) -> Result<StoredModel, ModelUpdateError>;
}

/// Validate-before-write model update service.
pub struct ModelUpdateService<'a, A, S> {
    availability: &'a A,
    store: &'a S,
}

impl<'a, A: ModelAvailability, S: ModelStore> ModelUpdateService<'a, A, S> {
    /// Creates a service over dependency-neutral availability and persistence ports.
    #[must_use]
    pub const fn new(availability: &'a A, store: &'a S) -> Self {
        Self {
            availability,
            store,
        }
    }

    /// Checks availability first, then performs one optimistic atomic store write.
    ///
    /// # Errors
    /// Returns without writing on unavailable/check errors; revision conflicts remain atomic.
    pub fn update(
        &self,
        expected_revision: u64,
        handle: ModelHandle,
        settings: ModelSettings,
    ) -> Result<StoredModel, ModelUpdateError> {
        match self.availability.is_available(&handle) {
            Ok(true) => self
                .store
                .replace_model(expected_revision, handle, settings),
            Ok(false) => Err(ModelUpdateError::Unavailable),
            Err(_) => Err(ModelUpdateError::AvailabilityCheck),
        }
    }
}

/// Stable credential-free update failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ModelUpdateError {
    /// The requested model is not currently available.
    #[error("model unavailable")]
    Unavailable,
    /// Availability could not be established.
    #[error("model availability check failed")]
    AvailabilityCheck,
    /// Persisted state changed before the atomic write.
    #[error("model revision conflict")]
    RevisionConflict,
    /// Persistence failed without exposing backend details.
    #[error("model persistence failed")]
    Store,
}
