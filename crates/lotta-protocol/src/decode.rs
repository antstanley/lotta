//! Transport-neutral decoding and malformed-input recovery policy.

use crate::WsProtocolCommand;
use lotta_domain::RuntimeScope;
use serde::Serialize;
use serde_json::{Map, Value};

/// Observable effects of decoding one already-received JSON value.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodeEffects {
    /// Decoding disposition.
    pub outcome: DecodeOutcome,
    /// Number of state mutations caused by decoding (always zero in this crate).
    pub state_mutations: usize,
    /// Normalized outbound notices, excluding adapter-owned lifecycle fields.
    pub outbound: Vec<RecoverableLoopErrorNotice>,
}

/// Classification produced by the bounded decode policy.
#[derive(Clone, Debug, PartialEq)]
pub enum DecodeOutcome {
    /// Invalid framing or silently invalid command shape.
    SilentDrop,
    /// An unknown string discriminator, retained only as this classification.
    DroppedUnknown,
    /// A known protocol tag. Payload validation is deferred except for `input`.
    Accepted(WsProtocolCommand),
    /// A malformed `input` that admits a later input.
    RecoverableInput,
}

/// Adapter-independent loop-error notice for malformed input.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecoverableLoopErrorNotice {
    /// Outbound protocol discriminator.
    #[serde(rename = "type")]
    pub discriminator: &'static str,
    /// Stream delta subtype.
    pub message_type: &'static str,
    /// Exact baseline validation reason.
    pub reason: String,
    /// Error stop reason without terminal semantics.
    pub stop_reason: &'static str,
    /// This notice does not finish the turn.
    pub is_terminal: bool,
    /// Runtime scope copied from the valid input envelope.
    pub runtime: RuntimeScope,
    /// Explicit proof that decoding mutated no runtime state.
    pub state_mutations: usize,
    /// Explicit proof that subsequent input remains admissible.
    pub admits_further_input: bool,
}

/// Decodes already-received JSON text without applying transport bounds or performing I/O.
#[must_use]
pub fn decode_text(text: &str) -> DecodeEffects {
    match serde_json::from_str(text) {
        Ok(value) => decode_value(&value),
        Err(_) => silent_drop(),
    }
}

/// Decodes an already-parsed JSON value without applying transport bounds or performing I/O.
#[must_use]
pub fn decode_value(value: &Value) -> DecodeEffects {
    let Some(object) = value.as_object() else {
        return silent_drop();
    };
    let Some(tag) = object.get("type").and_then(Value::as_str) else {
        return silent_drop();
    };
    let Some(command) = WsProtocolCommand::from_discriminant(tag) else {
        return DecodeEffects {
            outcome: DecodeOutcome::DroppedUnknown,
            state_mutations: 0,
            outbound: Vec::new(),
        };
    };
    if command != WsProtocolCommand::Input {
        return DecodeEffects {
            outcome: DecodeOutcome::Accepted(command),
            state_mutations: 0,
            outbound: Vec::new(),
        };
    }
    decode_input(object, command)
}

fn decode_input(object: &Map<String, Value>, command: WsProtocolCommand) -> DecodeEffects {
    let Some(runtime) = object.get("runtime").and_then(valid_runtime) else {
        return silent_drop();
    };
    if !valid_request_id(object.get("request_id")) {
        return silent_drop();
    }
    match invalid_input_reason(object) {
        None => DecodeEffects {
            outcome: DecodeOutcome::Accepted(command),
            state_mutations: 0,
            outbound: Vec::new(),
        },
        Some(reason) => recoverable(runtime, reason),
    }
}

fn invalid_input_reason(object: &Map<String, Value>) -> Option<String> {
    let Some(payload) = object.get("payload").and_then(Value::as_object) else {
        return Some("Protocol violation: input.payload must be an object".into());
    };
    match payload.get("kind").and_then(Value::as_str) {
        Some("create_message") => validate_create_message(payload),
        Some("approval_response") => validate_approval(payload),
        Some("teleport_continue") => validate_teleport(payload),
        Some(kind) => Some(format!("Unsupported input payload kind: {kind}")),
        None => Some(format!(
            "Unsupported input payload kind: {}",
            js_string(payload.get("kind"))
        )),
    }
}

