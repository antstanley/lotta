//! Anthropic Messages adapter.

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
    header::{ACCEPT, HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tokio::time::Instant;

const MESSAGES_PATH: &str = "messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
const THINKING_OUTPUT_TOKENS_MIN: u64 = 1024;
const THINKING_BUDGET_TOKENS_MINIMAL: u64 = 1024;
const THINKING_BUDGET_TOKENS_LOW: u64 = 2048;
const THINKING_BUDGET_TOKENS_MEDIUM: u64 = 8192;
const THINKING_BUDGET_TOKENS_HIGH: u64 = 16384;
const SIGNATURE_BYTES_MAX: usize = 16 * 1024;
const REDACTED_DATA_BYTES_MAX: usize = 1024 * 1024;
const REDACTED_REASONING_MARKER: &str = "<redacted-reasoning>";

/// Anthropic Messages single-attempt HTTP/SSE adapter.
#[derive(Clone)]
pub struct Anthropic {
    client: Client,
    endpoint: Url,
    credential: HeaderValue,
    supports_images: bool,
}

impl std::fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Anthropic")
            .field("endpoint", &self.endpoint)
            .field("credential", &"[REDACTED]")
            .field("supports_images", &self.supports_images)
            .field("client", &"reqwest::Client")
            .finish()
    }
}

impl Anthropic {
    /// Builds an image-capable Anthropic adapter.
    ///
    /// # Errors
    /// Rejects insecure non-loopback endpoints, empty credentials, or invalid configuration.
    pub fn new(endpoint: &Url, credential: &str) -> Result<Self, RuntimeError> {
        Self::with_image_support(endpoint, credential, true)
    }

    /// Builds an adapter with explicit image capability.
    ///
    /// # Errors
    /// Rejects insecure non-loopback endpoints, empty credentials, or invalid configuration.
    pub fn with_image_support(
        endpoint: &Url,
        credential: &str,
        supports_images: bool,
    ) -> Result<Self, RuntimeError> {
        if !shared::is_secure_endpoint(endpoint) {
            return Err(invalid("provider endpoint requires HTTPS"));
        }
        let endpoint = shared::endpoint(endpoint, MESSAGES_PATH);
        if credential.is_empty() {
            return Err(invalid("provider credential"));
        }
        let mut credential =
            HeaderValue::from_str(credential).map_err(|_| invalid("provider credential"))?;
        credential.set_sensitive(true);
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

impl ProviderPort for Anthropic {
    fn stream(
        &self,
        request: ProviderRequest,
        events: ProviderEventSink,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        Box::pin(async move { stream_anthropic(self, request, events).await })
    }
}

async fn stream_anthropic(
    adapter: &Anthropic,
    request: ProviderRequest,
    events: ProviderEventSink,
) -> Result<(), RuntimeError> {
    request.validate_bytes()?;
    let body = map_request(adapter, &request)?;
    let deadline = Instant::now() + request.deadline.get();
    let future = adapter
        .client
        .post(adapter.endpoint.clone())
        .header("x-api-key", adapter.credential.clone())
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", beta_header(&request))
        .header(ACCEPT, "text/event-stream")
        .json(&body)
        .send();
    let Some(response) = await_response(future, &request, deadline, &events).await? else {
        return Ok(());
    };
    if !response.status().is_success() {
        return response_error(response, &request, deadline, &events).await;
    }
    consume(response, &request, deadline, &events).await
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

fn map_request(adapter: &Anthropic, request: &ProviderRequest) -> Result<Value, RuntimeError> {
    let mut messages = Vec::new();
    for message in request.messages.as_slice() {
        let mapped = map_message(message, request, adapter.supports_images)?;
        append_or_merge_message(&mut messages, mapped);
    }
    let (max_tokens, thinking) = map_thinking(request);
    let mut body = json!({
        "model": shared::model_id(request.model.handle.as_str())
            .map_err(|()| invalid("provider model handle"))?,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": true,
        "thinking": thinking,
    });
    if let Some(system) = &request.system_prompt {
        body["system"] = json!(system.as_str());
    }
    if !request.tools.is_empty() {
        body["tools"] = map_tools(request);
        // Anthropic defaults an omitted `tool_choice` to auto; the baseline omits it.
        if request.tool_choice != ProviderToolChoice::Auto {
            body["tool_choice"] = map_tool_choice(&request.tool_choice);
        }
    }
    Ok(body)
}

fn map_message(
    message: &lotta_runtime::ports::ProviderMessage,
    request: &ProviderRequest,
    supports_images: bool,
) -> Result<Value, RuntimeError> {
    let content = request.content_for_image_support(&message.content, supports_images)?;
    let blocks = content.as_slice().iter().map(map_part).collect::<Vec<_>>();
    if message.role == ProviderMessageRole::Tool {
        let tool_use_id = message
            .tool_call_id
            .as_ref()
            .map_or("tool-call", ToolCallId::as_str);
        return Ok(json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": blocks,
                "is_error": false,
            }],
        }));
    }
    let role = match message.role {
        ProviderMessageRole::Assistant => "assistant",
        ProviderMessageRole::User | ProviderMessageRole::Tool => "user",
    };
    let mapped = if let [ProviderContentPart::Text(value)] = content.as_slice() {
        Value::String(value.as_str().to_owned())
    } else {
        Value::Array(blocks)
    };
    Ok(json!({"role": role, "content": mapped}))
}

