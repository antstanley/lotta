use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};

const MODEL_ID_BYTES_MAX: usize = 512;
const PROVIDER_ID_BYTES_MAX: usize = 128;

/// A strict stable provider/model identity rendered as `provider/model`.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModelHandle {
    provider_id: String,
    model_id: String,
}

impl ModelHandle {
    /// Creates a strict handle without retaining credentials or endpoint configuration.
    ///
    /// # Errors
    /// Returns an error for empty, oversized, control-containing, or malformed IDs.
    pub fn new(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
    ) -> Result<Self, ModelHandleError> {
        let provider_id = provider_id.into();
        let model_id = model_id.into();
        validate_provider_id(&provider_id)?;
        validate_model_id(&model_id)?;
        Ok(Self {
            provider_id,
            model_id,
        })
    }

    /// Returns the exact provider identifier.
    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// Returns the exact provider-owned model identifier.
    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
}

fn validate_provider_id(value: &str) -> Result<(), ModelHandleError> {
    validate_component(value, PROVIDER_ID_BYTES_MAX)?;
    if value.contains('/') {
        return Err(ModelHandleError::InvalidIdentifier);
    }
    Ok(())
}

fn validate_model_id(value: &str) -> Result<(), ModelHandleError> {
    if value.len() > MODEL_ID_BYTES_MAX {
        return Err(ModelHandleError::IdentifierTooLong);
    }
    if value.split('/').any(str::is_empty) {
        return Err(ModelHandleError::InvalidIdentifier);
    }
    for component in value.split('/') {
        validate_component(component, MODEL_ID_BYTES_MAX)?;
    }
    Ok(())
}

fn validate_component(value: &str, max: usize) -> Result<(), ModelHandleError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(ModelHandleError::InvalidIdentifier);
    }
    if value.len() > max {
        return Err(ModelHandleError::IdentifierTooLong);
    }
    Ok(())
}

impl FromStr for ModelHandle {
    type Err = ModelHandleError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (provider, model) = value
            .split_once('/')
            .ok_or(ModelHandleError::InvalidFormat)?;
        Self::new(provider, model)
    }
}

impl fmt::Display for ModelHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.provider_id, self.model_id)
    }
}

impl fmt::Debug for ModelHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Serialize for ModelHandle {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ModelHandle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Stable model-handle validation failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ModelHandleError {
    /// The display form did not contain a provider/model separator.
    #[error("invalid model handle format")]
    InvalidFormat,
    /// An identifier was empty or contained forbidden bytes.
    #[error("invalid model identifier")]
    InvalidIdentifier,
    /// An identifier exceeded its fixed byte limit.
    #[error("model identifier exceeds limit")]
    IdentifierTooLong,
}
