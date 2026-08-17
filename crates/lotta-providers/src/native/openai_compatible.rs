//! OpenAI-compatible Chat Completions adapter.

use crate::native::{shared, sse::SseParser};
use base64::Engine as _;
use futures_util::StreamExt as _;
use lotta_domain::BoundedJsonValue;
use lotta_runtime::{
    RuntimeError,
    boundary::{ProviderEventText, ProviderName, ToolArgumentChunk},
    ports::{
        ProviderContentPart, ProviderError, ProviderEvent, ProviderEventSink, ProviderMessageRole,
        ProviderMetadata, ProviderMetadataInput, ProviderPort, ProviderRequest, ProviderToolChoice,
        ProviderUsage, StopReason, ToolCallId,
    },
};
use reqwest::{
    Client, StatusCode, Url,
    header::{HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tokio::time::Instant;

const CHAT_COMPLETIONS_PATH: &str = "chat/completions";

/// Maximum-token field used by an OpenAI-compatible endpoint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MaxTokensField {
    /// Serialize the broadly compatible `max_tokens` field.
    #[default]
    MaxTokens,
    /// Serialize `OpenAI`'s newer `max_completion_tokens` field.
    MaxCompletionTokens,
}

/// OpenAI-compatible single-attempt HTTP/SSE adapter.
#[derive(Clone)]
pub struct OpenAiCompatible {
    client: Client,
    endpoint: Url,
    credential: Option<HeaderValue>,
    max_tokens_field: MaxTokensField,
    supports_images: bool,
}

impl std::fmt::Debug for OpenAiCompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatible")
            .field("endpoint", &self.endpoint)
            .field("credential", &"[REDACTED]")
            .field("max_tokens_field", &self.max_tokens_field)
            .field("supports_images", &self.supports_images)
            .field("client", &"reqwest::Client")
            .finish()
    }
}

impl OpenAiCompatible {
    /// Builds an image-capable adapter using the compatible maximum-token field.
    ///
    /// Empty credentials are accepted for keyless local endpoints and omit authorization.
    ///
    /// # Errors
    /// Rejects insecure non-loopback endpoints or invalid client configuration.
    pub fn new(endpoint: &Url, credential: &str) -> Result<Self, RuntimeError> {
        Self::with_capabilities(endpoint, credential, MaxTokensField::MaxTokens, true)
    }

    /// Builds an adapter with explicit image capability and maximum-token compatibility.
    ///
    /// # Errors
    /// Rejects insecure non-loopback endpoints or invalid client configuration.
    pub fn with_capabilities(
        endpoint: &Url,
        credential: &str,
        max_tokens_field: MaxTokensField,
        supports_images: bool,
    ) -> Result<Self, RuntimeError> {
        if !shared::endpoint_allowed(endpoint, !credential.is_empty()) {
            return Err(invalid(
                "provider endpoint requires HTTPS or an uncredentialed LAN URL",
            ));
        }
        let endpoint = shared::endpoint(endpoint, CHAT_COMPLETIONS_PATH);
        let credential = if credential.is_empty() {
            None
        } else {
            let mut value =
                HeaderValue::from_str(credential).map_err(|_| invalid("provider credential"))?;
            value.set_sensitive(true);
            Some(value)
        };
        let client = Client::builder()
            .no_proxy()
            .https_only(endpoint.scheme() == "https")
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| invalid("provider client"))?;
        Ok(Self {
            client,
            endpoint,
            credential,
            max_tokens_field,
            supports_images,
        })
    }
}

fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}

fn provider_text(value: &str) -> Result<ProviderEventText, RuntimeError> {
    ProviderEventText::new(value.to_owned())
}

fn call_id(value: &str) -> Result<ToolCallId, RuntimeError> {
    Ok(ToolCallId::from_name(ProviderName::new(value.to_owned())?))
}

async fn send_error(events: &ProviderEventSink, error: ProviderError) -> Result<(), RuntimeError> {
    events.send(ProviderEvent::Error { error }).await
}

impl ProviderPort for OpenAiCompatible {
    fn stream(
        &self,
        request: ProviderRequest,
        events: ProviderEventSink,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        Box::pin(async move { stream_openai(self, request, events).await })
    }
}