fn map_part(part: &ProviderContentPart) -> Value {
    match part {
        ProviderContentPart::Text(value) => json!({"type": "text", "text": value.as_str()}),
        ProviderContentPart::Image { media_type, bytes } => json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type.as_str(),
                "data": base64::engine::general_purpose::STANDARD.encode(bytes.as_slice()),
            },
        }),
    }
}

fn append_or_merge_message(messages: &mut Vec<Value>, mut message: Value) {
    let merge = message["role"] == "user"
        && message["content"]
            .as_array()
            .is_some_and(|blocks| blocks.iter().all(|block| block["type"] == "tool_result"))
        && messages.last().is_some_and(|prior| {
            prior["role"] == "user"
                && prior["content"]
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().all(|block| block["type"] == "tool_result"))
        });
    if merge {
        if let (Some(target), Some(source)) = (
            messages
                .last_mut()
                .and_then(|value| value["content"].as_array_mut()),
            message["content"].as_array_mut(),
        ) {
            target.append(source);
        }
    } else {
        messages.push(message);
    }
}

fn map_tools(request: &ProviderRequest) -> Value {
    request
        .tools
        .as_slice()
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name.as_str(),
                "description": tool.description.as_str(),
                "input_schema": tool.input_schema.as_value(),
            })
        })
        .collect()
}

fn map_tool_choice(choice: &ProviderToolChoice) -> Value {
    match choice {
        ProviderToolChoice::Auto => json!({"type": "auto"}),
        ProviderToolChoice::None => json!({"type": "none"}),
        ProviderToolChoice::Required => json!({"type": "any"}),
        ProviderToolChoice::Named(name) => json!({"type": "tool", "name": name.as_str()}),
    }
}

fn beta_header(request: &ProviderRequest) -> String {
    if request.tools.is_empty() {
        INTERLEAVED_THINKING_BETA.to_owned()
    } else {
        format!("{FINE_GRAINED_TOOL_STREAMING_BETA},{INTERLEAVED_THINKING_BETA}")
    }
}

fn map_thinking(request: &ProviderRequest) -> (u64, Value) {
    let output = request.output_tokens_max.get();
    if !request.reasoning.enabled {
        return (output, json!({"type": "disabled"}));
    }
    let requested = match request.reasoning.effort.as_ref().map(ProviderName::as_str) {
        Some("minimal") => THINKING_BUDGET_TOKENS_MINIMAL,
        Some("low") => THINKING_BUDGET_TOKENS_LOW,
        Some("high" | "xhigh") => THINKING_BUDGET_TOKENS_HIGH,
        _ => THINKING_BUDGET_TOKENS_MEDIUM,
    };
    let maximum = output.saturating_sub(THINKING_OUTPUT_TOKENS_MIN);
    let budget = requested.min(maximum);
    (
        output.max(1),
        json!({"type": "enabled", "budget_tokens": budget}),
    )
}

#[derive(Debug, Default)]
pub(super) struct AnthropicState {
    calls: BTreeMap<u64, ToolCallId>,
    ended: BTreeSet<u64>,
    input_tokens: u64,
    cached_tokens: u64,
    reasoning_tokens: u64,
    message_id: Option<String>,
    message_model: Option<String>,
    metadata_emitted: bool,
    pub(super) signatures: BTreeMap<u64, String>,
    stop_reason: Option<StopReason>,
    terminal: bool,
}

