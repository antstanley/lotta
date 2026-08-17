//! Typed, bounded endpoint-native model discovery.

use crate::local::common;
use futures_util::StreamExt as _;
use lotta_runtime::{
    RuntimeError,
    boundary::{ProviderEventText, ProviderName},
    ports::{ProviderError, ProviderErrorContext},
};
use reqwest::{Client, Response, StatusCode, Url, header::HeaderValue};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Maximum models accepted from one discovery operation.
pub const DISCOVERY_MODELS_MAX: usize = 1_024;
/// Maximum bytes accepted from one discovery response.
pub const DISCOVERY_RESPONSE_BYTES_MAX: usize = 2 * 1024 * 1024;
/// Maximum concurrent model-detail requests during discovery.
pub const DISCOVERY_REQUESTS_IN_FLIGHT_MAX: usize = 8;

/// Endpoint dialect whose native catalog is queried.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryDialect {
    /// Ollama local daemon.
    Ollama,
    /// Authenticated Ollama Cloud.
    OllamaCloud,
    /// LM Studio native API.
    LmStudio,
    /// llama.cpp native model API.
    LlamaCpp,
}

/// Capabilities reported authoritatively by a local engine.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModelCapabilities {
    /// Input images are accepted.
    pub vision: bool,
    /// Reasoning output is reported separately.
    pub reasoning: bool,
    /// Native tool calls are supported.
    pub tools: bool,
}

/// One bounded model discovered directly from an endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredModel {
    /// Stable endpoint model identifier.
    pub id: ProviderName,
    /// Engine-reported architecture when available.
    pub architecture: Option<String>,
    /// Engine-reported quantization when available.
    pub quantization: Option<String>,
    /// Engine-reported context window when available.
    pub context_window: Option<u64>,
    /// Engine-reported maximum output tokens when available.
    pub max_output_tokens: Option<u64>,
    /// Engine-reported capabilities, conservatively false when unknown.
    pub capabilities: ModelCapabilities,
}

/// Single-deadline native discovery client.
#[derive(Clone)]
pub struct DiscoveryClient {
    client: Client,
    endpoint: Url,
    credential: Option<HeaderValue>,
    dialect: DiscoveryDialect,
    llama_models_path: String,
}

impl std::fmt::Debug for DiscoveryClient {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("DiscoveryClient")
            .field("client", &"reqwest::Client")
            .field("endpoint", &self.endpoint)
            .field("credential", &"[REDACTED]")
            .field("dialect", &self.dialect)
            .field("llama_models_path", &self.llama_models_path)
            .finish()
    }
}