async fn stream_openai(
    adapter: &OpenAiCompatible,
    request: ProviderRequest,
    events: ProviderEventSink,
) -> Result<(), RuntimeError> {
    request.validate_bytes()?;
    let body = map_request(adapter, &request)?;
    let deadline = Instant::now() + request.deadline.get();
    let mut builder = adapter
        .client
        .post(adapter.endpoint.clone())
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&body);
    if let Some(credential) = &adapter.credential {
        builder = builder.header(reqwest::header::AUTHORIZATION, bearer(credential)?);
    }
    let Some(response) = await_response(builder.send(), &request, deadline, &events).await? else {
        return Ok(());
    };
    if !response.status().is_success() {
        return response_error(response, &request, deadline, &events).await;
    }
    consume_openai(response, &request, deadline, &events).await
}

fn bearer(credential: &HeaderValue) -> Result<HeaderValue, RuntimeError> {
    let text = credential
        .to_str()
        .map_err(|_| invalid("provider credential"))?;
    let mut value = HeaderValue::from_str(&format!("Bearer {text}"))
        .map_err(|_| invalid("provider credential"))?;
    value.set_sensitive(true);
    Ok(value)
}

async fn await_response(
    future: impl Future<Output = Result<reqwest::Response, reqwest::Error>>,
    request: &ProviderRequest,
    deadline: Instant,
    events: &ProviderEventSink,
) -> Result<Option<reqwest::Response>, RuntimeError> {
    tokio::select! {
        biased;
        () = request.cancellation.cancelled() => {
            send_error(events, terminal_error("cancelled", "provider request cancelled")).await?;
            Ok(None)
        }
        result = tokio::time::timeout_at(deadline, future) => match result {
            Err(_) => {
                send_error(events, terminal_error("timeout", "provider request timed out")).await?;
                Ok(None)
            }
            Ok(Err(_)) => {
                send_error(events, transport_error()).await?;
                Ok(None)
            }
            Ok(Ok(response)) => Ok(Some(response)),
        }
    }
}

fn terminal_error(code: &str, message: &str) -> ProviderError {
    shared::map_error(
        StatusCode::REQUEST_TIMEOUT,
        code,
        code,
        message,
        &HeaderMap::new(),
    )
}

fn transport_error() -> ProviderError {
    shared::map_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "unavailable",
        "provider transport unavailable",
        &HeaderMap::new(),
    )
}

async fn response_error(
    response: reqwest::Response,
    request: &ProviderRequest,
    deadline: Instant,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = match read_error_body(response, request, deadline).await {
        Ok(body) => body,
        Err(error) => {
            send_error(events, error).await?;
            return Ok(());
        }
    };
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let object = value.get("error").unwrap_or(&value);
    let error_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("provider_error");
    let code = object
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or(error_type);
    let message = object
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("provider request failed");
    send_error(
        events,
        shared::map_error(status, error_type, code, message, &headers),
    )
    .await
}