async fn consume(
    response: reqwest::Response,
    request: &ProviderRequest,
    deadline: Instant,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let mut parser = SseParser::new();
    let mut stream = response.bytes_stream();
    let mut state = AnthropicState::default();
    loop {
        let chunk = next_chunk(&mut stream, request, deadline).await;
        match chunk {
            Ok(Some(bytes)) => process_records(parser.push(&bytes), &mut state, events).await?,
            Ok(None) => {
                process_records(parser.finish(), &mut state, events).await?;
                return finish_stream(&mut state, events).await;
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
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let Ok(records) = records else {
        send_error(events, protocol_error("provider SSE protocol error")).await?;
        state.terminal = true;
        return Ok(());
    };
    for record in records {
        handle_record(&record, state, events).await?;
        if state.terminal {
            break;
        }
    }
    Ok(())
}

async fn handle_record(
    record: &crate::native::sse::SseEvent,
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    if state.terminal {
        return Ok(());
    }
    let Ok(value) = serde_json::from_str::<Value>(&record.data) else {
        send_error(events, protocol_error("provider stream JSON error")).await?;
        state.terminal = true;
        return Ok(());
    };
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .or(record.event.as_deref())
        .unwrap_or("");
    handle_event(event_type, &value, state, events).await
}

pub(super) async fn handle_event(
    event_type: &str,
    value: &Value,
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    match event_type {
        "message_start" => handle_message_start(value, state),
        "content_block_start" => handle_block_start(value, state, events).await?,
        "content_block_delta" => handle_block_delta(value, state, events).await?,
        "content_block_stop" => handle_block_stop(value, state, events).await?,
        "message_delta" => handle_message_delta(value, state, events).await?,
        "message_stop" => complete(state, events).await?,
        "error"
        | "api_error"
        | "authentication_error"
        | "billing_error"
        | "invalid_request_error"
        | "not_found_error"
        | "overloaded_error"
        | "permission_error"
        | "rate_limit_error"
        | "request_too_large" => {
            emit_stream_error(event_type, value, state, events).await?;
        }
        "ping" => {}
        _ if event_type.ends_with("_error") => {
            emit_stream_error(event_type, value, state, events).await?;
        }
        _ => {}
    }
    Ok(())
}

fn handle_message_start(value: &Value, state: &mut AnthropicState) {
    let usage = value.pointer("/message/usage").unwrap_or(&Value::Null);
    state.input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    state.cached_tokens = cache_tokens(usage);
    state.message_id = value
        .pointer("/message/id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    state.message_model = value
        .pointer("/message/model")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
}

async fn emit_message_metadata(
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    if state.metadata_emitted {
        return Ok(());
    }
    let entries = [
        ("id", state.message_id.as_deref()),
        ("model", state.message_model.as_deref()),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|value| (key, value)))
    .map(|(key, value)| {
        Ok(ProviderMetadataInput::Persist {
            key: ProviderName::new(key.to_owned())?,
            value: BoundedJsonValue::new(json!(value))
                .map_err(|_| invalid("provider metadata limit"))?,
        })
    })
    .collect::<Result<Vec<_>, RuntimeError>>()?;
    if !entries.is_empty() {
        events
            .send(ProviderEvent::ProviderMetadata {
                metadata: ProviderMetadata::classified(entries)?,
            })
            .await?;
        state.metadata_emitted = true;
    }
    Ok(())
}

async fn handle_block_start(
    value: &Value,
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
    match value.pointer("/content_block/type").and_then(Value::as_str) {
        Some("redacted_thinking") => {
            let data = value
                .pointer("/content_block/data")
                .and_then(Value::as_str)
                .unwrap_or("");
            if data.len() > REDACTED_DATA_BYTES_MAX {
                return Err(invalid("provider redacted metadata limit"));
            }
            events
                .send(ProviderEvent::RedactedReasoning {
                    marker: provider_text(REDACTED_REASONING_MARKER)?,
                })
                .await?;
        }
        Some("tool_use") => {
            let id = value
                .pointer("/content_block/id")
                .and_then(Value::as_str)
                .unwrap_or("tool-call");
            let name = value
                .pointer("/content_block/name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let id = call_id(id)?;
            events
                .send(ProviderEvent::ToolCallStart {
                    call_id: id.clone(),
                    name: provider_text(name)?,
                })
                .await?;
            state.calls.insert(index, id);
        }
        _ => {}
    }
    Ok(())
}

async fn handle_block_delta(
    value: &Value,
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    match value.pointer("/delta/type").and_then(Value::as_str) {
        Some("text_delta") => emit_text(value, "/delta/text", false, events).await?,
        // Reasoning tokens come only from provider-reported usage; deltas are not tokens.
        Some("thinking_delta") => emit_text(value, "/delta/thinking", true, events).await?,
        Some("signature_delta") => retain_signature(value, state)?,
        Some("input_json_delta") => emit_arguments(value, state, events).await?,
        _ => {}
    }
    Ok(())
}

async fn emit_text(
    value: &Value,
    pointer: &str,
    reasoning: bool,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let Some(text) = value
        .pointer(pointer)
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

fn retain_signature(value: &Value, state: &mut AnthropicState) -> Result<(), RuntimeError> {
    let Some(signature) = value.pointer("/delta/signature").and_then(Value::as_str) else {
        return Ok(());
    };
    if signature.len() > SIGNATURE_BYTES_MAX {
        return Err(invalid("provider signature metadata limit"));
    }
    let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
    state.signatures.insert(index, signature.to_owned());
    Ok(())
}

async fn emit_arguments(
    value: &Value,
    state: &AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
    let Some(call_id) = state.calls.get(&index) else {
        return Ok(());
    };
    let Some(arguments) = value
        .pointer("/delta/partial_json")
        .and_then(Value::as_str)
        .filter(|arguments| !arguments.is_empty())
    else {
        return Ok(());
    };
    events
        .send(ProviderEvent::ToolCallArgumentsDelta {
            call_id: call_id.clone(),
            chunk: ToolArgumentChunk::new(arguments.as_bytes().to_vec())?,
        })
        .await
}

async fn handle_block_stop(
    value: &Value,
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
    if let Some(signature) = state.signatures.remove(&index) {
        let _ = ProviderMetadata::classified([ProviderMetadataInput::Secret {
            key: ProviderName::new("signature".to_owned())?,
            value: BoundedJsonValue::new(json!(signature))
                .map_err(|_| invalid("provider metadata limit"))?,
        }])?;
    }
    if let Some(call_id) = state.calls.get(&index)
        && state.ended.insert(index)
    {
        events
            .send(ProviderEvent::ToolCallEnd {
                call_id: call_id.clone(),
            })
            .await?;
    }
    Ok(())
}

async fn handle_message_delta(
    value: &Value,
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let usage = value.get("usage").unwrap_or(&Value::Null);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    state.cached_tokens = state.cached_tokens.max(cache_tokens(usage));
    if output == 0 {
        return Ok(());
    }
    state.reasoning_tokens = usage
        .get("thinking_tokens")
        .or_else(|| usage.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(state.reasoning_tokens);
    events
        .send(ProviderEvent::Usage {
            usage: ProviderUsage {
                input_tokens: state.input_tokens,
                output_tokens: output,
                cached_input_tokens: state.cached_tokens.min(state.input_tokens),
                reasoning_tokens: state.reasoning_tokens.min(output),
            },
        })
        .await?;
    if let Some(reason) = value.pointer("/delta/stop_reason").and_then(Value::as_str) {
        state.stop_reason = Some(stop_reason(reason));
    }
    if state.stop_reason.is_none() {
        emit_message_metadata(state, events).await?;
    }
    Ok(())
}

fn cache_tokens(value: &Value) -> u64 {
    value
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(
            value
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
}

async fn complete(
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    if state.terminal {
        return Ok(());
    }
    emit_message_metadata(state, events).await?;
    events
        .send(ProviderEvent::Stop {
            reason: state.stop_reason.unwrap_or(StopReason::EndTurn),
        })
        .await?;
    state.terminal = true;
    Ok(())
}

async fn finish_stream(
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    if state.terminal {
        Ok(())
    } else {
        send_error(
            events,
            protocol_error("provider stream missing message_stop"),
        )
        .await?;
        state.terminal = true;
        Ok(())
    }
}

async fn emit_stream_error(
    event_type: &str,
    value: &Value,
    state: &mut AnthropicState,
    events: &ProviderEventSink,
) -> Result<(), RuntimeError> {
    let error = value.get("error").unwrap_or(value);
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(event_type);
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
        .or_else(|| error.get("status"))
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or_else(|| status_for_error(error_type));
    send_error(
        events,
        shared::map_error(status, error_type, code, message, &HeaderMap::new()),
    )
    .await?;
    state.terminal = true;
    Ok(())
}

fn status_for_error(error_type: &str) -> StatusCode {
    match error_type {
        "authentication_error" => StatusCode::UNAUTHORIZED,
        "permission_error" => StatusCode::FORBIDDEN,
        "billing_error" => StatusCode::PAYMENT_REQUIRED,
        "invalid_request_error" => StatusCode::BAD_REQUEST,
        "not_found_error" => StatusCode::NOT_FOUND,
        "request_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
        "rate_limit_error" => StatusCode::TOO_MANY_REQUESTS,
        "overloaded_error" => StatusCode::from_u16(529).expect("valid status"),
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
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

fn stop_reason(value: &str) -> StopReason {
    match value {
        "end_turn" | "stop_sequence" | "refusal" => StopReason::EndTurn,
        "max_tokens" | "model_context_window_exceeded" => StopReason::OutputLimit,
        "tool_use" => StopReason::ToolUse,
        _ => StopReason::Other,
    }
}
