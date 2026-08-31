//! Unsigned, bounded stored-Response state cursors.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use lotta_domain::{AgentId, ConversationId};
use serde::{Deserialize, Serialize};

use super::chat::fresh_uuid;

/// Prefix distinguishing durable Responses cursors from ordinary response IDs.
pub const STORED_RESPONSE_PREFIX: &str = "resp_letta_";
/// Current stored-Response cursor schema version.
pub const STORED_RESPONSE_VERSION: u8 = 1;
/// Largest accepted encoded cursor payload.
pub const RESPONSE_CURSOR_BYTES_MAX: usize = 4_096;
/// Largest accepted agent identifier in a cursor.
pub const RESPONSE_CURSOR_AGENT_ID_BYTES_MAX: usize = 1_024;
/// Largest accepted conversation identifier in a cursor.
pub const RESPONSE_CURSOR_CONVERSATION_ID_BYTES_MAX: usize = 1_024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CursorWire {
    version: u8,
    nonce: String,
    agent_id: String,
    conversation_id: String,
}

/// Opaque stored-Response cursor validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorError;

/// Validated state recovered from an unsigned stored-Response cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredResponseCursor {
    /// Cursor owner agent.
    pub agent_id: AgentId,
    /// Retained source conversation.
    pub conversation_id: ConversationId,
}

/// Encodes a fresh unsigned base64url-no-pad cursor.
///
/// # Errors
/// Returns an opaque failure if bounded canonical identifiers cannot be serialized.
pub fn encode(agent_id: &AgentId, conversation_id: &ConversationId) -> Result<String, CursorError> {
    validate_lengths(agent_id.as_str(), conversation_id.as_str())?;
    let wire = CursorWire {
        version: STORED_RESPONSE_VERSION,
        nonce: fresh_uuid().to_string(),
        agent_id: agent_id.as_str().to_owned(),
        conversation_id: conversation_id.as_str().to_owned(),
    };
    let bytes = serde_json::to_vec(&wire).map_err(|_| CursorError)?;
    if bytes.len() > RESPONSE_CURSOR_BYTES_MAX {
        return Err(CursorError);
    }
    Ok(format!(
        "{STORED_RESPONSE_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

/// Strictly parses one bounded cursor without accepting signatures or padded base64.
///
/// # Errors
/// Rejects prefix, base64, schema, version, nonce, ID, and bound mismatches.
pub fn parse(response_id: &str) -> Result<StoredResponseCursor, CursorError> {
    let encoded = response_id
        .strip_prefix(STORED_RESPONSE_PREFIX)
        .filter(|value| !value.is_empty() && value.len() <= RESPONSE_CURSOR_BYTES_MAX * 2)
        .ok_or(CursorError)?;
    if encoded.contains('=') {
        return Err(CursorError);
    }
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| CursorError)?;
    if URL_SAFE_NO_PAD.encode(&bytes) != encoded {
        return Err(CursorError);
    }
    if bytes.is_empty() || bytes.len() > RESPONSE_CURSOR_BYTES_MAX {
        return Err(CursorError);
    }
    let wire: CursorWire = serde_json::from_slice(&bytes).map_err(|_| CursorError)?;
    validate_wire(wire)
}

fn validate_wire(wire: CursorWire) -> Result<StoredResponseCursor, CursorError> {
    if wire.version != STORED_RESPONSE_VERSION || uuid::Uuid::parse_str(&wire.nonce).is_err() {
        return Err(CursorError);
    }
    validate_lengths(&wire.agent_id, &wire.conversation_id)?;
    let agent_id = AgentId::accept(wire.agent_id).map_err(|_| CursorError)?;
    let conversation_id = ConversationId::accept(wire.conversation_id).map_err(|_| CursorError)?;
    Ok(StoredResponseCursor {
        agent_id,
        conversation_id,
    })
}

fn validate_lengths(agent_id: &str, conversation_id: &str) -> Result<(), CursorError> {
    if agent_id.is_empty()
        || conversation_id.is_empty()
        || agent_id.len() > RESPONSE_CURSOR_AGENT_ID_BYTES_MAX
        || conversation_id.len() > RESPONSE_CURSOR_CONVERSATION_ID_BYTES_MAX
    {
        return Err(CursorError);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (AgentId, ConversationId) {
        (
            AgentId::accept("agent-local-cursor").unwrap(),
            ConversationId::accept("local-conv-cursor").unwrap(),
        )
    }

    #[test]
    fn cursor_round_trip_has_exact_unsigned_fields() {
        let (agent, conversation) = ids();
        let encoded = encode(&agent, &conversation).unwrap();
        assert!(!encoded.contains('='));
        assert_eq!(parse(&encoded).unwrap().agent_id, agent);
        let payload = encoded.strip_prefix(STORED_RESPONSE_PREFIX).unwrap();
        let decoded = URL_SAFE_NO_PAD.decode(payload).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert_eq!(value["version"], 1);
        assert!(value["nonce"].as_str().is_some());
        assert_eq!(value["conversation_id"], conversation.as_str());
    }

    #[test]
    fn malformed_and_tampered_cursors_are_rejected_deterministically() {
        let (agent, conversation) = ids();
        let valid = encode(&agent, &conversation).unwrap();
        for invalid in ["resp_missing", "resp_letta_", "resp_letta_%%%%"] {
            assert!(parse(invalid).is_err());
        }
        let payload = valid.strip_prefix(STORED_RESPONSE_PREFIX).unwrap();
        let decoded = URL_SAFE_NO_PAD.decode(payload).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        value["version"] = serde_json::json!(2);
        let tampered = format!(
            "{STORED_RESPONSE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())
        );
        assert!(parse(&tampered).is_err());
    }
}
