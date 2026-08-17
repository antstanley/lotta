//! Ollama native JSONL adapters.

use crate::local::common;
use base64::Engine as _;
use futures_util::StreamExt as _;
use lotta_runtime::{
    RuntimeError,
    boundary::{ProviderEventText, ProviderName, ToolArgumentChunk},
    ports::{
        ProviderContentPart, ProviderError, ProviderEvent, ProviderEventSink, ProviderMessageRole,
        ProviderPort, ProviderRequest, ProviderToolChoice, ProviderUsage, StopReason, ToolCallId,
    },
};
use reqwest::{Client, StatusCode, Url, header::HeaderValue};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tokio::time::Instant;

const NDJSON_LINE_BYTES_MAX: usize = 1024 * 1024;

/// Native local Ollama `/api/chat` adapter.
#[derive(Clone)]
pub struct Ollama {
    core: OllamaCore,
}

/// Native authenticated Ollama Cloud `/api/chat` adapter.
#[derive(Clone)]
pub struct OllamaCloud {
    core: OllamaCore,
}

#[derive(Clone)]
struct OllamaCore {
    client: Client,
    endpoint: Url,
    credential: Option<HeaderValue>,
    supports_images: bool,
}

impl std::fmt::Debug for OllamaCore {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("OllamaCore")
            .field("client", &"reqwest::Client")
            .field("endpoint", &self.endpoint)
            .field("credential", &"[REDACTED]")
            .field("supports_images", &self.supports_images)
            .finish()
    }
}

impl std::fmt::Debug for Ollama {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.debug_tuple("Ollama").field(&self.core).finish()
    }
}

impl std::fmt::Debug for OllamaCloud {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.debug_tuple("OllamaCloud").field(&self.core).finish()
    }
}

impl Ollama {
    /// Constructs an Ollama adapter for loopback or a configured LAN daemon.
    ///
    /// # Errors
    /// Rejects public plaintext endpoints and malformed credentials.
    pub fn new(endpoint: &Url, credential: &str) -> Result<Self, RuntimeError> {
        Self::with_image_support(endpoint, credential, false)
    }

    /// Constructs an Ollama adapter with explicitly configured image support.
    ///
    /// # Errors
    /// Rejects public plaintext endpoints and malformed credentials.
    pub fn with_image_support(
        endpoint: &Url,
        credential: &str,
        supports_images: bool,
    ) -> Result<Self, RuntimeError> {
        Ok(Self {
            core: OllamaCore::new(endpoint, credential, supports_images)?,
        })
    }
}

