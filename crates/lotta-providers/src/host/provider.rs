//! Public single-attempt [`lotta_runtime::ports::ProviderPort`] backed by the pinned child host.

use super::client::{HostClient, HostConfig, HostError};
use super::protocol::{
    HOST_CREDENTIAL_BYTES_MAX, HOST_STREAM_EVENTS_MAX, HostAuth, HostFixtureStream,
    HostInferenceStart, HostOptions,
};
use base64::Engine as _;
use lotta_domain::BoundedJsonValue;
use lotta_extensions::sidecar::SidecarOwnerIdentity;
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{ProviderEventText, ProviderName, ToolArgumentChunk};
use lotta_runtime::ports::{
    ImagePolicy, ProviderContentPart, ProviderError, ProviderErrorContext, ProviderEvent,
    ProviderEventSink, ProviderMessageRole, ProviderMetadata, ProviderMetadataInput, ProviderPort,
    ProviderRequest, ProviderToolChoice, ProviderUsage, StopReason, ToolCallId,
};
use lotta_runtime::retry::RetryAfter;
use serde_json::{Value, json};
use std::fmt;
use std::time::Instant;

/// Explicit deterministic fixture input, available only with a test-mode host configuration.
#[derive(Clone)]
pub struct FixtureInjection {
    /// Fixture dialect understood by the pinned translator.
    pub dialect: String,
    /// Captured response status.
    pub status: u16,
    /// Captured response headers.
    pub headers: Value,
    /// Raw captured stream bytes.
    pub bytes: Vec<u8>,
    /// Exact expected vendor request.
    pub expected_request: Value,
}

/// A pinned host provider with explicit scoped auth and options.
pub struct HostProvider {
    config: HostConfig,
    owner: SidecarOwnerIdentity,
    auth: HostAuth,
    options: HostOptions,
    fixture: Option<FixtureInjection>,
    descriptors: std::sync::RwLock<Vec<Value>>,
}

impl fmt::Debug for HostProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostProvider")
            .field("auth", &"[REDACTED]")
            .field("fixture", &self.fixture.is_some())
            .finish_non_exhaustive()
    }
}

impl HostProvider {
    /// Launches a real pinned child and retains no ambient credential or proxy configuration.
    ///
    /// # Errors
    /// Returns a configuration or child handshake failure.
    pub async fn spawn(
        config: HostConfig,
        owner: SidecarOwnerIdentity,
        auth: HostAuth,
        options: HostOptions,
    ) -> Result<Self, HostError> {
        validate_auth(&auth)?;
        let client = HostClient::spawn(config.clone(), owner.clone()).await?;
        client.shutdown().await?;
        Ok(Self {
            config,
            owner,
            auth,
            options,
            fixture: None,
            descriptors: std::sync::RwLock::new(Vec::new()),
        })
    }

    /// Enables explicit deterministic fixture translation for tests.
    #[must_use]
    pub fn with_fixture(mut self, fixture: FixtureInjection) -> Self {
        self.fixture = Some(fixture);
        self
    }

    /// Atomically registers a trusted bounded pi-ai adapter descriptor in this provider's child.
    ///
    /// # Errors
    /// Returns a typed protocol or ownership/conflict rejection.
    pub async fn register_adapter(&self, descriptor: Value) -> Result<u64, HostError> {
        let mut client = HostClient::spawn(self.config.clone(), self.owner.clone()).await?;
        let revision = client.register(descriptor.clone()).await?;
        client.shutdown().await?;
        self.descriptors
            .write()
            .map_err(|_| HostError::Unavailable)?
            .push(descriptor);
        Ok(revision)
    }

    /// Removes an adapter owned by the exact supplied owner.
    ///
    /// # Errors
    /// Returns a typed ownership or transport rejection.
    pub fn unregister_adapter(&self, id: &str, owner: &str) -> Result<u64, HostError> {
        let mut descriptors = self
            .descriptors
            .write()
            .map_err(|_| HostError::Unavailable)?;
        let position = descriptors
            .iter()
            .position(|value| {
                value["id"].as_str() == Some(id) && value["owner"].as_str() == Some(owner)
            })
            .ok_or(HostError::Rejected)?;
        descriptors.remove(position);
        Ok(u64::try_from(descriptors.len()).unwrap_or(u64::MAX) + 1)
    }

