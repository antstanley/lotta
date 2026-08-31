//! Bounded pinned Responses request parsing and input projection.

use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::bounds::HTTP_BODY_BYTES_MAX;

use super::super::chat::{self, TurnMessage, fresh_uuid};

/// Maximum decoded Responses JSON nesting.
pub const OPENAI_RESPONSES_JSON_DEPTH_MAX: usize = 64;
/// Maximum model identifier length.
pub const OPENAI_RESPONSES_MODEL_BYTES_MAX: usize = 1_024;
/// Maximum Responses input items.
pub const OPENAI_RESPONSES_INPUT_ITEMS_MAX: usize = 4_096;
/// Maximum content parts in one input item.
pub const OPENAI_RESPONSES_CONTENT_PARTS_MAX: usize = 4_096;
/// Maximum accepted aggregate projected content.
pub const OPENAI_RESPONSES_CONTENT_BYTES_MAX: usize = HTTP_BODY_BYTES_MAX;
/// Maximum instructions length.
pub const OPENAI_RESPONSES_INSTRUCTIONS_BYTES_MAX: usize = HTTP_BODY_BYTES_MAX;
/// Maximum previous-response identifier length.
pub const OPENAI_RESPONSES_PREVIOUS_ID_BYTES_MAX: usize = 16_384;

#[derive(Deserialize)]
struct RequestWire {
    model: Option<String>,
    input: Option<InputWire>,
    instructions: Option<String>,
    previous_response_id: Option<String>,
    #[serde(default)]
    store: Value,
    #[serde(default)]
    stream: Value,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InputWire {
    Text(String),
    Items(Vec<InputItem>),
}

#[derive(Deserialize)]
struct InputItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    role: Option<String>,
    content: Option<Value>,
}

/// Fully validated route request.
pub struct PreparedRequest {
    /// Advertised model value returned on the wire.
    pub model: String,
    /// Canonical runtime input messages.
    pub messages: Vec<TurnMessage>,
    /// Explicit persistent chat identity.
    pub chat_key: Option<String>,
    /// Opaque prior response identifier, if supplied.
    pub previous_response_id: Option<String>,
    /// Retain a successful conversation.
    pub store: bool,
    /// Select SSE projection.
    pub streaming: bool,
}

/// Stable request-validation failure message.
pub struct InputError(pub String);

/// Parses and validates the exact pinned request subset.
///
/// # Errors
/// Returns a bounded stable invalid-request message.
pub fn prepare(value: Value, headers: &HeaderMap) -> Result<PreparedRequest, InputError> {
    validate_json_shape(&value)?;
    let request: RequestWire =
        serde_json::from_value(value).map_err(|_| InputError("invalid request body".to_owned()))?;
    validate_optional_lengths(&request)?;
    let model = required_model(request.model.clone())?;
    let streaming = request.stream == Value::Bool(true);
    let chat_key = chat::chat_key(headers, streaming)
        .map_err(|_| InputError("invalid request header".to_owned()))?;
    let stateful = chat_key.is_some() || request.previous_response_id.is_some();
    let combined = combined_instructions(&request);
    let previous_response_id = request.previous_response_id.clone();
    let store = request.store == Value::Bool(true);
    let mut messages = normalize_input(request.input, stateful)?;
    apply_instructions(&mut messages, combined.as_deref());
    Ok(PreparedRequest {
        model,
        messages,
        chat_key,
        previous_response_id,
        store,
        streaming,
    })
}

fn required_model(model: Option<String>) -> Result<String, InputError> {
    let model = model
        .filter(|value| !value.is_empty())
        .ok_or_else(|| InputError("you must provide a model parameter".to_owned()))?;
    if model.len() > OPENAI_RESPONSES_MODEL_BYTES_MAX {
        return Err(InputError("model exceeds maximum length".to_owned()));
    }
    Ok(model)
}

fn validate_optional_lengths(request: &RequestWire) -> Result<(), InputError> {
    if request
        .instructions
        .as_ref()
        .is_some_and(|value| value.len() > OPENAI_RESPONSES_INSTRUCTIONS_BYTES_MAX)
    {
        return Err(InputError("instructions exceeds maximum length".to_owned()));
    }
    if request.previous_response_id.as_ref().is_some_and(|value| {
        value.is_empty() || value.len() > OPENAI_RESPONSES_PREVIOUS_ID_BYTES_MAX
    }) {
        return Err(InputError(
            "previous_response_id exceeds maximum length".to_owned(),
        ));
    }
    Ok(())
}