fn validate_create_message(payload: &Map<String, Value>) -> Option<String> {
    if !payload.get("messages").is_some_and(Value::is_array) {
        return Some(
            "Protocol violation: input.kind=create_message requires payload.messages[]".into(),
        );
    }
    if !optional_enum(payload, "image_failure_mode", &["strict", "drop"]) {
        return Some(
            "Protocol violation: input.payload.image_failure_mode must be strict or drop".into(),
        );
    }
    if !optional_string_array(payload, "client_tool_allowlist") {
        return Some(
            "Protocol violation: input.payload.client_tool_allowlist must be string[]".into(),
        );
    }
    if !optional_toolset(payload.get("client_toolset")) {
        return Some(
            concat!(
                "Protocol violation: input.payload.client_toolset must contain ",
                "an optional valid base and string[] include"
            )
            .into(),
        );
    }
    if payload
        .get("exclude_interactive_tools")
        .is_some_and(|value| !value.is_boolean())
    {
        return Some(
            "Protocol violation: input.payload.exclude_interactive_tools must be boolean".into(),
        );
    }
    if !optional_string_array(payload, "external_tool_scope_ids") {
        return Some(
            "Protocol violation: input.payload.external_tool_scope_ids must be string[]".into(),
        );
    }
    None
}

fn validate_approval(payload: &Map<String, Value>) -> Option<String> {
    let valid = payload.get("request_id").is_some_and(Value::is_string)
        && payload
            .get("error")
            .map_or_else(|| valid_decision(payload.get("decision")), Value::is_string);
    (!valid).then(|| {
        concat!(
            "Protocol violation: input.kind=approval_response requires ",
            "payload.request_id and either payload.decision or payload.error"
        )
        .into()
    })
}

fn valid_decision(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(Value::as_object) else {
        return false;
    };
    match value.get("behavior").and_then(Value::as_str) {
        Some("allow") => {
            optional_string(value, "message")
                && value
                    .get("updated_input")
                    .is_none_or(|v| v.is_null() || v.is_object())
                && optional_string_array(value, "selected_permission_suggestion_ids")
        }
        Some("deny") => value.get("message").is_some_and(Value::is_string),
        _ => false,
    }
}

fn validate_teleport(payload: &Map<String, Value>) -> Option<String> {
    let source = payload.get("source").and_then(Value::as_object);
    let continuation = payload.get("continuation");
    let valid = nonempty_string(payload.get("teleport_id"))
        && source.is_some_and(|s| {
            s.get("device_id").is_some_and(Value::is_string)
                && s.get("connection_name").is_some_and(Value::is_string)
        })
        && continuation.is_none_or(|v| {
            v.as_object()
                .is_some_and(|o| o.get("approvals").is_some_and(Value::is_array))
        });
    (!valid).then(|| {
        concat!(
            "Protocol violation: input.kind=teleport_continue requires ",
            "teleport_id, source, and optional continuation.approvals[]"
        )
        .into()
    })
}

fn valid_runtime(value: &Value) -> Option<RuntimeScope> {
    let object = value.as_object()?;
    if !nonempty_string(object.get("agent_id")) || !nonempty_string(object.get("conversation_id")) {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

fn valid_request_id(value: Option<&Value>) -> bool {
    value.is_none_or(|candidate| nonempty_string(Some(candidate)))
}
fn nonempty_string(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|v| !v.is_empty())
}
fn optional_string(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).is_none_or(Value::is_string)
}
fn optional_string_array(object: &Map<String, Value>, key: &str) -> bool {
    object
        .get(key)
        .is_none_or(|v| v.as_array().is_some_and(|a| a.iter().all(Value::is_string)))
}
fn optional_enum(object: &Map<String, Value>, key: &str, choices: &[&str]) -> bool {
    object
        .get(key)
        .is_none_or(|v| v.as_str().is_some_and(|s| choices.contains(&s)))
}
fn optional_toolset(value: Option<&Value>) -> bool {
    const BASES: &[&str] = &[
        "auto",
        "codex",
        "codex_snake",
        "default",
        "gemini",
        "gemini_snake",
        "none",
    ];
    value.is_none_or(|v| {
        v.as_object()
            .is_some_and(|o| optional_enum(o, "base", BASES) && optional_string_array(o, "include"))
    })
}
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(v) if v.is_string() => v.as_str().unwrap_or_default().into(),
        Some(v) => v.to_string(),
    }
}
fn silent_drop() -> DecodeEffects {
    DecodeEffects {
        outcome: DecodeOutcome::SilentDrop,
        state_mutations: 0,
        outbound: Vec::new(),
    }
}
fn recoverable(runtime: RuntimeScope, reason: String) -> DecodeEffects {
    DecodeEffects {
        outcome: DecodeOutcome::RecoverableInput,
        state_mutations: 0,
        outbound: vec![RecoverableLoopErrorNotice {
            discriminator: "stream_delta",
            message_type: "loop_error",
            reason,
            stop_reason: "error",
            is_terminal: false,
            runtime,
            state_mutations: 0,
            admits_further_input: true,
        }],
    }
}