async fn read_error_body(
    response: reqwest::Response,
    request: &ProviderRequest,
    deadline: Instant,
) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            biased;
            () = request.cancellation.cancelled() => {
                return Err(terminal_error("cancelled", "provider request cancelled"));
            }
            result = tokio::time::timeout_at(deadline, stream.next()) => result
                .map_err(|_| terminal_error("timeout", "provider request timed out"))?,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| transport_error())?;
        if body.len().saturating_add(chunk.len()) > shared::ERROR_BODY_BYTES_MAX {
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn map_request(
    adapter: &OpenAiCompatible,
    request: &ProviderRequest,
) -> Result<Value, RuntimeError> {
    let mut messages = Vec::new();
    if let Some(system) = &request.system_prompt {
        messages.push(json!({"role": "system", "content": system.as_str()}));
    }
    for message in request.messages.as_slice() {
        messages.push(map_message(message, request, adapter.supports_images)?);
    }
    let mut body = json!({
        "model": shared::model_id(request.model.handle.as_str())
            .map_err(|()| invalid("provider model handle"))?,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    body[match adapter.max_tokens_field {
        MaxTokensField::MaxTokens => "max_tokens",
        MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
    }] = json!(request.output_tokens_max.get());
    if !request.tools.is_empty() {
        body["tools"] = map_tools(request);
        body["tool_choice"] = map_tool_choice(&request.tool_choice);
    }
    Ok(body)
}

fn map_message(
    message: &lotta_runtime::ports::ProviderMessage,
    request: &ProviderRequest,
    supports_images: bool,
) -> Result<Value, RuntimeError> {
    let content = request.content_for_image_support(&message.content, supports_images)?;
    let parts = content.as_slice();
    let mapped = if let [ProviderContentPart::Text(value)] = parts {
        Value::String(value.as_str().to_owned())
    } else {
        map_openai_parts(parts)
    };
    let role = match message.role {
        ProviderMessageRole::User => "user",
        ProviderMessageRole::Assistant => "assistant",
        ProviderMessageRole::Tool => "tool",
    };
    let mut value = json!({"role": role, "content": mapped});
    if let Some(id) = &message.tool_call_id {
        value["tool_call_id"] = json!(id.as_str());
    }
    Ok(value)
}

fn map_tools(request: &ProviderRequest) -> Value {
    request
        .tools
        .as_slice()
        .iter()
        .map(|tool| {
            json!({"type": "function", "function": {
                "name": tool.name.as_str(),
                "description": tool.description.as_str(),
                "parameters": tool.input_schema.as_value(),
            }})
        })
        .collect()
}

fn map_tool_choice(choice: &ProviderToolChoice) -> Value {
    match choice {
        ProviderToolChoice::Auto => json!("auto"),
        ProviderToolChoice::None => json!("none"),
        ProviderToolChoice::Required => json!("required"),
        ProviderToolChoice::Named(name) => {
            json!({"type": "function", "function": {"name": name.as_str()}})
        }
    }
}

fn map_openai_parts(parts: &[ProviderContentPart]) -> Value {
    Value::Array(
        parts
            .iter()
            .map(|part| match part {
                ProviderContentPart::Text(value) => {
                    json!({"type": "text", "text": value.as_str()})
                }
                ProviderContentPart::Image { media_type, bytes } => json!({
                    "type": "image_url",
                    "image_url": {"url": format!(
                        "data:{};base64,{}",
                        media_type.as_str(),
                        base64::engine::general_purpose::STANDARD.encode(bytes.as_slice())
                    )},
                }),
            })
            .collect(),
    )
}

#[derive(Debug, Default)]
pub(super) struct OpenAiState {
    tools: BTreeMap<u64, ToolState>,
    ended_tools: BTreeSet<u64>,
    finish: Option<StopReason>,
    metadata: Option<ProviderMetadata>,
    metadata_emitted: bool,
    terminal: bool,
}

#[derive(Debug)]
struct ToolState {
    call_id: ToolCallId,
    started: bool,
    name: Option<String>,
}

async fn consume_openai(
    response: reqwest::Response,
    request: &ProviderRequest,
    deadline: Instant,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let mut parser = SseParser::new();
    let mut stream = response.bytes_stream();
    let mut state = OpenAiState::default();
    loop {
        let chunk = next_chunk(&mut stream, request, deadline).await;
        match chunk {
            Ok(Some(bytes)) => process_records(parser.push(&bytes), &mut state, events).await?,
            Ok(None) => {
                process_records(parser.finish(), &mut state, events).await?;
                return complete_openai(&mut state, events).await;
            }
            Err(error) => {
                send_error(events, error).await?;
                return Ok(());
            }
        }
        if state.terminal {
            return Ok(());
        }
    }
}

async fn next_chunk(
    stream: &mut (impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin),
    request: &ProviderRequest,
    deadline: Instant,
) -> Result<Option<bytes::Bytes>, ProviderError> {
    tokio::select! {
        biased;
        () = request.cancellation.cancelled() => {
            Err(terminal_error("cancelled", "provider request cancelled"))
        }
        result = tokio::time::timeout_at(deadline, stream.next()) => match result {
            Err(_) => Err(terminal_error("timeout", "provider request timed out")),
            Ok(Some(Err(_))) => Err(transport_error()),
            Ok(value) => Ok(value.transpose().expect("transport error handled")),
        }
    }
}

async fn process_records(
    records: Result<Vec<crate::native::sse::SseEvent>, crate::native::sse::SseError>,
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let Ok(records) = records else {
        send_error(events, protocol_error("provider SSE protocol error")).await?;
        state.terminal = true;
        return Ok(());
    };
    for record in records {
        handle_record(record, state, events).await?;
        if state.terminal {
            break;
        }
    }
    Ok(())
}

async fn handle_record(
    record: crate::native::sse::SseEvent,
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    if state.terminal {
        return Ok(());
    }
    if record.data.trim_start().starts_with("[DONE]") {
        complete_openai(state, events).await?;
        return Ok(());
    }
    let Ok(value) = serde_json::from_str::<Value>(&record.data) else {
        send_error(events, protocol_error("provider stream JSON error")).await?;
        state.terminal = true;
        return Ok(());
    };
    if value.get("error").is_some_and(|error| !error.is_null()) {
        emit_stream_error(&value, state, events).await?;
        return Ok(());
    }
    handle_value(&value, state, events).await
}

fn retain_metadata(value: &Value, state: &mut OpenAiState) -> Result<(), RuntimeError> {
    let fields = [
        ("id", value.get("id")),
        ("system_fingerprint", value.get("system_fingerprint")),
        ("model", value.get("model")),
    ];
    let entries = fields
        .into_iter()
        .filter_map(|(key, value)| {
            value.filter(|value| !value.is_null()).map(|value| {
                Ok(ProviderMetadataInput::Persist {
                    key: ProviderName::new(key.to_owned())?,
                    value: BoundedJsonValue::new(value.clone())
                        .map_err(|_| invalid("provider metadata limit"))?,
                })
            })
        })
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    if !entries.is_empty() {
        state.metadata = Some(ProviderMetadata::classified(entries)?);
    }
    Ok(())
}

async fn emit_metadata(
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    if state.metadata_emitted {
        return Ok(());
    }
    let Some(metadata) = state.metadata.clone() else {
        return Ok(());
    };
    events
        .send(ProviderEvent::ProviderMetadata { metadata })
        .await?;
    state.metadata_emitted = true;
    Ok(())
}

pub(super) async fn handle_value(
    value: &Value,
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    for choice in value
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|choice| choice.get("index").and_then(Value::as_u64).unwrap_or(0) == 0)
    {
        handle_choice(choice, state, events).await?;
    }
    if value.get("id").is_some() && state.metadata.is_none() {
        retain_metadata(value, state)?;
    }
    if state.finish.is_some() && value.get("usage").is_none_or(Value::is_null) {
        emit_metadata(state, events).await?;
    }
    if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
        if state.finish.is_some() && !state.metadata_emitted && value.get("id").is_some() {
            emit_metadata(state, events).await?;
        }
        emit_usage(usage, events).await?;
    }
    Ok(())
}

async fn handle_choice(
    choice: &Value,
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    if state.finish.is_some() {
        return Ok(());
    }
    if let Some(delta) = choice.get("delta") {
        emit_nonempty(delta.get("content"), false, events).await?;
        let reasoning = ["reasoning_content", "reasoning", "reasoning_text"]
            .iter()
            .filter_map(|key| delta.get(*key).and_then(Value::as_str))
            .find(|text| !text.is_empty());
        if let Some(reasoning) = reasoning {
            events
                .send(ProviderEvent::ReasoningDelta {
                    text: provider_text(reasoning)?,
                })
                .await?;
        }
        if let Some(marker) = delta
            .get("redacted_reasoning")
            .and_then(Value::as_str)
            .filter(|marker| !marker.is_empty())
        {
            events
                .send(ProviderEvent::RedactedReasoning {
                    marker: provider_text(marker)?,
                })
                .await?;
        }
        handle_tools(delta, state, events).await?;
    }
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        state.finish = Some(stop_reason(reason));
        // Tool calls close on the finishing choice, before that chunk's usage block.
        end_tools(state, events).await?;
    }
    Ok(())
}

