//! Bounded WebSocket JSON framing before protocol dispatch.

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::fmt;

use crate::{
    bounds::{REQUEST_ID_BYTES_MAX, WS_MESSAGE_FIELDS_MAX},
    errors::ProtocolErrorEnvelope,
};

/// Successfully bounded protocol input and downstream decode effects.
#[derive(Debug)]
pub struct DecodedFrame {
    /// Valid top-level request correlation.
    pub request_id: Option<String>,
    /// Parsed JSON retained for Task 16 command routing.
    pub value: Value,
    /// Existing protocol decoder classification.
    pub effects: lotta_protocol::DecodeEffects,
}

/// Validates field count and request correlation before protocol dispatch.
///
/// # Errors
/// Returns a stable protocol error for malformed, over-field, or invalid request-ID input.
pub fn decode_text(text: &str) -> Result<DecodedFrame, ProtocolErrorEnvelope> {
    preflight_fields(text)?;
    let value: Value = serde_json::from_str(text)
        .map_err(|_| protocol_error("malformed_json", "malformed JSON", None))?;
    let request_id = request_id(&value)?;
    validate_nested_request_id(&value, request_id.clone())?;
    let effects = lotta_protocol::decode_value(&value);
    Ok(DecodedFrame {
        request_id,
        value,
        effects,
    })
}

fn preflight_fields(text: &str) -> Result<(), ProtocolErrorEnvelope> {
    let mut fields = 0usize;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    FieldSeed {
        fields: &mut fields,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| {
        if error.to_string().contains("field limit") {
            protocol_error(
                "message_fields_exceeded",
                "message has too many fields",
                None,
            )
        } else {
            protocol_error("malformed_json", "malformed JSON", None)
        }
    })?;
    deserializer
        .end()
        .map_err(|_| protocol_error("malformed_json", "malformed JSON", None))
}

struct FieldSeed<'a> {
    fields: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for FieldSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(FieldVisitor {
            fields: self.fields,
        })
    }
}

struct FieldVisitor<'a> {
    fields: &'a mut usize,
}

impl<'de> Visitor<'de> for FieldVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON")
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        while map.next_key::<IgnoredAny>()?.is_some() {
            *self.fields = self.fields.saturating_add(1);
            if *self.fields > WS_MESSAGE_FIELDS_MAX {
                return Err(de::Error::custom("field limit"));
            }
            map.next_value_seed(FieldSeed {
                fields: self.fields,
            })?;
        }
        Ok(())
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence
            .next_element_seed(FieldSeed {
                fields: self.fields,
            })?
            .is_some()
        {}
        Ok(())
    }

    fn visit_bool<E>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_borrowed_str<E>(self, _: &'de str) -> Result<(), E> {
        Ok(())
    }
    fn visit_string<E>(self, _: String) -> Result<(), E> {
        Ok(())
    }
    fn visit_none<E>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_some<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }
}

fn request_id(value: &Value) -> Result<Option<String>, ProtocolErrorEnvelope> {
    let Some(raw) = value
        .as_object()
        .and_then(|object| object.get("request_id"))
    else {
        return Ok(None);
    };
    let Some(text) = raw.as_str() else {
        return Err(protocol_error(
            "request_id_invalid",
            "request_id must be a string",
            None,
        ));
    };
    if text.len() > REQUEST_ID_BYTES_MAX {
        return Err(protocol_error(
            "request_id_too_long",
            "request_id is too long",
            None,
        ));
    }
    Ok(Some(text.to_owned()))
}

fn validate_nested_request_id(
    value: &Value,
    correlation: Option<String>,
) -> Result<(), ProtocolErrorEnvelope> {
    let Some(payload) = value
        .as_object()
        .filter(|object| object.get("type").and_then(Value::as_str) == Some("input"))
        .and_then(|object| object.get("payload"))
        .and_then(Value::as_object)
        .filter(|payload| payload.get("kind").and_then(Value::as_str) == Some("approval_response"))
    else {
        return Ok(());
    };
    let Some(raw) = payload.get("request_id") else {
        return Ok(());
    };
    let Some(text) = raw.as_str() else {
        return Err(protocol_error(
            "request_id_invalid",
            "request_id must be a string",
            correlation,
        ));
    };
    if text.len() > REQUEST_ID_BYTES_MAX {
        return Err(protocol_error(
            "request_id_too_long",
            "request_id is too long",
            correlation,
        ));
    }
    Ok(())
}

fn protocol_error(
    code: &'static str,
    message: &'static str,
    request_id: Option<String>,
) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(code, message, request_id)
}