impl OllamaCloud {
    /// Constructs the pinned TLS and authenticated Ollama Cloud adapter.
    ///
    /// # Errors
    /// Rejects non-HTTPS endpoints, empty credentials, or malformed credentials.
    pub fn new(endpoint: &Url, credential: &str) -> Result<Self, RuntimeError> {
        if endpoint.scheme() != "https" || credential.is_empty() || credential == "not-needed" {
            return Err(common::invalid(
                "Ollama Cloud requires HTTPS authentication",
            ));
        }
        Ok(Self {
            core: OllamaCore::new(endpoint, credential, false)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_test_core(
        endpoint: &Url,
        credential: &str,
        supports_images: bool,
    ) -> Result<Self, RuntimeError> {
        let mut core = OllamaCore::new(endpoint, "", supports_images)?;
        core.credential = common::credential(credential)?;
        Ok(Self { core })
    }
}

impl OllamaCore {
    fn new(endpoint: &Url, credential: &str, supports_images: bool) -> Result<Self, RuntimeError> {
        Ok(Self {
            client: common::client(endpoint, credential)?,
            endpoint: common::endpoint(endpoint, "api/chat"),
            credential: common::credential(credential)?,
            supports_images,
        })
    }
}

macro_rules! port {
    ($type:ty) => {
        impl ProviderPort for $type {
            fn stream(
                &self,
                request: ProviderRequest,
                events: ProviderEventSink,
            ) -> lotta_runtime::ports::PortFuture<'_, ()> {
                Box::pin(self.core.stream(request, events))
            }
        }
    };
}
port!(Ollama);
port!(OllamaCloud);

impl OllamaCore {
    async fn stream(
        &self,
        request: ProviderRequest,
        events: ProviderEventSink,
    ) -> Result<(), RuntimeError> {
        request.validate_bytes()?;
        let body = map_request(&request, self.supports_images)?;
        let deadline = Instant::now() + request.deadline.get();
        let mut builder = self.client.post(self.endpoint.clone()).json(&body);
        if let Some(value) = &self.credential {
            builder =
                builder.bearer_auth(value.to_str().map_err(|_| common::invalid("credential"))?);
        }
        let response = tokio::select! {
            biased;
            () = request.cancellation.cancelled() => return cancelled(&events).await,
            value = tokio::time::timeout_at(deadline, builder.send()) => value,
        };
        let response = match response {
            Err(_) => return timeout(&events).await,
            Ok(Err(_)) => return unavailable(&events).await,
            Ok(Ok(value)) => value,
        };
        if !response.status().is_success() {
            return status_error(response, &request, deadline, &events).await;
        }
        consume(response, &request, deadline, &events).await
    }
}

fn map_request(request: &ProviderRequest, supports_images: bool) -> Result<Value, RuntimeError> {
    if matches!(
        request.tool_choice,
        ProviderToolChoice::Required | ProviderToolChoice::Named(_)
    ) {
        return Err(common::invalid("Ollama tool choice unsupported"));
    }
    let model = request
        .model
        .handle
        .as_str()
        .split_once('/')
        .map_or(request.model.handle.as_str(), |(_, id)| id);
    let mut messages = Vec::new();
    if let Some(system) = &request.system_prompt {
        messages.push(json!({"role": "system", "content": system.as_str()}));
    }
    for message in request.messages.as_slice() {
        let content = request.content_for_image_support(&message.content, supports_images)?;
        let text = content
            .as_slice()
            .iter()
            .filter_map(|part| match part {
                ProviderContentPart::Text(value) => Some(value.as_str()),
                ProviderContentPart::Image { .. } => None,
            })
            .collect::<String>();
        let images = content
            .as_slice()
            .iter()
            .filter_map(|part| match part {
                ProviderContentPart::Image { bytes, .. } => {
                    Some(base64::engine::general_purpose::STANDARD.encode(bytes.as_slice()))
                }
                ProviderContentPart::Text(_) => None,
            })
            .collect::<Vec<_>>();
        let role = match message.role {
            ProviderMessageRole::User => "user",
            ProviderMessageRole::Assistant => "assistant",
            ProviderMessageRole::Tool => "tool",
        };
        let mut mapped = json!({"role": role, "content": text});
        if let Some(id) = &message.tool_call_id {
            mapped["tool_call_id"] = json!(id.as_str());
        }
        if !images.is_empty() {
            mapped["images"] = json!(images);
        }
        messages.push(mapped);
    }
    Ok(json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "think": request.reasoning.enabled,
        "tools": request.tools.as_slice().iter().map(|tool| json!({"type": "function", "function": {
            "name": tool.name.as_str(), "description": tool.description.as_str(),
            "parameters": tool.input_schema.as_value()
        }})).collect::<Vec<_>>(),
        "options": {"num_ctx": request.context_tokens_max.get(),
                    "num_predict": request.output_tokens_max.get()}
    }))
}

async fn consume(
    response: reqwest::Response,
    request: &ProviderRequest,
    deadline: Instant,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut state = StreamState::default();
    loop {
        let chunk = tokio::select! {
            biased;
            () = request.cancellation.cancelled() => return cancelled(events).await,
            value = tokio::time::timeout_at(deadline, stream.next()) => value,
        };
        let chunk = match chunk {
            Err(_) => return timeout(events).await,
            Ok(Some(Err(_))) => return unavailable(events).await,
            Ok(Some(Ok(value))) => value,
            Ok(None) => break,
        };
        if buffer.len().saturating_add(chunk.len()) > NDJSON_LINE_BYTES_MAX {
            return protocol(events).await;
        }
        buffer.extend_from_slice(&chunk);
        while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let line = buffer[..end].to_vec();
            buffer.drain(..=end);
            if state.parse_line(&line, events).await? {
                return Ok(());
            }
        }
    }
    if !buffer.is_empty() {
        state.parse_line(&buffer, events).await?;
    }
    if !state.terminal {
        protocol(events).await?;
    }
    Ok(())
}