impl DiscoveryClient {
    /// Constructs an endpoint-native discovery client.
    ///
    /// # Errors
    /// Rejects insecure public endpoints, malformed credentials, and keyless Cloud.
    pub fn new(
        endpoint: &Url,
        credential: &str,
        dialect: DiscoveryDialect,
    ) -> Result<Self, RuntimeError> {
        if dialect == DiscoveryDialect::OllamaCloud
            && (endpoint.scheme() != "https" || credential.is_empty() || credential == "not-needed")
        {
            return Err(common::invalid("Ollama Cloud discovery authentication"));
        }
        Ok(Self {
            client: common::client(endpoint, credential)?,
            endpoint: endpoint.clone(),
            credential: common::credential(credential)?,
            dialect,
            llama_models_path: "models".to_owned(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_test_core(
        endpoint: &Url,
        credential: &str,
        dialect: DiscoveryDialect,
    ) -> Result<Self, RuntimeError> {
        Ok(Self {
            client: common::client(endpoint, "")?,
            endpoint: endpoint.clone(),
            credential: common::credential(credential)?,
            dialect,
            llama_models_path: "models".to_owned(),
        })
    }

    /// Pins a llama.cpp native models path (without a leading slash).
    #[must_use]
    pub fn with_llama_models_path(mut self, path: impl Into<String>) -> Self {
        path.into()
            .trim_start_matches('/')
            .clone_into(&mut self.llama_models_path);
        self
    }

    /// Fetches, enriches, bounds, parses, sorts, and deduplicates the native model catalog.
    ///
    /// # Errors
    /// Returns a typed provider error for cancellation, deadline, transport, status, limits,
    /// malformed JSON, or malformed model identifiers.
    pub async fn discover(
        &self,
        cancellation: &CancellationToken,
        deadline: Duration,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        let end = Instant::now() + deadline;
        let models = match self.dialect {
            DiscoveryDialect::Ollama | DiscoveryDialect::OllamaCloud => {
                let tags = self.get_json("api/tags", cancellation, end).await?;
                let mut models = parse_ollama_tags(&tags)?;
                if models.len() > DISCOVERY_MODELS_MAX {
                    return Err(protocol());
                }
                self.enrich_ollama_models(&mut models, cancellation, end)
                    .await?;
                models
            }
            DiscoveryDialect::LmStudio => {
                match self
                    .get_response("api/v0/models", cancellation, end)
                    .await?
                {
                    response if response.status().is_success() => {
                        parse_lmstudio(&read_json(response, cancellation, end).await?)?
                    }
                    response if response.status() == StatusCode::NOT_FOUND => {
                        parse_openai(&self.get_json("v1/models", cancellation, end).await?)?
                    }
                    response => return Err(status(&response)),
                }
            }
            DiscoveryDialect::LlamaCpp => {
                let response = self
                    .get_response(&self.llama_models_path, cancellation, end)
                    .await?;
                let mut models = if response.status().is_success() {
                    parse_llama_cpp(&read_json(response, cancellation, end).await?)?
                } else if response.status() == StatusCode::NOT_FOUND {
                    parse_openai(&self.get_json("v1/models", cancellation, end).await?)?
                } else {
                    return Err(status(&response));
                };
                for model in &mut models {
                    let mut props_url = common::endpoint(&self.endpoint, "props");
                    props_url
                        .query_pairs_mut()
                        .append_pair("model", model.id.as_str());
                    if let Ok(props) = self.get_json_url(props_url, cancellation, end).await {
                        enrich_llama(model, &props);
                    }
                }
                models
            }
        };
        normalize(models)
    }

    async fn get_json(
        &self,
        path: &str,
        cancellation: &CancellationToken,
        end: Instant,
    ) -> Result<Value, ProviderError> {
        self.get_json_url(common::endpoint(&self.endpoint, path), cancellation, end)
            .await
    }

    async fn get_json_url(
        &self,
        url: Url,
        cancellation: &CancellationToken,
        end: Instant,
    ) -> Result<Value, ProviderError> {
        let response = self.send(self.client.get(url), cancellation, end).await?;
        if !response.status().is_success() {
            return Err(status(&response));
        }
        read_json(response, cancellation, end).await
    }

    async fn get_response(
        &self,
        path: &str,
        cancellation: &CancellationToken,
        end: Instant,
    ) -> Result<Response, ProviderError> {
        self.send(
            self.client.get(common::endpoint(&self.endpoint, path)),
            cancellation,
            end,
        )
        .await
    }

    async fn post_json(
        &self,
        path: &str,
        body: &Value,
        cancellation: &CancellationToken,
        end: Instant,
    ) -> Result<Value, ProviderError> {
        let response = self
            .send(
                self.client
                    .post(common::endpoint(&self.endpoint, path))
                    .json(body),
                cancellation,
                end,
            )
            .await?;
        if !response.status().is_success() {
            return Err(status(&response));
        }
        read_json(response, cancellation, end).await
    }

    async fn send(
        &self,
        mut request: reqwest::RequestBuilder,
        cancellation: &CancellationToken,
        end: Instant,
    ) -> Result<Response, ProviderError> {
        if let Some(value) = &self.credential {
            request = request.bearer_auth(value.to_str().map_err(|_| protocol())?);
        }
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(cancelled()),
            result = tokio::time::timeout_at(end, request.send()) => {
                result.map_err(|_| timeout())?.map_err(|_| unavailable())
            }
        }
    }

    async fn enrich_ollama_models(
        &self,
        models: &mut [DiscoveredModel],
        cancellation: &CancellationToken,
        end: Instant,
    ) -> Result<(), ProviderError> {
        let mut tasks = JoinSet::new();
        let mut next = 0;
        while next < models.len() || !tasks.is_empty() {
            while next < models.len() && tasks.len() < DISCOVERY_REQUESTS_IN_FLIGHT_MAX {
                spawn_ollama_show(&mut tasks, self, cancellation, models, next, end);
                next += 1;
            }
            let result = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    tasks.abort_all();
                    return Err(cancelled());
                }
                result = tokio::time::timeout_at(end, tasks.join_next()) => {
                    result.map_err(|_| timeout())?
                }
            };
            if let Some(Ok((index, Ok(shown)))) = result {
                enrich_ollama(&mut models[index], &shown);
            }
        }
        Ok(())
    }
}

fn spawn_ollama_show(
    tasks: &mut JoinSet<(usize, Result<Value, ProviderError>)>,
    client: &DiscoveryClient,
    cancellation: &CancellationToken,
    models: &[DiscoveredModel],
    index: usize,
    end: Instant,
) {
    let client = client.clone();
    let cancellation = cancellation.clone();
    let name = models[index].id.as_str().to_owned();
    tasks.spawn(async move {
        let shown = client
            .post_json("api/show", &json!({"name": name}), &cancellation, end)
            .await;
        (index, shown)
    });
}

async fn read_json(
    response: Response,
    cancellation: &CancellationToken,
    end: Instant,
) -> Result<Value, ProviderError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(cancelled()),
            result = tokio::time::timeout_at(end, stream.next()) => result,
        };
        let Some(chunk) = chunk
            .map_err(|_| timeout())?
            .transpose()
            .map_err(|_| unavailable())?
        else {
            break;
        };
        if body.len().saturating_add(chunk.len()) > DISCOVERY_RESPONSE_BYTES_MAX {
            return Err(protocol());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| protocol())
}