#[cfg(test)]
pub(crate) fn decode_text_with_probe(
    text: &str,
    probe: &std::sync::atomic::AtomicUsize,
) -> Result<DecodedFrame, ProtocolErrorEnvelope> {
    preflight_fields(text)?;
    probe.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let value: Value = serde_json::from_str(text)
        .map_err(|_| protocol_error("malformed_json", "malformed JSON", None))?;
    let request_id = request_id(&value)?;
    validate_nested_request_id(&value, request_id.clone())?;
    let effects = lotta_protocol::decode_value(&value);
    Ok(DecodedFrame {
        request_id,
        value,
        effects,
    })
}

#[cfg(test)]
pub(crate) fn test_object(fields: usize) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(fields * 7 + 2);
    text.push('{');
    for index in 0..fields {
        if index > 0 {
            text.push(',');
        }
        write!(text, "\"k{index}\":0").expect("writing to String cannot fail");
    }
    text.push('}');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn fields_below_accepted() {
        assert!(decode_text(&test_object(4095)).is_ok());
    }
    #[test]
    fn fields_at_accepted_and_parsed_once() {
        let probe = AtomicUsize::new(0);
        assert!(decode_text_with_probe(&test_object(4096), &probe).is_ok());
        assert_eq!(probe.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn fields_above_rejected_before_parse() {
        let probe = AtomicUsize::new(0);
        assert!(decode_text_with_probe(&test_object(4097), &probe).is_err());
        assert_eq!(probe.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn nested_field_totals_enforced() {
        let text = format!("{{\"outer\":{}}}", test_object(4096));
        assert!(decode_text(&text).is_err());
    }
    #[test]
    fn request_id_below_bound_correlates() {
        let id = "r".repeat(REQUEST_ID_BYTES_MAX - 1);
        let frame = decode_text(&format!(r#"{{"type":"sync","request_id":"{id}"}}"#))
            .expect("bounded frame");
        assert_eq!(frame.request_id.as_deref(), Some(id.as_str()));
    }
    #[test]
    fn request_id_at_bound_correlates() {
        let id = "r".repeat(256);
        let frame = decode_text(&format!(r#"{{"type":"sync","request_id":"{id}"}}"#))
            .expect("bounded frame");
        assert_eq!(frame.request_id.as_deref(), Some(id.as_str()));
    }
    #[test]
    fn request_id_above_bound_is_not_correlated() {
        let id = "r".repeat(257);
        let error = decode_text(&format!(r#"{{"type":"sync","request_id":"{id}"}}"#))
            .expect_err("oversized ID");
        assert!(error.request_id.is_none());
    }
    #[test]
    fn nested_approval_request_id_below_bound_is_accepted() {
        assert!(approval_with_ids(REQUEST_ID_BYTES_MAX, REQUEST_ID_BYTES_MAX - 1).is_ok());
    }
    #[test]
    fn nested_approval_request_id_at_bound_is_accepted() {
        assert!(approval_with_ids(REQUEST_ID_BYTES_MAX, REQUEST_ID_BYTES_MAX).is_ok());
    }
    #[test]
    fn nested_approval_request_id_above_correlates_outer() {
        let error = approval_with_ids(REQUEST_ID_BYTES_MAX, REQUEST_ID_BYTES_MAX + 1)
            .expect_err("oversized nested ID");
        assert_eq!(error.code, "request_id_too_long");
        assert_eq!(error.request_id.as_deref(), Some("r".repeat(256).as_str()));
    }
    #[test]
    fn arbitrary_tool_content_request_id_is_not_rejected() {
        let content = "x".repeat(REQUEST_ID_BYTES_MAX + 1);
        let text = serde_json::json!({
            "type": "input",
            "request_id": "r",
            "payload": {
                "kind": "create_message",
                "request_id": content,
                "messages": [],
            },
        })
        .to_string();
        assert!(decode_text(&text).is_ok());
    }
    #[test]
    fn request_id_nonstring_is_stable() {
        let error = decode_text(r#"{"type":"sync","request_id":7}"#).expect_err("nonstring");
        assert_eq!(error.code, "request_id_invalid");
    }
    fn approval_with_ids(
        outer_len: usize,
        nested_len: usize,
    ) -> Result<DecodedFrame, ProtocolErrorEnvelope> {
        let outer = "r".repeat(outer_len);
        let nested = "n".repeat(nested_len);
        let text = serde_json::json!({
            "type": "input",
            "request_id": outer,
            "runtime": { "agent_id": "a", "conversation_id": "c" },
            "payload": {
                "kind": "approval_response",
                "request_id": nested,
                "decision": { "behavior": "allow" },
            },
        })
        .to_string();
        decode_text(&text)
    }

    #[test]
    fn decoder_integration_accepts_known_type() {
        let frame = decode_text(r#"{"type":"sync","request_id":"r"}"#).expect("valid");
        assert!(matches!(
            frame.effects.outcome,
            lotta_protocol::DecodeOutcome::Accepted(_)
        ));
    }
}
