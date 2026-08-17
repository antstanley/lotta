//! Provider-neutral model configuration services.

mod catalog;
mod handle;
mod resolve;
mod service;
mod settings;

pub use catalog::{
    ConnectionReadiness, ListedModel, MODELS_PER_PROVIDER_MAX, ModelCatalog, ModelCatalogError,
};
pub use handle::{ModelHandle, ModelHandleError};
pub use resolve::{
    ModelOverride, ResolutionError, ResolutionLevel, ResolvedModel, SettingsSource, resolve_model,
};
pub use service::{
    AvailabilityError, ModelAvailability, ModelStore, ModelUpdateError, ModelUpdateService,
    StoredModel,
};
pub use settings::{ModelSettings, ModelSettingsError};

#[cfg(test)]
#[path = "handle_certificate.rs"]
mod handle_certificate;
#[cfg(test)]
#[path = "listing_certificate.rs"]
mod listing;
#[cfg(test)]
#[path = "resolution_order_certificate.rs"]
mod resolution_order;
#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
#[path = "settings_normalization_certificate.rs"]
mod settings_normalization;
#[cfg(test)]
mod test_support;
#[cfg(test)]
#[path = "update_validates_first_certificate.rs"]
mod update_validates_first;