fn normalize(models: Vec<DiscoveredModel>) -> Result<Vec<DiscoveredModel>, ProviderError> {
    if models.len() > DISCOVERY_MODELS_MAX {
        return Err(protocol());
    }
    let mut unique = BTreeMap::new();
    for model in models {
        unique.entry(model.id.as_str().to_owned()).or_insert(model);
    }
    Ok(unique.into_values().collect())
}

fn base_model(id: &str) -> Result<DiscoveredModel, ProviderError> {
    Ok(DiscoveredModel {
        id: ProviderName::new(id.to_owned()).map_err(|_| protocol())?,
        architecture: None,
        quantization: None,
        context_window: None,
        max_output_tokens: None,
        capabilities: ModelCapabilities::default(),
    })
}

fn parse_ollama_tags(value: &Value) -> Result<Vec<DiscoveredModel>, ProviderError> {
    value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(protocol)?
        .iter()
        .filter_map(|item| {
            item.get("name")
                .or_else(|| item.get("model"))
                .and_then(Value::as_str)
        })
        .map(base_model)
        .collect()
}

fn enrich_ollama(model: &mut DiscoveredModel, value: &Value) {
    let info = value.get("model_info").unwrap_or(&Value::Null);
    model.architecture = string(value.pointer("/details/family"))
        .or_else(|| string(info.get("general.architecture")));
    model.quantization = string(value.pointer("/details/quantization_level"));
    model.context_window =
        positive(info.get("context_length")).or_else(|| object_suffix_u64(info, ".context_length"));
    let caps = strings(value.get("capabilities"));
    let template = value
        .get("template")
        .and_then(Value::as_str)
        .unwrap_or_default();
    model.capabilities = ModelCapabilities {
        vision: caps.contains(&"vision")
            || info.get("clip.has_vision_encoder").and_then(Value::as_bool) == Some(true),
        reasoning: caps.contains(&"thinking") || caps.contains(&"reasoning"),
        tools: caps.contains(&"tools") || caps.contains(&"tool") || template.contains(".Tools"),
    };
}

fn parse_lmstudio(value: &Value) -> Result<Vec<DiscoveredModel>, ProviderError> {
    value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(protocol)?
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) != Some("embeddings"))
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(|id| (id, item)))
        .map(|(id, item)| {
            let mut model = base_model(id)?;
            model.architecture = string(item.get("architecture"));
            model.quantization = string(item.get("quantization"));
            let loaded = item.get("state").and_then(Value::as_str) == Some("loaded");
            model.context_window = loaded
                .then(|| positive(item.get("loaded_context_length")))
                .flatten()
                .or_else(|| positive(item.get("max_context_length")));
            model.max_output_tokens = positive(item.get("max_output_tokens"));
            let caps = strings(item.get("capabilities"));
            model.capabilities.vision =
                item.get("type").and_then(Value::as_str) == Some("vlm") || caps.contains(&"vision");
            model.capabilities.reasoning = caps.contains(&"reasoning");
            Ok(model)
        })
        .collect()
}

