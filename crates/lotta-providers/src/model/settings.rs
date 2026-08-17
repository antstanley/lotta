use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const MODEL_SETTINGS_BYTES_MAX: usize = 1_048_576;
const MODEL_SETTINGS_FIELDS_MAX: usize = 1_024;

const ALIASES: &[(&str, &[&str])] = &[
    (
        "context_window_limit",
        &["contextWindow", "context_window", "contextWindowLimit"],
    ),
    ("reasoning_effort", &["reasoningEffort"]),
    ("reasoning_tier", &["reasoningTier"]),
    ("base_url", &["baseUrl", "endpoint"]),
    ("provider_type", &["providerType"]),
];

/// Open, bounded model settings with canonical known keys and lossless unknown keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelSettings(BTreeMap<String, Value>);

impl ModelSettings {
    /// Normalizes known aliases while preserving all provider-specific keys.
    ///
    /// # Errors
    /// Rejects duplicate aliases, conflicting provider types, invalid known values, or bounds.
    pub fn normalize(value: &Value) -> Result<Self, ModelSettingsError> {
        let object = value
            .as_object()
            .ok_or(ModelSettingsError::ExpectedObject)?;
        if object.len() > MODEL_SETTINGS_FIELDS_MAX {
            return Err(ModelSettingsError::TooLarge);
        }
        let bytes = serde_json::to_vec(value)
            .map_err(|_| ModelSettingsError::InvalidValue)?
            .len();
        if bytes > MODEL_SETTINGS_BYTES_MAX {
            return Err(ModelSettingsError::TooLarge);
        }
        let mut values: BTreeMap<String, Value> = object.clone().into_iter().collect();
        normalize_aliases(&mut values)?;
        remove_unset_known(&mut values);
        validate_known(&values)?;
        Ok(Self(values))
    }

    /// Returns a known or provider-specific setting.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// Returns whether there are no configured settings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Overlays all keys from `higher`, returning canonical deterministic settings.
    #[must_use]
    pub fn merged_with(&self, higher: &Self) -> Self {
        let mut merged = self.0.clone();
        merged.extend(higher.0.clone());
        Self(merged)
    }
}

fn normalize_aliases(values: &mut BTreeMap<String, Value>) -> Result<(), ModelSettingsError> {
    for (canonical, aliases) in ALIASES {
        let mut found = Vec::new();
        if let Some(value) = values.remove(*canonical) {
            found.push(value);
        }
        for alias in *aliases {
            if let Some(value) = values.remove(*alias) {
                found.push(value);
            }
        }
        if found.len() > 1 {
            return Err(ModelSettingsError::DuplicateAlias);
        }
        if let Some(value) = found.pop() {
            values.insert((*canonical).to_owned(), value);
        }
    }
    Ok(())
}

fn remove_unset_known(values: &mut BTreeMap<String, Value>) {
    for key in [
        "context_window_limit",
        "reasoning_effort",
        "reasoning_tier",
        "base_url",
        "provider_type",
    ] {
        if values.get(key).is_some_and(Value::is_null) {
            values.remove(key);
        }
    }
}

fn validate_known(values: &BTreeMap<String, Value>) -> Result<(), ModelSettingsError> {
    if values
        .get("context_window_limit")
        .is_some_and(|value| value.as_u64().is_none_or(|number| number == 0))
    {
        return Err(ModelSettingsError::InvalidValue);
    }
    for key in [
        "reasoning_effort",
        "reasoning_tier",
        "base_url",
        "provider_type",
    ] {
        if values
            .get(key)
            .is_some_and(|value| value.as_str().is_none_or(str::is_empty))
        {
            return Err(ModelSettingsError::InvalidValue);
        }
    }
    if let (Some(provider_type), Some(provider)) =
        (values.get("provider_type"), values.get("provider"))
        && provider
            .as_str()
            .is_some_and(|provider| Some(provider) != provider_type.as_str())
    {
        return Err(ModelSettingsError::ProviderTypeConflict);
    }
    Ok(())
}

impl Serialize for ModelSettings {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ModelSettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::normalize(&value).map_err(de::Error::custom)
    }
}

impl TryFrom<Map<String, Value>> for ModelSettings {
    type Error = ModelSettingsError;
    fn try_from(value: Map<String, Value>) -> Result<Self, Self::Error> {
        Self::normalize(&Value::Object(value))
    }
}

/// Stable model-settings validation failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ModelSettingsError {
    /// Settings were not a JSON object.
    #[error("model settings must be an object")]
    ExpectedObject,
    /// Multiple aliases for one canonical setting were supplied.
    #[error("duplicate model setting aliases")]
    DuplicateAlias,
    /// Provider type declarations disagreed.
    #[error("conflicting provider type")]
    ProviderTypeConflict,
    /// A known setting had an invalid type or value.
    #[error("invalid model setting")]
    InvalidValue,
    /// The settings exceeded fixed cardinality or byte limits.
    #[error("model settings exceed limit")]
    TooLarge,
}
