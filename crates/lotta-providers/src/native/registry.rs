//! Native provider adapter registry used by production selection.

use crate::connections::{ConnectionError, ProviderAuth};
use crate::native::anthropic::Anthropic;
use crate::native::openai_compatible::{MaxTokensField, OpenAiCompatible};
use lotta_runtime::ports::ProviderPort;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use url::Url;

/// Future returned by a native adapter factory.
pub type NativeFactoryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn ProviderPort>, ConnectionError>> + Send + 'a>>;

/// Secret-scoped factory for one registered native provider family.
pub trait NativeAdapterFactory: Send + Sync {
    /// Builds a single-attempt normalized provider adapter.
    fn build<'a>(
        &'a self,
        provider_type: &'a str,
        auth: &'a ProviderAuth,
        base_url: Option<&'a str>,
    ) -> NativeFactoryFuture<'a>;
}

/// Deterministic registry of provider families implemented by native Rust adapters.
#[derive(Clone, Default)]
pub struct NativeAdapterRegistry {
    factories: BTreeMap<String, Arc<dyn NativeAdapterFactory>>,
}

impl NativeAdapterRegistry {
    /// Creates the production native registry.
    #[must_use]
    pub fn production() -> Self {
        let mut registry = Self::default();
        let native: Arc<dyn NativeAdapterFactory> = Arc::new(HttpNativeFactory);
        for provider in [
            "openai",
            "openai-compatible",
            "anthropic",
            "ollama",
            "ollama-cloud",
            "lmstudio",
            "llama.cpp",
        ] {
            registry.register(provider.to_owned(), Arc::clone(&native));
        }
        registry
    }

    /// Registers or replaces one provider family.
    pub fn register(&mut self, provider_type: String, factory: Arc<dyn NativeAdapterFactory>) {
        self.factories.insert(provider_type, factory);
    }

    /// Returns whether a native family is registered.
    #[must_use]
    pub fn contains(&self, provider_type: &str) -> bool {
        self.factories.contains_key(provider_type)
    }

    /// Builds a registered adapter while secrets remain borrowed from the connection manager.
    ///
    /// # Errors
    /// Returns a typed missing-registration, configuration, or adapter-construction failure.
    pub async fn build(
        &self,
        provider_type: &str,
        auth: &ProviderAuth,
        base_url: Option<&str>,
    ) -> Result<Box<dyn ProviderPort>, ConnectionError> {
        self.factories
            .get(provider_type)
            .ok_or(ConnectionError::NotFound)?
            .build(provider_type, auth, base_url)
            .await
    }
}

struct HttpNativeFactory;

impl NativeAdapterFactory for HttpNativeFactory {
    fn build<'a>(
        &'a self,
        provider_type: &'a str,
        auth: &'a ProviderAuth,
        base_url: Option<&'a str>,
    ) -> NativeFactoryFuture<'a> {
        Box::pin(async move {
            let credential = api_credential(auth)?;
            let endpoint = endpoint(provider_type, base_url)?;
            let adapter: Box<dyn ProviderPort> = if provider_type == "anthropic" {
                Box::new(
                    Anthropic::new(&endpoint, credential).map_err(|_| ConnectionError::Adapter)?,
                )
            } else {
                let field = if provider_type == "openai" {
                    MaxTokensField::MaxCompletionTokens
                } else {
                    MaxTokensField::MaxTokens
                };
                Box::new(
                    OpenAiCompatible::with_capabilities(&endpoint, credential, field, true)
                        .map_err(|_| ConnectionError::Adapter)?,
                )
            };
            Ok(adapter)
        })
    }
}

fn api_credential(auth: &ProviderAuth) -> Result<&str, ConnectionError> {
    match auth {
        ProviderAuth::Api { key, .. } => Ok(key.expose()),
        ProviderAuth::OAuth { access, .. } => Ok(access.expose()),
        ProviderAuth::BedrockProfile { .. } => Err(ConnectionError::Unsupported),
    }
}

fn endpoint(provider_type: &str, configured: Option<&str>) -> Result<Url, ConnectionError> {
    let value = configured.unwrap_or(match provider_type {
        "anthropic" => "https://api.anthropic.com/v1/",
        "ollama" | "ollama-cloud" => "http://localhost:11434/v1/",
        "lmstudio" => "http://127.0.0.1:1234/v1/",
        "llama.cpp" => "http://localhost:8080/v1/",
        _ => "https://api.openai.com/v1/",
    });
    Url::parse(value).map_err(|_| ConnectionError::InvalidInput("provider base URL"))
}