fn parse_llama_cpp(value: &Value) -> Result<Vec<DiscoveredModel>, ProviderError> {
    value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(protocol)?
        .iter()
        .filter(|item| llama_cpp_model_is_selectable(item))
        .filter_map(|item| {
            item.get("id")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .map(|id| (id, item))
        })
        .map(|(id, item)| {
            let mut model = base_model(id)?;
            enrich_llama(&mut model, item);
            Ok(model)
        })
        .collect()
}

fn llama_cpp_model_is_selectable(item: &Value) -> bool {
    let Some(status) = item.get("status") else {
        return true;
    };
    let Some(status) = status.as_object() else {
        return false;
    };
    match status.get("value").and_then(Value::as_str) {
        None | Some("loaded" | "sleeping") => true,
        Some("unloaded") => status.get("failed").and_then(Value::as_bool) != Some(true),
        Some(_) => false,
    }
}

fn parse_openai(value: &Value) -> Result<Vec<DiscoveredModel>, ProviderError> {
    value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(protocol)?
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(base_model)
        .collect()
}

fn enrich_llama(model: &mut DiscoveredModel, value: &Value) {
    let meta = value.get("meta").unwrap_or(value);
    model.architecture = model
        .architecture
        .take()
        .or_else(|| string(meta.get("architecture")))
        .or_else(|| string(meta.get("model_arch")));
    model.quantization = model
        .quantization
        .take()
        .or_else(|| string(meta.get("quantization")))
        .or_else(|| string(meta.get("quant")));
    model.context_window = model.context_window.or_else(|| llama_context(value, meta));
    model.max_output_tokens = model
        .max_output_tokens
        .or_else(|| positive(meta.get("max_output_tokens")));
    model.capabilities.vision |=
        strings(value.pointer("/architecture/input_modalities")).contains(&"image");
}

fn llama_context(value: &Value, meta: &Value) -> Option<u64> {
    positive(meta.get("n_ctx"))
        .or_else(|| positive(meta.get("n_ctx_train")))
        .or_else(|| {
            positive(
                value
                    .get("default_generation_settings")
                    .and_then(|settings| settings.get("n_ctx")),
            )
        })
}

fn strings(value: Option<&Value>) -> Vec<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}
fn string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(ToOwned::to_owned)
}
fn positive(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64).filter(|value| *value > 0)
}
fn object_suffix_u64(value: &Value, suffix: &str) -> Option<u64> {
    value.as_object()?.iter().find_map(|(key, value)| {
        key.ends_with(suffix)
            .then(|| positive(Some(value)))
            .flatten()
    })
}
fn status(response: &Response) -> ProviderError {
    crate::native::shared::map_error(
        response.status(),
        "http_error",
        "http_error",
        "provider discovery failed",
        response.headers(),
    )
}

fn context(code: &str, message: &str) -> ProviderErrorContext {
    let code = ProviderName::new(code.to_owned()).unwrap_or_else(|_| {
        ProviderName::new("protocol".to_owned())
            .unwrap_or_else(|_| unreachable!("static provider code"))
    });
    let message = ProviderEventText::new(message.to_owned()).unwrap_or_else(|_| {
        ProviderEventText::new("provider error".to_owned())
            .unwrap_or_else(|_| unreachable!("static provider message"))
    });
    ProviderErrorContext::new(code, message)
}
fn cancelled() -> ProviderError {
    ProviderError::Cancelled(context("cancelled", "discovery cancelled"))
}
fn timeout() -> ProviderError {
    ProviderError::Timeout(context("timeout", "discovery timed out"))
}
fn unavailable() -> ProviderError {
    ProviderError::Unavailable(context("unavailable", "endpoint unavailable"))
}
fn protocol() -> ProviderError {
    ProviderError::Protocol(context("protocol", "invalid discovery response"))
}