async fn end_tools(
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    for (index, tool) in &state.tools {
        if tool.started && state.ended_tools.insert(*index) {
            events
                .send(ProviderEvent::ToolCallEnd {
                    call_id: tool.call_id.clone(),
                })
                .await?;
        }
    }
    Ok(())
}

async fn emit_nonempty(
    value: Option<&Value>,
    reasoning: bool,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let Some(text) = value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    else {
        return Ok(());
    };
    let event = if reasoning {
        ProviderEvent::ReasoningDelta {
            text: provider_text(text)?,
        }
    } else {
        ProviderEvent::TextDelta {
            text: provider_text(text)?,
        }
    };
    events.send(event).await
}

async fn handle_tools(
    delta: &Value,
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    for tool in delta
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let index = tool.get("index").and_then(Value::as_u64).unwrap_or(0);
        let candidate_id = tool
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let candidate_name = tool
            .pointer("/function/name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty());
        update_tool(index, candidate_id, candidate_name, state)?;
        start_tool_if_ready(index, state, events).await?;
        emit_tool_arguments(index, tool, state, events).await?;
    }
    Ok(())
}

fn update_tool(
    index: u64,
    id: Option<&str>,
    name: Option<&str>,
    state: &mut OpenAiState,
) -> Result<(), RuntimeError> {
    if state.tools.contains_key(&index) {
        let replacement = id
            .filter(|_| !state.tools[&index].started)
            .map(|id| unique_call_id(id, index, state))
            .transpose()?;
        let tool = state.tools.get_mut(&index).expect("checked above");
        if !tool.started {
            if let Some(call_id) = replacement {
                tool.call_id = call_id;
            }
            if let Some(name) = name {
                tool.name = Some(name.to_owned());
            }
        }
        return Ok(());
    }
    let raw_id = id.map_or_else(|| format!("tool-call-{index}"), ToOwned::to_owned);
    let call_id = unique_call_id(&raw_id, index, state)?;
    state.tools.insert(
        index,
        ToolState {
            call_id,
            started: false,
            name: name.map(ToOwned::to_owned),
        },
    );
    Ok(())
}

