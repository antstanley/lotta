//! Sanitized provider-stream fixture corpus and replay support.

use super::ProviderFixtureError;
use super::types::{
    BaselineEventRecord, ContentFixture, FixtureEventRecord, ImagePolicyFixture, MessageFixture,
    MetadataEntryFixture, ProviderErrorKind, RequestFixture, RetryAfterFixture, RoleFixture,
    StopReasonFixture, TRACE_EVENTS_MAX, ToolChoiceFixture, ToolFixture,
};
use lotta_domain::{BoundedJsonValue, BoundedVec};
use lotta_runtime::boundary::{
    ProviderEventText, ProviderImageBytes, ProviderName, ProviderText, ToolArgumentChunk,
};
use lotta_runtime::ports::{
    ImagePolicy, ProviderContent, ProviderContentPart, ProviderDeadline, ProviderError,
    ProviderErrorContext, ProviderEvent, ProviderMessage, ProviderMessageRole, ProviderMessages,
    ProviderMetadata, ProviderMetadataInput, ProviderRequest, ProviderToolChoice,
    ProviderToolDefinition, ProviderTools, ProviderUsage, ReasoningControls, StopReason,
    TokenLimit, ToolCallAccumulator, ToolCallId,
};
use lotta_runtime::retry::RetryAfter;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) fn convert_request(
    value: RequestFixture,
) -> Result<ProviderRequest, ProviderFixtureError> {
    let messages = value
        .messages
        .as_slice()
        .iter()
        .cloned()
        .map(convert_message)
        .collect::<Result<Vec<_>, _>>()?;
    let tools = value
        .tools
        .as_slice()
        .iter()
        .cloned()
        .map(convert_tool)
        .collect::<Result<Vec<_>, _>>()?;
    let request = ProviderRequest {
        model: value.model,
        system_prompt: value.system.map(ProviderText::new).transpose()?,
        messages: ProviderMessages::new(messages)?,
        tools: ProviderTools::new(tools)?,
        tool_choice: match value.tool_choice {
            ToolChoiceFixture::Auto => ProviderToolChoice::Auto,
            ToolChoiceFixture::None => ProviderToolChoice::None,
            ToolChoiceFixture::Required => ProviderToolChoice::Required,
        },
        image_policy: match value.image_policy {
            ImagePolicyFixture::Strict => ImagePolicy::Strict,
            ImagePolicyFixture::Drop => ImagePolicy::Drop,
        },
        context_tokens_max: TokenLimit::new(value.context_tokens_max)?,
        output_tokens_max: TokenLimit::new(value.output_tokens_max)?,
        reasoning: ReasoningControls {
            enabled: value.reasoning.enabled,
            effort: value.reasoning.effort.map(ProviderName::new).transpose()?,
            tier: value.reasoning.tier.map(ProviderName::new).transpose()?,
        },
        cancellation: CancellationToken::new(),
        context: None,
        deadline: ProviderDeadline::new(Duration::from_millis(value.deadline_ms))?,
    };
    request.validate_bytes()?;
    Ok(request)
}
fn convert_message(value: MessageFixture) -> Result<ProviderMessage, ProviderFixtureError> {
    let parts = value
        .content
        .as_slice()
        .iter()
        .cloned()
        .map(convert_content)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProviderMessage {
        role: match value.role {
            RoleFixture::User => ProviderMessageRole::User,
            RoleFixture::Assistant => ProviderMessageRole::Assistant,
            RoleFixture::Tool => ProviderMessageRole::Tool,
        },
        content: ProviderContent::new(parts)?,
        tool_call_id: value.tool_call_id.map(call_id).transpose()?,
    })
}
fn convert_content(value: ContentFixture) -> Result<ProviderContentPart, ProviderFixtureError> {
    match value {
        ContentFixture::Text { text } => Ok(ProviderContentPart::Text(ProviderText::new(text)?)),
        ContentFixture::Image { media_type, base64 } => Ok(ProviderContentPart::Image {
            media_type: ProviderName::new(media_type)?,
            bytes: ProviderImageBytes::new(decode_base64(&base64)?)?,
        }),
    }
}
fn convert_tool(value: ToolFixture) -> Result<ProviderToolDefinition, ProviderFixtureError> {
    Ok(ProviderToolDefinition {
        name: ProviderName::new(value.name)?,
        description: ProviderText::new(value.description)?,
        input_schema: value.input_schema,
    })
}
pub(super) fn decode_base64(value: &str) -> Result<Vec<u8>, ProviderFixtureError> {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.len().is_multiple_of(4) {
        return Err(ProviderFixtureError::Semantic("base64 length"));
    }
    let padding =
        usize::from(bytes[bytes.len() - 1] == b'=') + usize::from(bytes[bytes.len() - 2] == b'=');
    if padding > 2 || bytes[..bytes.len() - padding].contains(&b'=') {
        return Err(ProviderFixtureError::Semantic("base64 padding"));
    }
    let output_len = bytes
        .len()
        .checked_div(4)
        .and_then(|n| n.checked_mul(3))
        .and_then(|n| n.checked_sub(padding))
        .ok_or(ProviderFixtureError::TraceLimit)?;
    if output_len > lotta_runtime::bounds::TOOL_ARGUMENT_BYTES_MAX.value {
        return Err(ProviderFixtureError::TraceLimit);
    }
    let mut output = Vec::with_capacity(output_len);
    for (index, block) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let final_block = index + 1 == bytes.len() / 4;
        if (block[2] == b'=' || block[3] == b'=') && !final_block {
            return Err(ProviderFixtureError::Semantic("base64 early padding"));
        }
        let a = base64_digit(block[0])?;
        let b = base64_digit(block[1])?;
        let c = if block[2] == b'=' {
            0
        } else {
            base64_digit(block[2])?
        };
        let d = if block[3] == b'=' {
            0
        } else {
            base64_digit(block[3])?
        };
        if block[2] == b'=' && (block[3] != b'=' || b & 0x0f != 0) {
            return Err(ProviderFixtureError::Semantic("base64 canonical bits"));
        }
        if block[3] == b'=' && block[2] != b'=' && c & 0x03 != 0 {
            return Err(ProviderFixtureError::Semantic("base64 canonical bits"));
        }
        output.push((a << 2) | (b >> 4));
        if block[2] != b'=' {
            output.push((b << 4) | (c >> 2));
        }
        if block[3] != b'=' {
            output.push((c << 6) | d);
        }
    }
    if output.len() != output_len {
        return Err(ProviderFixtureError::Semantic("base64 output"));
    }
    Ok(output)
}
fn base64_digit(value: u8) -> Result<u8, ProviderFixtureError> {
    match value {
        b'A'..=b'Z' => Ok(value - b'A'),
        b'a'..=b'z' => Ok(value - b'a' + 26),
        b'0'..=b'9' => Ok(value - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(ProviderFixtureError::Semantic("base64 alphabet")),
    }
}
pub(super) fn convert_trace(
    values: &[FixtureEventRecord],
) -> Result<BoundedVec<ProviderEvent, TRACE_EVENTS_MAX>, ProviderFixtureError> {
    let events = values
        .iter()
        .map(convert_event)
        .collect::<Result<Vec<_>, _>>()?;
    BoundedVec::new(events).map_err(|_| ProviderFixtureError::TraceLimit)
}
pub(super) fn normalize_baseline(
    values: &[BaselineEventRecord],
) -> Result<BoundedVec<ProviderEvent, TRACE_EVENTS_MAX>, ProviderFixtureError> {
    let mut events = Vec::new();
    let mut terminal = false;
    let mut tools = std::collections::BTreeMap::<String, Vec<u8>>::new();
    for value in values {
        validate_provenance(value)?;
        if terminal || matches!(value, BaselineEventRecord::LateTextDelta { .. }) {
            continue;
        }
        let event = normalize_event(value, &mut tools, &mut terminal)?;
        if events.len() == TRACE_EVENTS_MAX {
            return Err(ProviderFixtureError::TraceLimit);
        }
        events.push(event);
    }
    BoundedVec::new(events).map_err(|_| ProviderFixtureError::TraceLimit)
}
fn normalize_event(
    value: &BaselineEventRecord,
    tools: &mut std::collections::BTreeMap<String, Vec<u8>>,
    terminal: &mut bool,
) -> Result<ProviderEvent, ProviderFixtureError> {
    match value {
        BaselineEventRecord::ToolcallStart { .. }
        | BaselineEventRecord::ToolcallArgumentsDelta { .. }
        | BaselineEventRecord::ToolcallEnd { .. } => normalize_tool_event(value, tools),
        BaselineEventRecord::Done { .. }
        | BaselineEventRecord::Error { .. }
        | BaselineEventRecord::Cancelled { .. } => normalize_terminal_event(value, terminal),
        _ => normalize_scalar_event(value),
    }
}
fn normalize_tool_event(
    value: &BaselineEventRecord,
    tools: &mut std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<ProviderEvent, ProviderFixtureError> {
    match value {
        BaselineEventRecord::ToolcallStart {
            call_id: id, name, ..
        } => {
            tools.insert(id.clone(), Vec::new());
            Ok(ProviderEvent::ToolCallStart {
                call_id: call_id(id.clone())?,
                name: ProviderEventText::new(name.clone())?,
            })
        }
        BaselineEventRecord::ToolcallArgumentsDelta {
            call_id: id,
            arguments,
            ..
        } => {
            tools
                .get_mut(id)
                .ok_or(ProviderFixtureError::Semantic("baseline tool order"))?
                .extend_from_slice(arguments.as_bytes());
            Ok(ProviderEvent::ToolCallArgumentsDelta {
                call_id: call_id(id.clone())?,
                chunk: ToolArgumentChunk::new(arguments.as_bytes().to_vec())?,
            })
        }
        BaselineEventRecord::ToolcallEnd { call_id: id, .. } => {
            let complete = tools
                .get(id)
                .ok_or(ProviderFixtureError::Semantic("baseline tool end"))?;
            serde_json::from_slice::<BoundedJsonValue>(complete)
                .map_err(|_| ProviderFixtureError::Semantic("baseline tool JSON"))?;
            Ok(ProviderEvent::ToolCallEnd {
                call_id: call_id(id.clone())?,
            })
        }
        _ => unreachable!(),
    }
}
fn normalize_terminal_event(
    value: &BaselineEventRecord,
    terminal: &mut bool,
) -> Result<ProviderEvent, ProviderFixtureError> {
    match value {
        BaselineEventRecord::Done { reason, .. } => Ok(terminal_stop(*reason, terminal)),
        BaselineEventRecord::Error {
            kind,
            code,
            context,
            retry_after,
            ..
        } => terminal_error(*kind, code, context, *retry_after, terminal),
        BaselineEventRecord::Cancelled { code, context, .. } => {
            terminal_error(ProviderErrorKind::Cancelled, code, context, None, terminal)
        }
        _ => unreachable!(),
    }
}
fn normalize_scalar_event(
    value: &BaselineEventRecord,
) -> Result<ProviderEvent, ProviderFixtureError> {
    Ok(match value {
        BaselineEventRecord::TextDelta { text, .. } => ProviderEvent::TextDelta {
            text: ProviderEventText::new(text.clone())?,
        },
        BaselineEventRecord::ThinkingDelta { text, .. } => ProviderEvent::ReasoningDelta {
            text: ProviderEventText::new(text.clone())?,
        },
        BaselineEventRecord::RedactedReasoning { marker, .. } => ProviderEvent::RedactedReasoning {
            marker: ProviderEventText::new(marker.clone())?,
        },
        BaselineEventRecord::Usage {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            reasoning_tokens,
            ..
        } => ProviderEvent::Usage {
            usage: ProviderUsage {
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
                cached_input_tokens: *cached_input_tokens,
                reasoning_tokens: *reasoning_tokens,
            },
        },
        BaselineEventRecord::Metadata { entries, .. } => ProviderEvent::ProviderMetadata {
            metadata: classified_entries(entries.as_slice())?,
        },
        _ => unreachable!(),
    })
}
fn terminal_stop(reason: StopReasonFixture, terminal: &mut bool) -> ProviderEvent {
    *terminal = true;
    ProviderEvent::Stop {
        reason: stop(reason),
    }
}
fn terminal_error(
    kind: ProviderErrorKind,
    code: &str,
    context: &str,
    retry_after: Option<RetryAfterFixture>,
    terminal: &mut bool,
) -> Result<ProviderEvent, ProviderFixtureError> {
    *terminal = true;
    Ok(ProviderEvent::Error {
        error: provider_error(kind, code, context, retry_after)?,
    })
}
fn classified_entries(
    entries: &[MetadataEntryFixture],
) -> Result<ProviderMetadata, ProviderFixtureError> {
    ProviderMetadata::classified(entries.iter().map(|entry| ProviderMetadataInput::Persist {
        key: ProviderName::new(entry.key.clone()).expect("validated fixture metadata key"),
        value: entry.value.clone(),
    }))
    .map_err(ProviderFixtureError::Runtime)
}
fn validate_provenance(value: &BaselineEventRecord) -> Result<(), ProviderFixtureError> {
    let provenance = match value {
        BaselineEventRecord::TextDelta { provenance, .. }
        | BaselineEventRecord::ThinkingDelta { provenance, .. }
        | BaselineEventRecord::RedactedReasoning { provenance, .. }
        | BaselineEventRecord::ToolcallStart { provenance, .. }
        | BaselineEventRecord::ToolcallArgumentsDelta { provenance, .. }
        | BaselineEventRecord::ToolcallEnd { provenance, .. }
        | BaselineEventRecord::Usage { provenance, .. }
        | BaselineEventRecord::Metadata { provenance, .. }
        | BaselineEventRecord::Done { provenance, .. }
        | BaselineEventRecord::Error { provenance, .. }
        | BaselineEventRecord::Cancelled { provenance, .. }
        | BaselineEventRecord::LateTextDelta { provenance, .. } => provenance,
    };
    if provenance.source_test.is_empty()
        || provenance.source_symbol.is_empty()
        || provenance.capture_boundary.is_empty()
    {
        Err(ProviderFixtureError::Semantic("baseline provenance"))
    } else {
        Ok(())
    }
}
fn convert_event(value: &FixtureEventRecord) -> Result<ProviderEvent, ProviderFixtureError> {
    Ok(match value {
        FixtureEventRecord::TextDelta { text } => ProviderEvent::TextDelta {
            text: ProviderEventText::new(text.clone())?,
        },
        FixtureEventRecord::ReasoningDelta { text } => ProviderEvent::ReasoningDelta {
            text: ProviderEventText::new(text.clone())?,
        },
        FixtureEventRecord::RedactedReasoning { marker } => ProviderEvent::RedactedReasoning {
            marker: ProviderEventText::new(marker.clone())?,
        },
        FixtureEventRecord::ToolCallStart { call_id: id, name } => ProviderEvent::ToolCallStart {
            call_id: call_id(id.clone())?,
            name: ProviderEventText::new(name.clone())?,
        },
        FixtureEventRecord::ToolCallArgumentsDelta {
            call_id: id,
            bytes_base64,
        } => ProviderEvent::ToolCallArgumentsDelta {
            call_id: call_id(id.clone())?,
            chunk: ToolArgumentChunk::new(decode_base64(bytes_base64)?)?,
        },
        FixtureEventRecord::ToolCallEnd { call_id: id } => ProviderEvent::ToolCallEnd {
            call_id: call_id(id.clone())?,
        },
        FixtureEventRecord::Usage {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            reasoning_tokens,
        } => ProviderEvent::Usage {
            usage: ProviderUsage {
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
                cached_input_tokens: *cached_input_tokens,
                reasoning_tokens: *reasoning_tokens,
            },
        },
        FixtureEventRecord::ProviderMetadata { entries } => ProviderEvent::ProviderMetadata {
            metadata: classified_entries(entries.as_slice())?,
        },
        FixtureEventRecord::Stop { reason } => ProviderEvent::Stop {
            reason: stop(*reason),
        },
        FixtureEventRecord::Error {
            kind,
            code,
            context,
            retry_after,
        } => ProviderEvent::Error {
            error: provider_error(*kind, code, context, *retry_after)?,
        },
    })
}
fn call_id(value: String) -> Result<ToolCallId, ProviderFixtureError> {
    Ok(ToolCallId::from_name(ProviderName::new(value)?))
}
fn stop(value: StopReasonFixture) -> StopReason {
    match value {
        StopReasonFixture::EndTurn => StopReason::EndTurn,
        StopReasonFixture::OutputLimit => StopReason::OutputLimit,
        StopReasonFixture::ToolUse => StopReason::ToolUse,
        StopReasonFixture::ContentFilter => StopReason::ContentFilter,
        StopReasonFixture::Other => StopReason::Other,
    }
}
fn provider_error(
    kind: ProviderErrorKind,
    code: &str,
    context: &str,
    retry_after: Option<RetryAfterFixture>,
) -> Result<ProviderError, ProviderFixtureError> {
    let context = ProviderErrorContext {
        retry_after: retry_after.map(|value| match value {
            RetryAfterFixture::Milliseconds { value } => RetryAfter::Milliseconds(value),
            RetryAfterFixture::DateMilliseconds { value } => RetryAfter::DateMilliseconds(value),
        }),
        code: ProviderName::new(code.to_owned())?,
        context: ProviderEventText::new(context.to_owned())?,
    };
    Ok(match kind {
        ProviderErrorKind::Authentication => ProviderError::Authentication(context),
        ProviderErrorKind::Authorization => ProviderError::Authorization(context),
        ProviderErrorKind::InvalidRequest => ProviderError::InvalidRequest(context),
        ProviderErrorKind::RateLimit => ProviderError::RateLimit(context),
        ProviderErrorKind::Quota => ProviderError::Quota(context),
        ProviderErrorKind::Timeout => ProviderError::Timeout(context),
        ProviderErrorKind::ContextOverflow => ProviderError::ContextOverflow(context),
        ProviderErrorKind::Overloaded => ProviderError::Overloaded(context),
        ProviderErrorKind::Unavailable => ProviderError::Unavailable(context),
        ProviderErrorKind::Protocol => ProviderError::Protocol(context),
        ProviderErrorKind::Cancelled => ProviderError::Cancelled(context),
        ProviderErrorKind::Unknown => ProviderError::Unknown(context),
    })
}
pub(super) fn validate_trace(events: &[ProviderEvent]) -> Result<(), ProviderFixtureError> {
    let mut terminal = false;
    let mut usage = None;
    let mut tools = ToolCallAccumulator::default();
    for event in events {
        if terminal {
            return Err(ProviderFixtureError::Semantic("event after terminal"));
        }
        match event {
            ProviderEvent::ToolCallStart { call_id, .. } => tools.start(call_id.clone())?,
            ProviderEvent::ToolCallArgumentsDelta { call_id, chunk } => {
                tools.append(call_id, chunk.as_slice())?;
            }
            ProviderEvent::ToolCallEnd { call_id } => {
                tools.end(call_id)?;
            }
            ProviderEvent::Usage { usage: next } => {
                if usage.is_some_and(|old: ProviderUsage| !next.is_monotonic_after(old)) {
                    return Err(ProviderFixtureError::Semantic("usage monotonicity"));
                }
                next.checked_total()?;
                usage = Some(*next);
            }
            ProviderEvent::Stop { .. } => {
                tools.terminal_stop()?;
                terminal = true;
            }
            ProviderEvent::Error { .. } => {
                tools.terminal_error();
                terminal = true;
            }
            _ => {}
        }
    }
    if !terminal {
        return Err(ProviderFixtureError::Semantic("missing terminal"));
    }
    Ok(())
}
