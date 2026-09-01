use crate::schema::{
    CHILD_TIMEOUT, DynamicKind, FIXTURE_BYTES_MAX, RelationshipMap, SSE_EVENTS_MAX,
};
use futures_util::StreamExt as _;
use reqwest::header::{CACHE_CONTROL, CONTENT_TYPE};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

const MAX_UNIX_TIMESTAMP_SECONDS: u64 = 253_402_300_799;

#[derive(Default)]
pub struct Normalizer {
    maps: BTreeMap<DynamicKind, BTreeMap<String, String>>,
    timestamp_tokens: Vec<String>,
}

struct TimestampAttempt {
    raw: Option<u64>,
    token: String,
    registered: bool,
}

impl Normalizer {
    fn token(&mut self, kind: DynamicKind, original: &str) -> String {
        debug_assert_ne!(kind, DynamicKind::Timestamp);
        let values = self.maps.entry(kind).or_default();
        if let Some(token) = values.get(original) {
            return token.clone();
        }
        let token = format!("<{}_ID_{}>", kind.token_name(), values.len() + 1);
        values.insert(original.to_owned(), token.clone());
        token
    }

    pub fn relationships(&self) -> RelationshipMap {
        let mut maps = self
            .maps
            .iter()
            .map(|(kind, values)| {
                let mut tokens = values.values().cloned().collect::<Vec<_>>();
                tokens.sort_by_key(|token| token_number(token));
                (*kind, tokens)
            })
            .collect::<BTreeMap<_, _>>();
        if !self.timestamp_tokens.is_empty() {
            maps.insert(DynamicKind::Timestamp, self.timestamp_tokens.clone());
        }
        let relationships = RelationshipMap(maps);
        relationships.validate().expect("valid identity registry");
        relationships
    }

    pub fn value(&mut self, value: &mut Value) -> Result<(), String> {
        self.at(value, None)
    }

    fn response_value(
        &mut self,
        value: &mut Value,
        attempt: &mut TimestampAttempt,
    ) -> Result<(), String> {
        self.at(value, Some(attempt))
    }

    fn response_attempt(&self) -> TimestampAttempt {
        TimestampAttempt {
            raw: None,
            token: format!("<TIMESTAMP_ID_{}>", self.timestamp_tokens.len() + 1),
            registered: false,
        }
    }