fn unique_call_id(raw: &str, index: u64, state: &OpenAiState) -> Result<ToolCallId, RuntimeError> {
    let used = |candidate: &str| {
        state
            .tools
            .iter()
            .any(|(other, tool)| *other != index && tool.call_id.as_str() == candidate)
    };
    let candidate = if used(raw) {
        format!("{raw}-{index}")
    } else {
        raw.to_owned()
    };
    call_id(&candidate)
}

async fn start_tool_if_ready(
    index: u64,
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let Some(tool) = state.tools.get_mut(&index) else {
        return Ok(());
    };
    let Some(name) = tool.name.as_deref() else {
        return Ok(());
    };
    if !tool.started {
        events
            .send(ProviderEvent::ToolCallStart {
                call_id: tool.call_id.clone(),
                name: provider_text(name)?,
            })
            .await?;
        tool.started = true;
    }
    Ok(())
}

async fn emit_tool_arguments(
    index: u64,
    value: &Value,
    state: &OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let Some(tool) = state.tools.get(&index).filter(|tool| tool.started) else {
        return Ok(());
    };
    let Some(arguments) = value
        .pointer("/function/arguments")
        .and_then(Value::as_str)
        .filter(|arguments| !arguments.is_empty())
    else {
        return Ok(());
    };
    events
        .send(ProviderEvent::ToolCallArgumentsDelta {
            call_id: tool.call_id.clone(),
            chunk: ToolArgumentChunk::new(arguments.as_bytes().to_vec())?,
        })
        .await
}

async fn complete_openai(
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    if state.terminal {
        return Ok(());
    }
    if state.finish.is_none() {
        send_error(
            events,
            protocol_error("provider stream missing finish reason or [DONE]"),
        )
        .await?;
        state.terminal = true;
        return Ok(());
    }
    end_tools(state, events).await?;
    emit_metadata(state, events).await?;
    events
        .send(ProviderEvent::Stop {
            reason: state.finish.expect("checked finish reason"),
        })
        .await?;
    state.terminal = true;
    Ok(())
}

async fn emit_stream_error(
    value: &Value,
    state: &mut OpenAiState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let error = &value["error"];
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("provider_error");
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or(error_type);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("provider stream error");
    let status = value
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    send_error(
        events,
        shared::map_error(status, error_type, code, message, &HeaderMap::new()),
    )
    .await?;
    state.terminal = true;
    Ok(())
}

fn protocol_error(message: &str) -> ProviderError {
    shared::map_error(
        StatusCode::OK,
        "protocol",
        "protocol",
        message,
        &HeaderMap::new(),
    )
}

async fn emit_usage(value: &Value, events: &ProviderEventSink) -> Result<(), RuntimeError> {
    events
        .send(ProviderEvent::Usage {
            usage: ProviderUsage {
                input_tokens: value
                    .get("prompt_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output_tokens: value
                    .get("completion_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cached_input_tokens: value
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                reasoning_tokens: value
                    .pointer("/completion_tokens_details/reasoning_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            },
        })
        .await
}

fn stop_reason(value: &str) -> StopReason {
    match value {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::OutputLimit,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" => StopReason::ContentFilter,
        _ => StopReason::Other,
    }
}