    /// Shuts down and reaps the exact child.
    ///
    /// # Errors
    /// Returns when the child cannot be gracefully reaped.
    pub fn shutdown(self) -> Result<(), HostError> {
        Ok(())
    }
}

impl ProviderPort for HostProvider {
    fn stream(
        &self,
        request: ProviderRequest,
        events: ProviderEventSink,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        Box::pin(async move {
            request.validate_bytes()?;
            let wire = request_wire(&request)?;
            let fixture = self.fixture.as_ref().map(fixture_wire).transpose()?;
            let deadline = request.deadline.get();
            let cancellation = request.cancellation.clone();
            let started = Instant::now();
            let operation = async {
                let mut client = HostClient::spawn(self.config.clone(), self.owner.clone())
                    .await
                    .map_err(|error| host_error(&error))?
                    .with_request_timeout(remaining(deadline, started)?);
                let descriptors = self.descriptors.read().map_err(|_| unavailable())?.clone();
                for descriptor in descriptors {
                    client
                        .register(descriptor)
                        .await
                        .map_err(|error| host_error(&error))?;
                }
                let stream_id = client
                    .inference_start(HostInferenceStart {
                        request: wire,
                        auth: self.auth.clone(),
                        options: self.options.clone(),
                        fixture,
                    })
                    .await
                    .map_err(|error| host_error(&error))?;
                for sequence in 0..HOST_STREAM_EVENTS_MAX as u64 {
                    if events.is_cancelled() {
                        let _ = client.inference_cancel(&stream_id).await;
                        return Err(cancelled());
                    }
                    let payload = client
                        .inference_event(&stream_id, sequence)
                        .await
                        .map_err(|error| host_error(&error))?;
                    let event = decode_event(&payload.event)?;
                    let terminal = matches!(
                        event,
                        ProviderEvent::Stop { .. } | ProviderEvent::Error { .. }
                    );
                    events.send(event).await?;
                    if terminal {
                        return client.shutdown().await.map_err(|error| host_error(&error));
                    }
                }
                let _ = client.inference_cancel(&stream_id).await;
                Err(RuntimeError::LimitExceeded {
                    context: "provider host stream events".into(),
                })
            };
            tokio::select! {
                biased;
                () = cancellation.cancelled() => Err(cancelled()),
                result = tokio::time::timeout(deadline, operation) => match result {
                    Ok(value) => value,
                    Err(_) => Err(RuntimeError::Timeout {
                        context: "provider host inference".into(),
                    }),
                },
            }
        })
    }
}

fn validate_auth(auth: &HostAuth) -> Result<(), HostError> {
    let bytes = serde_json::to_vec(auth).map_err(|_| HostError::Configuration)?;
    if bytes.len() > HOST_CREDENTIAL_BYTES_MAX {
        Err(HostError::Configuration)
    } else {
        Ok(())
    }
}

fn request_wire(request: &ProviderRequest) -> Result<Value, RuntimeError> {
    let model_id = request
        .model
        .handle
        .as_str()
        .strip_prefix(&format!("{}/", request.model.provider_id.as_str()))
        .unwrap_or(request.model.handle.as_str());
    let settings = request
        .model
        .model_settings
        .as_ref()
        .map(|value| serde_json::to_value(value).map_err(|_| invalid("provider model settings")))
        .transpose()?
        .unwrap_or(Value::Null);
    let messages = wire_messages(request)?;
    let tools = wire_tools(request);
    let tool_choice = wire_tool_choice(request);
    Ok(json!({
        "model":{
            "id":model_id,
            "handle":request.model.handle.as_str(),
            "provider":request.model.provider_id.as_str(),
            "context_window":request.model.context_window,
            "settings":settings
        },
        "system":request.system_prompt.as_ref().map(
            lotta_runtime::boundary::ProviderText::as_str
        ),
        "messages":messages,
        "tools":tools,
        "tool_choice":tool_choice,
        "image_policy":match request.image_policy {
            ImagePolicy::Strict => "strict",
            ImagePolicy::Drop => "drop"
        },
        "context_tokens_max":request.context_tokens_max.get(),
        "output_tokens_max":request.output_tokens_max.get(),
        "deadline_ms":u64::try_from(request.deadline.get().as_millis()).unwrap_or(u64::MAX),
        "reasoning":{
            "enabled":request.reasoning.enabled,
            "effort":request.reasoning.effort.as_ref().map(ProviderName::as_str),
            "tier":request.reasoning.tier.as_ref().map(ProviderName::as_str)
        }
    }))
}