    fn at(
        &mut self,
        value: &mut Value,
        mut attempt: Option<&mut TimestampAttempt>,
    ) -> Result<(), String> {
        match value {
            Value::String(text) => *text = self.string(text)?,
            Value::Array(values) => {
                for child in values {
                    self.at(child, attempt.as_deref_mut())?;
                }
            }
            Value::Object(values) => {
                let timestamp_key = response_timestamp_key(values);
                for (child_key, child) in values {
                    if timestamp_key == Some(child_key.as_str()) {
                        self.timestamp(child, attempt.as_deref_mut())?;
                    } else {
                        self.at(child, attempt.as_deref_mut())?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn timestamp(
        &mut self,
        value: &mut Value,
        attempt: Option<&mut TimestampAttempt>,
    ) -> Result<(), String> {
        let attempt =
            attempt.ok_or_else(|| "dynamic timestamp outside response attempt".to_owned())?;
        let raw = value
            .as_u64()
            .filter(|raw| *raw <= MAX_UNIX_TIMESTAMP_SECONDS)
            .ok_or_else(|| format!("invalid Unix created timestamp: {value}"))?;
        if attempt.raw.is_some_and(|previous| previous != raw) {
            return Err("created timestamp changed within response attempt".to_owned());
        }
        attempt.raw = Some(raw);
        if !attempt.registered {
            self.timestamp_tokens.push(attempt.token.clone());
            attempt.registered = true;
        }
        *value = json!(attempt.token);
        Ok(())
    }

    fn string(&mut self, text: &str) -> Result<String, String> {
        if text.starts_with("resp_letta_") {
            return Ok(self.token(DynamicKind::StoredResponse, text));
        }
        for (kind, prefix) in dynamic_prefixes() {
            if let Some(suffix) = text.strip_prefix(prefix) {
                validate_uuid(suffix)
                    .map_err(|_| format!("malformed dynamic identifier: {text}"))?;
                return Ok(self.token(kind, text));
            }
        }
        if uuid::Uuid::parse_str(text).is_ok() {
            validate_uuid(text).map_err(|_| format!("malformed UUID: {text}"))?;
            return Ok(self.token(DynamicKind::Uuid, text));
        }
        if valid_conversation(text) {
            return Ok(self.token(DynamicKind::Conversation, text));
        }
        if reserved_prefix(text) {
            return Err(format!("malformed dynamic identifier: {text}"));
        }
        Ok(text.to_owned())
    }
}

fn response_timestamp_key(values: &Map<String, Value>) -> Option<&'static str> {
    match values.get("object").and_then(Value::as_str) {
        Some(kind) if kind.starts_with("chat.completion") => Some("created"),
        Some("response") => Some("created_at"),
        _ => None,
    }
}

fn dynamic_prefixes() -> [(DynamicKind, &'static str); 6] {
    [
        (DynamicKind::ChatCompletion, "chatcmpl-"),
        (DynamicKind::Response, "resp_"),
        (DynamicKind::Msg, "msg_"),
        (DynamicKind::Fco, "fco_"),
        (DynamicKind::Fc, "fc_"),
        (DynamicKind::Rs, "rs_"),
    ]
}

fn token_number(token: &str) -> usize {
    token
        .strip_suffix('>')
        .and_then(|value| value.rsplit('_').next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(usize::MAX)
}

fn validate_uuid(text: &str) -> Result<(), ()> {
    let value = uuid::Uuid::parse_str(text).map_err(|_| ())?;
    if value.hyphenated().to_string() != text || value.get_version_num() != 4 {
        return Err(());
    }
    Ok(())
}

fn valid_conversation(text: &str) -> bool {
    ["conversation-", "conv-fake-headless-", "local-conv-"]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .is_some_and(|suffix| {
            !suffix.is_empty()
                && !suffix.starts_with('0')
                && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn reserved_prefix(text: &str) -> bool {
    dynamic_prefixes()
        .iter()
        .map(|(_, prefix)| *prefix)
        .chain(["conversation-", "conv-fake-headless-", "local-conv-"])
        .any(|prefix| text.starts_with(prefix))
}

pub struct NormalizedResponse {
    pub value: Value,
    pub stored_id: Option<String>,
}

pub async fn response(
    response: reqwest::Response,
    sse: bool,
    normalizer: &mut Normalizer,
) -> Result<NormalizedResponse, String> {
    let status = response.status().as_u16();
    let headers = selected_headers(response.headers());
    let mut attempt = normalizer.response_attempt();
    if sse {
        let events = parse_sse(response, normalizer, &mut attempt).await?;
        return Ok(NormalizedResponse {
            value: json!({
                "status":status,
                "headers":headers,
                "events":events,
                "dynamic_map":normalizer.relationships(),
            }),
            stored_id: None,
        });
    }
    json_response(response, status, headers, normalizer, &mut attempt).await
}

async fn json_response(
    response: reqwest::Response,
    status: u16,
    headers: Value,
    normalizer: &mut Normalizer,
    attempt: &mut TimestampAttempt,
) -> Result<NormalizedResponse, String> {
    let mut body: Value = tokio::time::timeout(CHILD_TIMEOUT, response.json())
        .await
        .map_err(|_| "JSON response timeout".to_owned())?
        .map_err(|error| format!("JSON response: {error}"))?;
    let stored_id = body
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.starts_with("resp_letta_"))
        .map(str::to_owned);
    normalizer.response_value(&mut body, attempt)?;
    Ok(NormalizedResponse {
        value: json!({
            "status":status,
            "headers":headers,
            "body":body,
            "dynamic_map":normalizer.relationships(),
        }),
        stored_id,
    })
}

fn selected_headers(headers: &reqwest::header::HeaderMap) -> Value {
    let mut values = Map::new();
    for name in [CONTENT_TYPE, CACHE_CONTROL] {
        if let Some(value) = headers.get(&name) {
            values.insert(
                name.as_str().to_owned(),
                Value::String(value.to_str().expect("header text").to_owned()),
            );
        }
    }
    Value::Object(values)
}

async fn parse_sse(
    response: reqwest::Response,
    normalizer: &mut Normalizer,
    attempt: &mut TimestampAttempt,
) -> Result<Vec<Value>, String> {
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut events = Vec::new();
    while let Some(chunk) = tokio::time::timeout(CHILD_TIMEOUT, stream.next())
        .await
        .map_err(|_| "SSE chunk timeout".to_owned())?
    {
        let chunk = chunk.map_err(|error| error.to_string())?;
        pending.extend_from_slice(&chunk);
        if pending.len() > FIXTURE_BYTES_MAX {
            return Err("SSE pending byte bound".to_owned());
        }
        drain_events(&mut pending, &mut events, normalizer, attempt)?;
    }
    if !pending.is_empty() {
        return Err("SSE framing has trailing bytes".to_owned());
    }
    Ok(events)
}

fn drain_events(
    pending: &mut Vec<u8>,
    events: &mut Vec<Value>,
    normalizer: &mut Normalizer,
    attempt: &mut TimestampAttempt,
) -> Result<(), String> {
    while let Some(end) = pending.windows(2).position(|part| part == b"\n\n") {
        let block = pending.drain(..end + 2).collect::<Vec<_>>();
        events.push(parse_block(&block[..end], normalizer, attempt)?);
        if events.len() > SSE_EVENTS_MAX {
            return Err("SSE event bound".to_owned());
        }
    }
    Ok(())
}

fn parse_block(
    block: &[u8],
    normalizer: &mut Normalizer,
    attempt: &mut TimestampAttempt,
) -> Result<Value, String> {
    let text = std::str::from_utf8(block).map_err(|error| error.to_string())?;
    let mut event = Value::Null;
    let mut data = None;
    for line in text.split('\n') {
        if let Some(value) = line.strip_prefix("event: ") {
            event = json!(value);
        } else if let Some(value) = line.strip_prefix("data: ") {
            if data.replace(value).is_some() {
                return Err("duplicate SSE data line".to_owned());
            }
        } else {
            return Err(format!("invalid SSE framing line: {line:?}"));
        }
    }
    let data = data.ok_or_else(|| "SSE data line".to_owned())?;
    let mut value = if data == "[DONE]" {
        json!(data)
    } else {
        serde_json::from_str(data).map_err(|error| error.to_string())?
    };
    normalizer.response_value(&mut value, attempt)?;
    Ok(json!({"event":event,"data":value}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_registry_preserves_reuse_and_distinction() {
        let one = "123e4567-e89b-42d3-a456-426614174000";
        let two = "123e4567-e89b-42d3-b456-426614174001";
        let mut value = json!([
            format!("msg_{one}"),
            format!("msg_{one}"),
            format!("msg_{two}")
        ]);
        let mut normalizer = Normalizer::default();
        normalizer.value(&mut value).expect("normalize IDs");
        assert_eq!(value, json!(["<MSG_ID_1>", "<MSG_ID_1>", "<MSG_ID_2>"]));
        normalizer
            .relationships()
            .validate()
            .expect("relationship schema");
    }

    #[test]
    fn relationship_kind_number_collapse_and_split_mutations_fail() {
        let malformed: Result<RelationshipMap, _> = serde_json::from_value(json!({"other":[]}));
        assert!(malformed.is_err());
        for tokens in [
            vec!["<MSG_ID_2>"],
            vec!["<MSG_ID_1>", "<MSG_ID_1>"],
            vec!["<UUID_ID_1>"],
        ] {
            let map = RelationshipMap(BTreeMap::from([(
                DynamicKind::Msg,
                tokens.into_iter().map(str::to_owned).collect(),
            )]));
            assert!(map.validate().is_err());
        }
        timestamps_are_attempt_scoped_not_raw_identities();
        timestamp_mutations_validate_attempt_consistency_and_range();
        static_semantic_timestamps_and_sequences_are_unchanged();
    }

    fn timestamps_are_attempt_scoped_not_raw_identities() {
        fn shape(raw: [u64; 2]) -> Value {
            let mut normalizer = Normalizer::default();
            let mut output = Vec::new();
            for created in raw {
                let mut value = json!({"object":"chat.completion","created":created});
                let mut attempt = normalizer.response_attempt();
                normalizer
                    .response_value(&mut value, &mut attempt)
                    .expect("normalize timestamp");
                output.push(value);
            }
            json!({"responses":output,"dynamic_map":normalizer.relationships()})
        }
        assert_eq!(shape([1, 1]), shape([1, 2]));
    }

    fn timestamp_mutations_validate_attempt_consistency_and_range() {
        let mut normalizer = Normalizer::default();
        let mut attempt = normalizer.response_attempt();
        let mut first = json!({"object":"chat.completion.chunk","created":1});
        normalizer
            .response_value(&mut first, &mut attempt)
            .expect("first chunk");
        let mut changed = json!({"object":"chat.completion.chunk","created":2});
        assert!(
            normalizer
                .response_value(&mut changed, &mut attempt)
                .is_err()
        );
        for malformed in [
            json!("1"),
            json!(1.5),
            json!(-1),
            json!(MAX_UNIX_TIMESTAMP_SECONDS + 1),
        ] {
            let mut normalizer = Normalizer::default();
            let mut attempt = normalizer.response_attempt();
            let mut value = json!({"object":"response","created_at":malformed});
            assert!(normalizer.response_value(&mut value, &mut attempt).is_err());
        }
    }

    fn static_semantic_timestamps_and_sequences_are_unchanged() {
        let mut value = json!({"object":"model","created":1,"timestamp":2,"sequence":3});
        Normalizer::default()
            .value(&mut value)
            .expect("normalize IDs only");
        assert_eq!(
            value,
            json!({"object":"model","created":1,"timestamp":2,"sequence":3})
        );
    }

    #[test]
    fn malformed_dynamic_identifier_is_rejected() {
        let mut malformed = json!("msg_not-a-uuid");
        assert!(Normalizer::default().value(&mut malformed).is_err());
    }
}
