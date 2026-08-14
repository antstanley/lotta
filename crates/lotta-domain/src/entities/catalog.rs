use super::{BoundedMap, UNBOUNDED_MAP_FIELDS_MAX};
use crate::NonEmptyString;
use serde::{Deserialize, Serialize};

/// Local inference provider connection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProviderConnection {
    /// Provider identifier.
    pub provider_id: NonEmptyString,
    /// Authentication method identifier.
    pub auth_method: NonEmptyString,
    /// Whether credentials are currently connected.
    pub connected: bool,
    /// Optional non-secret metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>,
}

/// Model catalog entry.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ModelDescriptor {
    /// Canonical model handle.
    pub handle: NonEmptyString,
    /// Owning provider identifier.
    pub provider_id: NonEmptyString,
    /// Whether the model can currently be selected.
    pub available: bool,
    /// Optional positive context-window size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Optional model settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>,
}