fn wire_messages(request: &ProviderRequest) -> Result<Vec<Value>, RuntimeError> {
    request
        .messages
        .as_slice()
        .iter()
        .map(|message| {
            let role = match message.role {
                ProviderMessageRole::User => "user",
                ProviderMessageRole::Assistant => "assistant",
                ProviderMessageRole::Tool => "tool",
            };
            let content = request
                .content_for_image_support(&message.content, true)?
                .as_slice()
                .iter()
                .map(|part| match part {
                    ProviderContentPart::Text(value) => {
                        json!({"type":"text","text":value.as_str()})
                    }
                    ProviderContentPart::Image { media_type, bytes } => json!({
                        "type":"image",
                        "media_type":media_type.as_str(),
                        "base64":base64::engine::general_purpose::STANDARD.encode(bytes.as_slice())
                    }),
                })
                .collect::<Vec<_>>();
            Ok(json!({
                "role":role,
                "content":content,
                "tool_call_id":message.tool_call_id.as_ref().map(ToolCallId::as_str)
            }))
        })
        .collect()
}

fn wire_tools(request: &ProviderRequest) -> Vec<Value> {
    request
        .tools
        .as_slice()
        .iter()
        .map(|tool| {
            json!({
                "name":tool.name.as_str(),
                "description":tool.description.as_str(),
                "input_schema":tool.input_schema.as_value()
            })
        })
        .collect()
}

fn wire_tool_choice(request: &ProviderRequest) -> Value {
    match &request.tool_choice {
        ProviderToolChoice::Auto => json!({"type":"auto"}),
        ProviderToolChoice::None => json!({"type":"none"}),
        ProviderToolChoice::Required => json!({"type":"required"}),
        ProviderToolChoice::Named(name) => json!({"type":"named","name":name.as_str()}),
    }
}

fn fixture_wire(value: &FixtureInjection) -> Result<HostFixtureStream, RuntimeError> {
    if value.bytes.len() > super::protocol::HOST_FIXTURE_BYTES_MAX {
        return Err(RuntimeError::LimitExceeded {
            context: "provider host fixture bytes".into(),
        });
    }
    Ok(HostFixtureStream {
        dialect: value.dialect.clone(),
        status: value.status,
        headers: value.headers.clone(),
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(&value.bytes),
        expected_request: value.expected_request.clone(),
    })
}

pub(super) fn decode_event(value: &Value) -> Result<ProviderEvent, RuntimeError> {
    let kind = value["type"]
        .as_str()
        .ok_or_else(|| invalid("provider host event"))?;
    match kind {
        "TextDelta" => Ok(ProviderEvent::TextDelta {
            text: event_text(&value["text"])?,
        }),
        "ReasoningDelta" => Ok(ProviderEvent::ReasoningDelta {
            text: event_text(&value["text"])?,
        }),
        "RedactedReasoning" => Ok(ProviderEvent::RedactedReasoning {
            marker: event_text(&value["marker"])?,
        }),
        "ToolCallStart" => Ok(ProviderEvent::ToolCallStart {
            call_id: call_id(&value["call_id"])?,
            name: event_text(&value["name"])?,
        }),
        "ToolCallArgumentsDelta" => decode_tool_arguments(value),
        "ToolCallEnd" => Ok(ProviderEvent::ToolCallEnd {
            call_id: call_id(&value["call_id"])?,
        }),
        "Usage" => Ok(ProviderEvent::Usage {
            usage: ProviderUsage {
                input_tokens: number(&value["input_tokens"])?,
                output_tokens: number(&value["output_tokens"])?,
                cached_input_tokens: number(&value["cached_input_tokens"])?,
                reasoning_tokens: number(&value["reasoning_tokens"])?,
            },
        }),
        "ProviderMetadata" => decode_metadata(value),
        "Stop" => Ok(decode_stop(value)),
        "Error" => decode_error(value),
        "FixtureAssertionFailure" => Err(invalid("provider host fixture request assertion")),
        _ => Err(invalid("provider host event type")),
    }
}

fn decode_tool_arguments(value: &Value) -> Result<ProviderEvent, RuntimeError> {
    let bytes = value["bytes_base64"]
        .as_str()
        .ok_or_else(|| invalid("provider host arguments"))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(bytes)
        .map_err(|_| invalid("provider host arguments"))?;
    Ok(ProviderEvent::ToolCallArgumentsDelta {
        call_id: call_id(&value["call_id"])?,
        chunk: ToolArgumentChunk::new(decoded)?,
    })
}

