//! LM Studio local endpoint adapter.

use crate::native::openai_compatible::{MaxTokensField, OpenAiCompatible};
use lotta_runtime::{
    RuntimeError,
    ports::{ProviderEventSink, ProviderPort, ProviderRequest},
};
use reqwest::Url;

/// Explicit LM Studio `/v1/chat/completions` adapter.
#[derive(Clone, Debug)]
pub struct LmStudio {
    inner: OpenAiCompatible,
}

impl LmStudio {
    /// Constructs an LM Studio adapter for a local or TLS endpoint.
    ///
    /// # Errors
    /// Rejects invalid or insecure public endpoints and malformed credentials.
    pub fn new(endpoint: &Url, credential: &str) -> Result<Self, RuntimeError> {
        Self::with_image_support(endpoint, credential, false)
    }

    /// Constructs an adapter with explicitly configured, conservatively disabled image support.
    ///
    /// # Errors
    /// Rejects invalid or insecure public endpoints and malformed credentials.
    pub fn with_image_support(
        endpoint: &Url,
        credential: &str,
        supports_images: bool,
    ) -> Result<Self, RuntimeError> {
        let endpoint = crate::local::common::endpoint(endpoint, "v1");
        let credential = if credential == "not-needed" {
            ""
        } else {
            credential
        };
        Ok(Self {
            inner: OpenAiCompatible::with_capabilities(
                &endpoint,
                credential,
                MaxTokensField::MaxTokens,
                supports_images,
            )?,
        })
    }
}

impl ProviderPort for LmStudio {
    fn stream(
        &self,
        request: ProviderRequest,
        events: ProviderEventSink,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        self.inner.stream(request, events)
    }
}
