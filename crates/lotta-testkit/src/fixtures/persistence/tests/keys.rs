use super::*;

const SOURCES: [&str; 3] = [
    "agent-local-fixture",
    "default:agent-local-fixture",
    "conversation:conversation-fixture",
];
const ENCODED: [&str; 3] = [AGENT_KEY, DEFAULT_KEY, NAMED_KEY];

fn decode_base64url(value: &str) -> Option<Vec<u8>> {
    if value.is_empty() || value.contains(['=', '+', '/']) {
        return None;
    }
    let mut output = Vec::with_capacity(value.len() * 3 / 4 + 2);
    let (mut accumulator, mut bits) = (0_u32, 0_u8);
    for byte in value.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | u32::from(digit);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(u8::try_from(accumulator >> bits).ok()?);
            accumulator &= (1_u32 << bits) - 1;
        }
    }
    Some(output)
}

fn encode_base64url(value: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::new();
    for chunk in value.chunks(3) {
        let a = u32::from(chunk[0]);
        let b = u32::from(*chunk.get(1).unwrap_or(&0));
        let c = u32::from(*chunk.get(2).unwrap_or(&0));
        let bits = (a << 16) | (b << 8) | c;
        output.push(char::from(ALPHABET[((bits >> 18) & 63) as usize]));
        output.push(char::from(ALPHABET[((bits >> 12) & 63) as usize]));
        if chunk.len() > 1 {
            output.push(char::from(ALPHABET[((bits >> 6) & 63) as usize]));
        }
        if chunk.len() > 2 {
            output.push(char::from(ALPHABET[(bits & 63) as usize]));
        }
    }
    output
}

#[test]
fn both_key_forms() {
    let value = index();
    let records = [
        &value.keys.agent,
        &value.keys.default_conversation,
        &value.keys.named_conversation,
    ];
    for position in 0..3 {
        assert_eq!(records[position].source, SOURCES[position]);
        assert_eq!(records[position].encoded, ENCODED[position]);
        assert_eq!(
            encode_base64url(SOURCES[position].as_bytes()),
            ENCODED[position]
        );
        assert_eq!(
            decode_base64url(ENCODED[position]).as_deref(),
            Some(SOURCES[position].as_bytes())
        );
        assert!(!ENCODED[position].contains(['=', '+', '/']));
    }
    assert_literal_current_tree();
}

fn assert_literal_current_tree() {
    let agent = load_json(&format!("current_typescript_state/agents/{AGENT_KEY}.json"));
    assert_eq!(agent["id"], AGENT_ID);
    for key in [DEFAULT_KEY, NAMED_KEY] {
        let root = conversation_root("current_typescript_state", key);
        let record = load_json(&format!("{root}/conversation.json"));
        assert_eq!(record["id"], CONVERSATION_ID);
        assert_eq!(record["agent_id"], AGENT_ID);
        assert_manifest(
            &format!("{root}/manifest.json"),
            2,
            "pi-session-entry-jsonl",
        );
        let transcript = rows(&format!("{root}/messages.jsonl"));
        assert_eq!(transcript[0]["type"], "session");
        assert_eq!(transcript[0]["version"], 3);
        assert_eq!(transcript[0]["id"], CONVERSATION_ID);
        assert_eq!(transcript[1]["type"], "message");
        assert_eq!(transcript[1]["parentId"], Value::Null);
        assert_eq!(transcript[1]["message"]["id"], "msg-user-fixture");
        assert_eq!(
            load_json(&format!("{root}/system-prompt.json"))["content"],
            "SANITIZED_FIXTURE_SYSTEM_PROMPT"
        );
    }
    let default = rows(&format!(
        "current_typescript_state/conversations/{DEFAULT_KEY}/messages.jsonl"
    ));
    let named = rows(&format!(
        "current_typescript_state/conversations/{NAMED_KEY}/messages.jsonl"
    ));
    assert_ne!(default[1]["id"], named[1]["id"]);
    assert_eq!(default[0]["id"], named[0]["id"]);
}