#[derive(Default)]
struct StreamState {
    calls: BTreeMap<String, ToolCallId>,
    completed_calls: BTreeSet<String>,
    next_call: usize,
    saw_tools: bool,
    terminal: bool,
}

impl StreamState {
    async fn parse_line(
        &mut self,
        line: &[u8],
        events: &ProviderEventSink,
    ) -> Result<bool, RuntimeError> {
        if self.terminal || line.iter().all(u8::is_ascii_whitespace) {
            return Ok(self.terminal);
        }
        let value: Value = if let Ok(value) = serde_json::from_slice(line) {
            value
        } else {
            self.terminal = true;
            protocol(events).await?;
            return Ok(true);
        };
        if let Some(error) = value.get("error") {
            self.terminal = true;
            mapped_error(error, events).await?;
            return Ok(true);
        }
        let message = value.get("message").unwrap_or(&Value::Null);
        let has_usage =
            value.get("prompt_eval_count").is_some() || value.get("eval_count").is_some();
        if let Some(text) = message
            .get("thinking")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            events
                .send(ProviderEvent::ReasoningDelta {
                    text: ProviderEventText::new(text.to_owned())?,
                })
                .await?;
        }
        if let Some(text) = message
            .get("content")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            events
                .send(ProviderEvent::TextDelta {
                    text: ProviderEventText::new(text.to_owned())?,
                })
                .await?;
        }
        self.emit_tools(message, events).await?;
        if has_usage {
            emit_usage(&value, events).await?;
        }
        if value.get("done").and_then(Value::as_bool) == Some(true) {
            events
                .send(ProviderEvent::Stop {
                    reason: if self.saw_tools {
                        StopReason::ToolUse
                    } else {
                        stop_reason(&value)
                    },
                })
                .await?;
            self.terminal = true;
        }
        Ok(self.terminal)
    }

    async fn emit_tools(
        &mut self,
        message: &Value,
        events: &ProviderEventSink,
    ) -> Result<(), RuntimeError> {
        for call in message
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let function = call.get("function").unwrap_or(call);
            let key = tool_key(call, function, &mut self.next_call);
            if self.completed_calls.contains(&key) {
                continue;
            }
            let call_id = tool_id(&key, call, &mut self.calls)?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("tool");
            self.saw_tools = true;
            events
                .send(ProviderEvent::ToolCallStart {
                    call_id: call_id.clone(),
                    name: ProviderEventText::new(name.to_owned())?,
                })
                .await?;
            let arguments =
                serde_json::to_string(function.get("arguments").unwrap_or(&Value::Null))
                    .map_err(|_| common::invalid("tool arguments"))?;
            events
                .send(ProviderEvent::ToolCallArgumentsDelta {
                    call_id: call_id.clone(),
                    chunk: ToolArgumentChunk::new(arguments.into_bytes())?,
                })
                .await?;
            events.send(ProviderEvent::ToolCallEnd { call_id }).await?;
            self.completed_calls.insert(key);
        }
        Ok(())
    }
}

fn tool_key(call: &Value, function: &Value, next_call: &mut usize) -> String {
    if let Some(id) = call.get("id").and_then(Value::as_str) {
        return format!("id:{id}");
    }
    if let Some(index) = function.get("index").and_then(Value::as_u64) {
        return format!("index:{index}");
    }
    let value = format!("call:{next_call}");
    *next_call = next_call.saturating_add(1);
    value
}

fn tool_id(
    key: &str,
    call: &Value,
    calls: &mut BTreeMap<String, ToolCallId>,
) -> Result<ToolCallId, RuntimeError> {
    if let Some(id) = calls.get(key) {
        return Ok(id.clone());
    }
    let value = call
        .get("id")
        .and_then(Value::as_str)
        .map_or_else(|| key.replace(':', "-"), str::to_owned);
    let id = ToolCallId::from_name(ProviderName::new(value)?);
    calls.insert(key.to_owned(), id.clone());
    Ok(id)
}