fn combined_instructions(request: &RequestWire) -> Option<String> {
    let mut values = request
        .instructions
        .iter()
        .filter(|text| !text.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if let Some(InputWire::Items(items)) = &request.input {
        values.extend(items.iter().filter_map(|item| {
            matches!(item.role.as_deref(), Some("system" | "developer"))
                .then(|| extract_text_content(item.content.as_ref()))
                .filter(|text| !text.is_empty())
        }));
    }
    (!values.is_empty()).then(|| values.join("\n\n"))
}

fn extract_text_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text" | "input_text" | "output_text") => part.get("text")?.as_str(),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

fn normalize_input(
    input: Option<InputWire>,
    stateful: bool,
) -> Result<Vec<TurnMessage>, InputError> {
    let items = match input {
        Some(InputWire::Text(text)) => vec![InputItem {
            kind: Some("message".to_owned()),
            role: Some("user".to_owned()),
            content: Some(Value::String(text)),
        }],
        Some(InputWire::Items(items)) => items,
        None => Vec::new(),
    };
    if items.len() > OPENAI_RESPONSES_INPUT_ITEMS_MAX {
        return Err(InputError("input exceeds maximum length".to_owned()));
    }
    validate_content_arrays(&items)?;
    let newest = newest_user(&items).ok_or_else(missing_user)?;
    let selected = select_items(&items, newest, stateful);
    validate_content_bytes(&selected)?;
    Ok(selected)
}

fn validate_content_arrays(items: &[InputItem]) -> Result<(), InputError> {
    for item in items {
        if let Some(Value::Array(parts)) = &item.content
            && parts.len() > OPENAI_RESPONSES_CONTENT_PARTS_MAX
        {
            return Err(InputError(
                "input content parts exceeds maximum length".to_owned(),
            ));
        }
    }
    Ok(())
}

fn newest_user(items: &[InputItem]) -> Option<usize> {
    items.iter().rposition(|item| {
        item.kind.as_deref() == Some("message")
            && item.role.as_deref() == Some("user")
            && !user_parts(item.content.as_ref()).is_empty()
    })
}

fn select_items(items: &[InputItem], newest: usize, stateful: bool) -> Vec<TurnMessage> {
    if stateful {
        return vec![turn_message(
            "user",
            user_parts(items[newest].content.as_ref()),
        )];
    }
    items
        .iter()
        .filter(|item| item.kind.as_deref() == Some("message"))
        .filter_map(|item| match item.role.as_deref() {
            Some("user") => turn_if_present("user", user_parts(item.content.as_ref())),
            Some("assistant") => turn_if_present("assistant", text_parts(item.content.as_ref())),
            _ => None,
        })
        .collect()
}

fn user_parts(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) if !text.is_empty() => vec![json!({"type":"text","text":text})],
        Some(Value::Array(parts)) if parts.len() <= OPENAI_RESPONSES_CONTENT_PARTS_MAX => {
            parts.iter().filter_map(user_part).collect()
        }
        _ => Vec::new(),
    }
}

fn user_part(part: &Value) -> Option<Value> {
    let kind = part.get("type")?.as_str()?;
    if matches!(kind, "text" | "input_text") {
        let text = part.get("text")?.as_str()?;
        return (!text.is_empty()).then(|| json!({"type":"text","text":text}));
    }
    if !matches!(kind, "image_url" | "input_image") {
        return None;
    }
    let raw = part
        .get("image_url")
        .and_then(|image| image.as_str().or_else(|| image.get("url")?.as_str()))?;
    image_part(raw)
}

fn image_part(raw: &str) -> Option<Value> {
    if let Some(rest) = raw.strip_prefix("data:") {
        let (media_type, data) = rest.split_once(";base64,")?;
        return (!media_type.is_empty() && !data.is_empty()).then(|| {
            json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}})
        });
    }
    (raw.starts_with("http://") || raw.starts_with("https://"))
        .then(|| json!({"type":"image","source":{"type":"url","url":raw}}))
}

fn text_parts(content: Option<&Value>) -> Vec<Value> {
    let text = match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text" | "input_text" | "output_text") => part.get("text")?.as_str(),
                _ => None,
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    (!text.is_empty())
        .then(|| json!({"type":"text","text":text}))
        .into_iter()
        .collect()
}

fn turn_if_present(role: &'static str, content: Vec<Value>) -> Option<TurnMessage> {
    (!content.is_empty()).then(|| turn_message(role, content))
}

fn turn_message(role: &'static str, content: Vec<Value>) -> TurnMessage {
    TurnMessage {
        role,
        content,
        client_message_id: fresh_uuid().to_string(),
    }
}

fn apply_instructions(messages: &mut [TurnMessage], instructions: Option<&str>) {
    let Some(text) = instructions.filter(|text| !text.is_empty()) else {
        return;
    };
    if let Some(message) = messages.iter_mut().rev().find(|item| item.role == "user") {
        let reminder = format!("<system-reminder>\n{text}\n</system-reminder>\n\n");
        if let Some(first) = message.content.first_mut()
            && first.get("type").and_then(Value::as_str) == Some("text")
            && let Some(existing) = first.get("text").and_then(Value::as_str)
        {
            *first = json!({"type":"text","text":format!("{reminder}{existing}")});
        } else {
            message
                .content
                .insert(0, json!({"type":"text","text":reminder}));
        }
    }
}

fn validate_content_bytes(messages: &[TurnMessage]) -> Result<(), InputError> {
    let bytes = messages
        .iter()
        .flat_map(|message| &message.content)
        .map(|part| part.to_string().len())
        .sum::<usize>();
    if bytes > OPENAI_RESPONSES_CONTENT_BYTES_MAX {
        return Err(InputError(
            "input content exceeds maximum length".to_owned(),
        ));
    }
    Ok(())
}

fn missing_user() -> InputError {
    InputError("input must include a user message with text or image content".to_owned())
}

fn validate_json_shape(value: &Value) -> Result<(), InputError> {
    let mut pending = vec![(value, 1_usize)];
    let mut work = 0_usize;
    while let Some((current, depth)) = pending.pop() {
        work = work
            .checked_add(1)
            .ok_or_else(|| InputError("request JSON exceeds maximum work".to_owned()))?;
        if work > HTTP_BODY_BYTES_MAX {
            return Err(InputError("request JSON exceeds maximum work".to_owned()));
        }
        if depth > OPENAI_RESPONSES_JSON_DEPTH_MAX {
            return Err(InputError("request JSON exceeds maximum depth".to_owned()));
        }
        match current {
            Value::Array(values) => pending.extend(values.iter().map(|item| (item, depth + 1))),
            Value::Object(values) => {
                pending.extend(values.values().map(|item| (item, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}