fn decode_metadata(value: &Value) -> Result<ProviderEvent, RuntimeError> {
    let entries = value["entries"]
        .as_array()
        .ok_or_else(|| invalid("provider host metadata"))?;
    let inputs = entries
        .iter()
        .map(|entry| {
            Ok(ProviderMetadataInput::Persist {
                key: ProviderName::new(
                    entry["key"]
                        .as_str()
                        .ok_or_else(|| invalid("provider host metadata"))?
                        .to_owned(),
                )?,
                value: BoundedJsonValue::new(entry["value"].clone())
                    .map_err(|_| invalid("provider host metadata"))?,
            })
        })
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    let metadata = ProviderMetadata::classified(inputs)?;
    Ok(ProviderEvent::ProviderMetadata { metadata })
}

fn decode_stop(value: &Value) -> ProviderEvent {
    let reason = match value["reason"].as_str() {
        Some("end_turn") => StopReason::EndTurn,
        Some("output_limit") => StopReason::OutputLimit,
        Some("tool_use") => StopReason::ToolUse,
        Some("content_filter") => StopReason::ContentFilter,
        _ => StopReason::Other,
    };
    ProviderEvent::Stop { reason }
}

fn decode_error(value: &Value) -> Result<ProviderEvent, RuntimeError> {
    let mut context = ProviderErrorContext::new(
        ProviderName::new(
            value["code"]
                .as_str()
                .ok_or_else(|| invalid("provider host error"))?
                .to_owned(),
        )?,
        event_text(&value["context"])?,
    );
    if let Some(milliseconds) = value["retry_after_ms"].as_u64() {
        context = context.with_retry_after(RetryAfter::Milliseconds(milliseconds));
    }
    let error = match value["kind"].as_str() {
        Some("authentication") => ProviderError::Authentication(context),
        Some("authorization") => ProviderError::Authorization(context),
        Some("invalid_request") => ProviderError::InvalidRequest(context),
        Some("rate_limit") => ProviderError::RateLimit(context),
        Some("quota") => ProviderError::Quota(context),
        Some("timeout") => ProviderError::Timeout(context),
        Some("context_overflow") => ProviderError::ContextOverflow(context),
        Some("overloaded") => ProviderError::Overloaded(context),
        Some("unavailable") => ProviderError::Unavailable(context),
        Some("protocol") => ProviderError::Protocol(context),
        Some("cancelled") => ProviderError::Cancelled(context),
        _ => ProviderError::Unknown(context),
    };
    Ok(ProviderEvent::Error { error })
}

fn event_text(value: &Value) -> Result<ProviderEventText, RuntimeError> {
    ProviderEventText::new(
        value
            .as_str()
            .ok_or_else(|| invalid("provider host text"))?
            .to_owned(),
    )
}
fn call_id(value: &Value) -> Result<ToolCallId, RuntimeError> {
    Ok(ToolCallId::from_name(ProviderName::new(
        value
            .as_str()
            .ok_or_else(|| invalid("provider host call id"))?
            .to_owned(),
    )?))
}
fn number(value: &Value) -> Result<u64, RuntimeError> {
    value.as_u64().ok_or_else(|| invalid("provider host usage"))
}
fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}
fn remaining(
    deadline: std::time::Duration,
    started: Instant,
) -> Result<std::time::Duration, RuntimeError> {
    deadline
        .checked_sub(started.elapsed())
        .filter(|value| !value.is_zero())
        .ok_or(RuntimeError::Timeout {
            context: "provider host inference".into(),
        })
}

fn unavailable() -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "provider_host_unavailable",
        context: "provider host".into(),
    }
}
fn cancelled() -> RuntimeError {
    RuntimeError::Cancelled {
        context: "provider host inference".into(),
    }
}
fn host_error(error: &HostError) -> RuntimeError {
    let code = match error {
        HostError::Configuration => "provider_host_configuration",
        HostError::Unavailable => "provider_host_unavailable",
        HostError::Protocol => "provider_host_protocol",
        HostError::Rejected => "provider_host_rejected",
    };
    RuntimeError::AdapterFailure {
        code,
        context: "provider host inference".into(),
    }
}