async fn emit_usage(value: &Value, events: &ProviderEventSink) -> Result<(), RuntimeError> {
    let input_tokens = value
        .get("prompt_eval_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = value.get("eval_count").and_then(Value::as_u64).unwrap_or(0);
    if input_tokens > 0 || output_tokens > 0 {
        events
            .send(ProviderEvent::Usage {
                usage: ProviderUsage {
                    input_tokens,
                    output_tokens,
                    cached_input_tokens: 0,
                    reasoning_tokens: 0,
                },
            })
            .await?;
    }
    Ok(())
}

fn stop_reason(value: &Value) -> StopReason {
    match value.get("done_reason").and_then(Value::as_str) {
        Some("length") => StopReason::OutputLimit,
        Some("tool_calls") => StopReason::ToolUse,
        _ => StopReason::EndTurn,
    }
}

async fn mapped_error(value: &Value, events: &ProviderEventSink) -> Result<(), RuntimeError> {
    let object = value.as_object();
    let kind = object
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("provider_error");
    let code = object
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .unwrap_or(kind);
    let message = value
        .as_str()
        .or_else(|| {
            object
                .and_then(|value| value.get("message"))
                .and_then(Value::as_str)
        })
        .unwrap_or("provider request failed");
    let error = crate::native::shared::map_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        kind,
        code,
        message,
        &reqwest::header::HeaderMap::new(),
    );
    events.send(ProviderEvent::Error { error }).await
}

async fn status_error(
    response: reqwest::Response,
    request: &ProviderRequest,
    deadline: Instant,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let status = response.status();
    let headers = response.headers().clone();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            biased;
            () = request.cancellation.cancelled() => return cancelled(events).await,
            value = tokio::time::timeout_at(deadline, stream.next()) => value,
        };
        let chunk = match chunk {
            Err(_) => return timeout(events).await,
            Ok(Some(Err(_))) => return unavailable(events).await,
            Ok(Some(Ok(value))) => value,
            Ok(None) => break,
        };
        if body.len().saturating_add(chunk.len()) > crate::native::shared::ERROR_BODY_BYTES_MAX {
            break;
        }
        body.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let object = value.get("error").unwrap_or(&value);
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("http_error");
    let code = object.get("code").and_then(Value::as_str).unwrap_or(kind);
    let message = object
        .as_str()
        .or_else(|| object.get("message").and_then(Value::as_str))
        .unwrap_or("provider request failed");
    let error = crate::native::shared::map_error(status, kind, code, message, &headers);
    events.send(ProviderEvent::Error { error }).await
}

async fn unavailable(events: &ProviderEventSink) -> Result<(), RuntimeError> {
    terminal(
        events,
        ProviderError::Unavailable,
        "unavailable",
        "provider transport unavailable",
    )
    .await
}
async fn timeout(events: &ProviderEventSink) -> Result<(), RuntimeError> {
    terminal(
        events,
        ProviderError::Timeout,
        "timeout",
        "provider request timed out",
    )
    .await
}
async fn cancelled(events: &ProviderEventSink) -> Result<(), RuntimeError> {
    terminal(
        events,
        ProviderError::Cancelled,
        "cancelled",
        "provider request cancelled",
    )
    .await
}
async fn protocol(events: &ProviderEventSink) -> Result<(), RuntimeError> {
    terminal(
        events,
        ProviderError::Protocol,
        "protocol",
        "invalid Ollama JSONL",
    )
    .await
}

async fn terminal(
    events: &ProviderEventSink,
    build: fn(lotta_runtime::ports::ProviderErrorContext) -> ProviderError,
    code: &str,
    message: &str,
) -> Result<(), RuntimeError> {
    let context = lotta_runtime::ports::ProviderErrorContext::new(
        ProviderName::new(code.to_owned())?,
        ProviderEventText::new(message.to_owned())?,
    );
    events
        .send(ProviderEvent::Error {
            error: build(context),
        })
        .await
}
