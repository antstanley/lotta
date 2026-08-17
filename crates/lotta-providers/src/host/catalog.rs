//! Complete deterministic typed catalog returned by the pinned host.

use super::protocol::{HOST_MODELS_MAX, HOST_PROVIDERS_MAX, HOST_TEXT_BYTES_MAX};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const HOST_CONTEXT_WINDOW_MAX: u64 = 100_000_000;
const HOST_MAX_TOKENS_MAX: u64 = 10_000_000;
const UNPRICED_SENTINEL: f64 = -1_000_000.0;

/// Complete provider catalog and monotonic mod-registration revision.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostCatalog {
    /// Deterministically ordered provider descriptors.
    pub providers: Vec<ProviderDescriptor>,
    /// Current registration revision.
    pub revision: u64,
}

#[cfg(test)]
mod certification {
    use super::super::client::HostClient;
    use super::super::tests::{config, owner};
    use serde_json::json;

    #[tokio::test]
    async fn complete_named_families_and_counts() {
        let (config, scratch) = config();
        let mut client = HostClient::spawn(config, owner()).await.unwrap();
        let first = client.catalog().await.unwrap();
        for family in [
            "openai",
            "anthropic",
            "openrouter",
            "amazon-bedrock",
            "google",
            "google-vertex",
            "zai",
            "minimax",
            "moonshotai",
            "kimi-coding",
        ] {
            assert!(first.providers.iter().any(|provider| provider.id == family));
        }
        assert_eq!(first.providers.len(), 38);
        assert_eq!(first.model_count(), 1109);
        assert_eq!(first.stable_hash().unwrap(), first.stable_hash().unwrap());
        client.shutdown().await.unwrap();
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[tokio::test]
    async fn mod_defined_provider_registers() {
        let (config, scratch) = config();
        let mut client = HostClient::spawn(config, owner()).await.unwrap();
        let descriptor = json!({
            "id":"mod-example","name":"Mod Example","owner":"mod-a",
            "adapter":{"type":"pi_ai_api","api":"openai-completions"},
            "models":[{
                "id":"model-a","name":"Model A","api":"openai-completions",
                "provider":"mod-example","reasoning":false,"input":["text"],
                "contextWindow":4096,"maxTokens":1024,
                "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"tiers":[]}
            }]
        });
        assert_eq!(client.register(descriptor).await.unwrap(), 1);
        assert!(
            client
                .catalog()
                .await
                .unwrap()
                .providers
                .iter()
                .any(|provider| provider.id == "mod-example")
        );
        assert_eq!(client.unregister("mod-example", "mod-a").await.unwrap(), 2);
        assert!(
            !client
                .catalog()
                .await
                .unwrap()
                .providers
                .iter()
                .any(|provider| provider.id == "mod-example")
        );
        client.shutdown().await.unwrap();
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

/// Credential-free built-in or mod-defined provider descriptor.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderDescriptor {
    /// Stable provider identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `builtin` or `mod`.
    pub source: String,
    /// Mod registration owner when applicable.
    #[serde(default)]
    pub owner: Option<String>,
    /// Complete static model catalog.
    pub models: Vec<ModelDescriptor>,
}

/// Complete bounded pi-ai model metadata needed for selection and pricing.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelDescriptor {
    /// Provider-local model identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Exact pi-ai API dialect.
    pub api: String,
    /// Owning provider identifier.
    pub provider: String,
    /// Whether reasoning is supported.
    pub reasoning: bool,
    /// Supported input kinds.
    pub input: Vec<String>,
    /// Context window.
    pub context_window: u64,
    /// Maximum output tokens.
    pub max_tokens: u64,
    /// Complete normalized cost metadata.
    pub cost: ModelCost,
}

/// Per-million-token cost metadata.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelCost {
    /// Input cost.
    pub input: Option<f64>,
    /// Output cost.
    pub output: Option<f64>,
    /// Cache-read cost.
    pub cache_read: Option<f64>,
    /// Cache-write cost.
    pub cache_write: Option<f64>,
    /// Optional request-wide pricing tiers.
    pub tiers: Vec<ModelCostTier>,
}

