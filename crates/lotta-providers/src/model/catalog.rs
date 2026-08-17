use super::{ModelHandle, ModelSettings};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Canonical maximum catalog entries owned by one provider.
pub const MODELS_PER_PROVIDER_MAX: usize = 10_000;

/// Non-secret provider connection state projected into model readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionReadiness {
    /// Connection is usable.
    Ready,
    /// No usable connection exists.
    Disconnected,
    /// Connection exists but is not fully ready.
    Degraded,
}

/// Credential-free model listing item.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListedModel {
    /// Stable provider/model handle.
    pub handle: ModelHandle,
    /// Per-model readiness derived from provider connection state and model availability.
    pub readiness: ConnectionReadiness,
    /// Canonical model settings defaults.
    pub model_settings: ModelSettings,
}

/// Validated immutable model catalog.
#[derive(Clone, Debug, Default)]
pub struct ModelCatalog {
    models: BTreeMap<String, Vec<ListedModel>>,
}

impl ModelCatalog {
    /// Creates a bounded catalog atomically.
    ///
    /// # Errors
    /// Rejects duplicate handles, provider mismatch, or more than the canonical provider bound.
    pub fn new(
        entries: impl IntoIterator<Item = (String, Vec<ListedModel>)>,
    ) -> Result<Self, ModelCatalogError> {
        let mut models = BTreeMap::new();
        let mut handles = BTreeSet::new();
        for (provider, provider_models) in entries {
            if provider_models.len() > MODELS_PER_PROVIDER_MAX {
                return Err(ModelCatalogError::TooManyModels);
            }
            for model in &provider_models {
                if model.handle.provider_id() != provider {
                    return Err(ModelCatalogError::ProviderMismatch);
                }
                if !handles.insert(model.handle.to_string()) {
                    return Err(ModelCatalogError::DuplicateModel);
                }
            }
            if models.insert(provider, provider_models).is_some() {
                return Err(ModelCatalogError::DuplicateProvider);
            }
        }
        Ok(Self { models })
    }

    /// Returns credential-free entries with readiness projected from current connection state.
    #[must_use]
    pub fn list_models(
        &self,
        connections: &BTreeMap<String, ConnectionReadiness>,
    ) -> Vec<ListedModel> {
        self.models
            .iter()
            .flat_map(|(provider, models)| {
                let connection = connections
                    .get(provider)
                    .copied()
                    .unwrap_or(ConnectionReadiness::Disconnected);
                models.iter().cloned().map(move |mut model| {
                    model.readiness = match connection {
                        ConnectionReadiness::Disconnected => ConnectionReadiness::Disconnected,
                        ConnectionReadiness::Degraded => ConnectionReadiness::Degraded,
                        ConnectionReadiness::Ready => model.readiness,
                    };
                    model
                })
            })
            .collect()
    }
}

/// Stable catalog construction failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ModelCatalogError {
    /// A provider exceeded the canonical model count.
    #[error("models per provider exceed limit")]
    TooManyModels,
    /// A model handle was repeated.
    #[error("duplicate model handle")]
    DuplicateModel,
    /// A provider catalog key was repeated.
    #[error("duplicate provider catalog")]
    DuplicateProvider,
    /// A handle's provider differed from its catalog owner.
    #[error("model provider mismatch")]
    ProviderMismatch,
}