/// One thresholded cost tier.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelCostTier {
    /// Input-token threshold.
    pub input_tokens_above: u64,
    /// Input cost.
    pub input: Option<f64>,
    /// Output cost.
    pub output: Option<f64>,
    /// Cache-read cost.
    pub cache_read: Option<f64>,
    /// Cache-write cost.
    pub cache_write: Option<f64>,
}

/// Stable catalog validation failure.
#[derive(Debug, thiserror::Error)]
#[error("invalid provider host catalog")]
pub struct CatalogError;

impl HostCatalog {
    /// Validates completeness bounds, deterministic ordering, identities, and finite costs.
    ///
    /// # Errors
    /// Returns [`CatalogError`] for any malformed or out-of-bound descriptor.
    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.providers.len() > HOST_PROVIDERS_MAX || !sorted_unique(&self.providers, |p| &p.id) {
            return Err(CatalogError);
        }
        let mut total = 0_usize;
        for provider in &self.providers {
            total = total
                .checked_add(provider.models.len())
                .ok_or(CatalogError)?;
            if total > HOST_MODELS_MAX
                || !text(&provider.id)
                || !text(&provider.name)
                || !sorted_unique(&provider.models, |model| &model.id)
                || provider
                    .models
                    .iter()
                    .any(|model| !valid_model(model, &provider.id))
            {
                return Err(CatalogError);
            }
        }
        Ok(())
    }

    /// Returns the stable SHA-256 of the deterministic typed catalog encoding.
    ///
    /// # Errors
    /// Returns [`CatalogError`] if catalog validation or deterministic encoding fails.
    pub fn stable_hash(&self) -> Result<String, CatalogError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| CatalogError)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    /// Returns the complete model count.
    #[must_use]
    pub fn model_count(&self) -> usize {
        self.providers
            .iter()
            .map(|provider| provider.models.len())
            .sum()
    }
}

fn valid_model(model: &ModelDescriptor, provider: &str) -> bool {
    model.provider == provider
        && text(&model.id)
        && text(&model.name)
        && text(&model.api)
        && (1..=HOST_CONTEXT_WINDOW_MAX).contains(&model.context_window)
        && (1..=HOST_MAX_TOKENS_MAX).contains(&model.max_tokens)
        && !model.input.is_empty()
        && sorted_unique_strings(&model.input)
        && model
            .input
            .iter()
            .all(|value| value == "image" || value == "text")
        && valid_cost(&model.cost)
}

fn text(value: &str) -> bool {
    !value.is_empty() && value.len() <= HOST_TEXT_BYTES_MAX
}

fn sorted_unique<T>(values: &[T], key: impl Fn(&T) -> &String) -> bool {
    let keys: Vec<_> = values.iter().map(key).collect();
    let unique: BTreeSet<_> = keys.iter().copied().collect();
    keys.windows(2)
        .all(|pair| pair[0].as_bytes() < pair[1].as_bytes())
        && unique.len() == keys.len()
}

fn sorted_unique_strings(values: &[String]) -> bool {
    values
        .windows(2)
        .all(|pair| pair[0].as_bytes() < pair[1].as_bytes())
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn valid_cost(cost: &ModelCost) -> bool {
    [cost.input, cost.output, cost.cache_read, cost.cache_write]
        .iter()
        .all(|value| value.is_none_or(|value| value.is_finite() && value >= 0.0))
        && cost
            .tiers
            .windows(2)
            .all(|pair| pair[0].input_tokens_above < pair[1].input_tokens_above)
        && cost.tiers.iter().all(|tier| {
            [tier.input, tier.output, tier.cache_read, tier.cache_write]
                .iter()
                .all(|value| value.is_none_or(|value| value.is_finite() && value >= 0.0))
        })
}

/// Converts the exact upstream unpriced sentinel without dropping the model.
#[must_use]
pub fn normalize_cost(value: f64) -> Option<f64> {
    if value == UNPRICED_SENTINEL {
        None
    } else {
        Some(value)
    }
}
