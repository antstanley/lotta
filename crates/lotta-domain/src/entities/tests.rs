use super::*;
use crate::bounds::EXTRAS_FIELDS_MAX;
use crate::{AgentId, ConversationId, MessageId, NonEmptyString, RunId, Timestamp};
use jsonschema::Validator;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

const CANONICAL_ENTITY_NAMES: [&str; 14] = [
    "Agent",
    "Conversation",
    "LocalMessage",
    "TranscriptManifest",
    "SessionEntry",
    "MessageEntry",
    "CompactionEntry",
    "ProviderConnection",
    "ModelDescriptor",
    "Run",
    "Schedule",
    "ChannelAccount",
    "ChannelRoute",
    "MemoryBlockInput",
];

fn schema() -> Value {
    let text = include_str!("../../../../.specs/canonical-types.schema.json");
    match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("checked-in canonical schema must parse: {error}"),
    }
}

fn validator(name: &str) -> Validator {
    let mut definition = schema()["$defs"][name].clone();
    if let Value::Object(map) = &mut definition {
        map.insert("$defs".into(), schema()["$defs"].clone());
    }
    match jsonschema::validator_for(&definition) {
        Ok(value) => value,
        Err(error) => panic!("canonical definition must compile: {error}"),
    }
}

fn timestamp() -> Timestamp {
    match serde_json::from_value(json!("2026-08-14T12:34:56Z")) {
        Ok(value) => value,
        Err(error) => panic!("test timestamp must parse: {error}"),
    }
}

fn non_empty(value: &str) -> NonEmptyString {
    match NonEmptyString::new(value) {
        Ok(value) => value,
        Err(error) => panic!("test non-empty string must parse: {error}"),
    }
}

fn agent_id() -> AgentId {
    match AgentId::accept("agent-local-test") {
        Ok(value) => value,
        Err(error) => panic!("test agent id must parse: {error}"),
    }
}

fn conversation_id() -> ConversationId {
    match ConversationId::accept("default") {
        Ok(value) => value,
        Err(error) => panic!("test conversation id must parse: {error}"),
    }
}

fn message_id() -> MessageId {
    match MessageId::accept("ui-msg-1") {
        Ok(value) => value,
        Err(error) => panic!("test message id must parse: {error}"),
    }
}

fn local_message() -> LocalMessage {
    LocalMessage {
        id: message_id(),
        role: LocalMessageRole::User,
        content: Some(
            BoundedJsonValue::new(json!([{"type":"text","text":"hello"}]))
                .unwrap_or_else(|e| panic!("content: {e}")),
        ),
        timestamp: 1.0,
        metadata: None,
    }
}

fn validate_round_trip<T>(name: &str, value: &T)
where
    T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = match serde_json::to_value(value) {
        Ok(value) => value,
        Err(error) => panic!("entity must serialize: {error}"),
    };
    assert!(validator(name).is_valid(&encoded), "{name}: {encoded}");
    let decoded: T = match serde_json::from_value(encoded) {
        Ok(value) => value,
        Err(error) => panic!("entity must deserialize: {error}"),
    };
    assert_eq!(&decoded, value);
}

fn empty_extras() -> EntityExtras {
    match EntityExtras::new(BTreeMap::new(), &[]) {
        Ok(value) => value,
        Err(error) => panic!("empty extras must be valid: {error}"),
    }
}

fn agent() -> Agent {
    Agent {
        id: agent_id(),
        name: non_empty("Agent"),
        description: None,
        system: String::new(),
        tags: BoundedVec::new(vec!["test".into()]).unwrap_or_else(|e| panic!("tags: {e}")),
        model: non_empty("provider/model"),
        model_settings: BoundedMap::new(BTreeMap::new()).unwrap_or_else(|e| panic!("map: {e}")),
        hidden: None,
        compaction_settings: None,
        extras: empty_extras(),
    }
}

fn conversation() -> Conversation {
    Conversation {
        id: conversation_id(),
        agent_id: agent_id(),
        archived: false,
        archived_at: None,
        created_at: timestamp(),
        updated_at: timestamp(),
        last_message_at: None,
        summary: None,
        in_context_message_ids: BoundedVec::new(vec![message_id()])
            .unwrap_or_else(|e| panic!("ids: {e}")),
        model: None,
        model_settings: None,
        context_window_limit: None,
        hidden: None,
        tags: None,
        extras: empty_extras(),
    }
}

fn schedule() -> Schedule {
    Schedule {
        id: non_empty("schedule-1"),
        agent_id: agent_id(),
        conversation_id: conversation_id(),
        name: "name".into(),
        description: String::new(),
        cron: non_empty("0 * * * *"),
        timezone: IanaTimezone::new("America/Los_Angeles")
            .unwrap_or_else(|error| panic!("test timezone must be valid: {error}")),
        recurring: false,
        prompt: non_empty("hello"),
        status: ScheduleStatus::Active,
        created_at: timestamp(),
        expires_at: None,
        last_fired_at: None,
        fire_count: 0,
        cancel_reason: None,
        jitter_offset_ms: -500,
        last_run_at: None,
        last_run_outcome: None,
        last_run_reason: None,
        last_run_error: None,
        missed_count: Some(0),
        failed_count: Some(0),
        scheduled_for: Some(timestamp()),
        fired_at: None,
        missed_at: None,
    }
}

fn channel_account() -> ChannelAccount {
    ChannelAccount {
        channel_id: non_empty("custom"),
        account_id: non_empty("account"),
        display_name: Some("Account".into()),
        enabled: true,
        configured: true,
        running: false,
        dm_policy: DmPolicy::Allowlist,
        group_policy: Some(GroupPolicy::Open),
        allowed_users: BoundedVec::new(vec!["user".into()]).unwrap_or_else(|e| panic!("list: {e}")),
        admin_users: Some(
            BoundedVec::new(vec!["admin".into()]).unwrap_or_else(|e| panic!("list: {e}")),
        ),
        user_allowed_commands: Some(
            BoundedVec::new(vec!["status".into()]).unwrap_or_else(|e| panic!("list: {e}")),
        ),
        config: BoundedMap::new(BTreeMap::new()).unwrap_or_else(|e| panic!("map: {e}")),
        created_at: timestamp(),
        updated_at: timestamp(),
    }
}

fn channel_route() -> ChannelRoute {
    ChannelRoute {
        channel_id: non_empty("custom"),
        account_id: non_empty("account"),
        chat_id: non_empty("chat"),
        chat_type: Some(ChannelChatType::Direct),
        thread_id: None,
        agent_id: agent_id(),
        conversation_id: conversation_id(),
        enabled: true,
        outbound_enabled: Some(true),
        created_at: timestamp(),
        updated_at: timestamp(),
    }
}

macro_rules! schema_case {
    ($test:ident, $name:literal, $value:expr) => {
        #[test]
        fn $test() {
            validate_round_trip($name, &$value);
        }
    };
}

schema_case!(schema_conformance_agent, "Agent", agent());
schema_case!(
    schema_conformance_conversation,
    "Conversation",
    conversation()
);
schema_case!(
    schema_conformance_local_message,
    "LocalMessage",
    local_message()
);
schema_case!(schema_conformance_schedule, "Schedule", schedule());
schema_case!(
    schema_conformance_channel_account,
    "ChannelAccount",
    channel_account()
);
schema_case!(
    schema_conformance_channel_route,
    "ChannelRoute",
    channel_route()
);
schema_case!(
    schema_conformance_memory_block,
    "MemoryBlockInput",
    MemoryBlockInput {
        label: non_empty("human"),
        value: "value".into(),
        description: None
    }
);
schema_case!(
    schema_conformance_provider,
    "ProviderConnection",
    ProviderConnection {
        provider_id: non_empty("openai"),
        auth_method: non_empty("api-key"),
        connected: true,
        metadata: Some(BoundedMap::new(BTreeMap::new()).unwrap_or_else(|e| panic!("map: {e}"))),
    }
);
schema_case!(
    schema_conformance_model,
    "ModelDescriptor",
    ModelDescriptor {
        handle: non_empty("openai/gpt"),
        provider_id: non_empty("openai"),
        available: true,
        context_window: Some(128_000),
        model_settings: Some(
            BoundedMap::new(BTreeMap::new()).unwrap_or_else(|e| panic!("map: {e}"))
        ),
    }
);
schema_case!(
    schema_conformance_run,
    "Run",
    Run {
        id: RunId::accept("local-run-1").unwrap_or_else(|error| panic!("test run: {error}")),
        agent_id: agent_id(),
        conversation_id: conversation_id(),
        status: RunStatus::Running,
        stop_reason: None,
        created_at: timestamp(),
        completed_at: None,
        background: Some(Some(false)),
        metadata: Some(BoundedMap::new(BTreeMap::new()).unwrap_or_else(|e| panic!("map: {e}"))),
        usage: None,
    }
);
schema_case!(
    schema_conformance_manifest,
    "TranscriptManifest",
    TranscriptManifest {
        schema_version: 2,
        message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
        provider_stack: ProviderStack::PiAi,
        created_at: timestamp(),
        migrated_from: None,
        migrated_at: None,
        backup_path: None,
    }
);
schema_case!(
    schema_conformance_session,
    "SessionEntry",
    SessionEntry {
        entry_type: SessionEntryType::Session,
        version: 3,
        id: non_empty("session"),
        timestamp: timestamp(),
        cwd: "/tmp".into(),
    }
);
schema_case!(
    schema_conformance_message,
    "MessageEntry",
    MessageEntry {
        entry_type: MessageEntryType::Message,
        id: non_empty("entry"),
        parent_id: None,
        timestamp: timestamp(),
        message: local_message(),
    }
);
schema_case!(
    schema_conformance_compaction,
    "CompactionEntry",
    CompactionEntry {
        entry_type: CompactionEntryType::Compaction,
        id: non_empty("entry"),
        parent_id: None,
        timestamp: timestamp(),
        summary: "summary".into(),
        first_kept_entry_id: None,
        tokens_before: 12,
        message: local_message(),
        details: Some(BoundedMap::new(BTreeMap::new()).unwrap_or_else(|e| panic!("map: {e}"))),
    }
);

#[test]
pub(super) fn schema_conformance_names_are_exact() {
    let covered = [
        "Agent",
        "Conversation",
        "LocalMessage",
        "Schedule",
        "ChannelAccount",
        "ChannelRoute",
        "MemoryBlockInput",
        "ProviderConnection",
        "ModelDescriptor",
        "Run",
        "TranscriptManifest",
        "SessionEntry",
        "MessageEntry",
        "CompactionEntry",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(covered.len(), 14);
    assert_eq!(covered, CANONICAL_ENTITY_NAMES.into_iter().collect());
}

#[test]
pub(super) fn schedule_all_canonical_fields_and_required_split() {
    let schedule_schema = &schema()["$defs"]["Schedule"];
    let properties = schedule_schema["properties"]
        .as_object()
        .map_or(0, Map::len);
    let required = schedule_schema["required"].as_array().map_or(0, Vec::len);
    let encoded =
        serde_json::to_value(schedule()).unwrap_or_else(|error| panic!("serialize: {error}"));
    assert_eq!(properties, 25);
    assert_eq!(required, 19);
    assert_eq!(properties - required, 6);
    let encoded_keys = encoded
        .as_object()
        .map(|map| map.keys().map(String::as_str).collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let property_keys = schedule_schema["properties"]
        .as_object()
        .map(|map| map.keys().map(String::as_str).collect::<BTreeSet<_>>())
        .unwrap_or_default();
    assert!(encoded_keys.is_subset(&property_keys));
    assert_eq!(encoded["jitter_offset_ms"], -500);
}

#[test]
pub(super) fn schedule_status_enum_is_exhaustive() {
    for status in ["active", "fired", "missed", "cancelled"] {
        let decoded: Result<ScheduleStatus, _> = serde_json::from_value(json!(status));
        assert!(decoded.is_ok());
    }
    assert!(serde_json::from_value::<ScheduleStatus>(json!("paused")).is_err());
}

#[test]
fn schedule_iana_acceptance_and_rejection() {
    assert!(IanaTimezone::new("UTC").is_ok());
    assert!(IanaTimezone::new("America/Los_Angeles").is_ok());
    assert!(IanaTimezone::new("PST").is_err());
    assert!(IanaTimezone::new("Mars/Olympus_Mons").is_err());
}

#[test]
fn channel_accounts_json_is_snake_case() {
    let value = serde_json::to_value(channel_account())
        .unwrap_or_else(|error| panic!("serialize: {error}"));
    assert!(value.get("group_policy").is_some());
    assert!(value.get("admin_users").is_some());
    assert!(value.get("user_allowed_commands").is_some());
    assert!(value.get("groupPolicy").is_none());
}

#[test]
fn channel_runtime_projection_is_camel_case() {
    let account = channel_account();
    let value = serde_json::to_value(account.runtime_projection())
        .unwrap_or_else(|error| panic!("serialize: {error}"));
    assert!(value.get("groupPolicy").is_some());
    assert!(value.get("adminUsers").is_some());
    assert!(value.get("userAllowedCommands").is_some());
    assert!(value.get("group_policy").is_none());
    let route = channel_route();
    let route_value = serde_json::to_value(route.runtime_projection())
        .unwrap_or_else(|error| panic!("serialize: {error}"));
    assert!(route_value.get("conversationId").is_some());
    assert!(route_value.get("conversation_id").is_none());
}

#[test]
pub(super) fn preserves_unknown_fields_agent_and_conversation() {
    let mut agent_value =
        serde_json::to_value(agent()).unwrap_or_else(|error| panic!("agent: {error}"));
    agent_value["future_flag"] = json!(true);
    let mut decoded: Agent =
        serde_json::from_value(agent_value).unwrap_or_else(|error| panic!("agent decode: {error}"));
    decoded.name = non_empty("Changed");
    let encoded = serde_json::to_value(decoded).unwrap_or_else(|error| panic!("agent: {error}"));
    assert_eq!(encoded["future_flag"], true);

    let mut conversation_value = serde_json::to_value(conversation())
        .unwrap_or_else(|error| panic!("conversation: {error}"));
    conversation_value["future_object"] = json!({"nested": [1, 2]});
    let decoded: Conversation = serde_json::from_value(conversation_value)
        .unwrap_or_else(|error| panic!("conversation decode: {error}"));
    let encoded =
        serde_json::to_value(decoded).unwrap_or_else(|error| panic!("conversation: {error}"));
    assert_eq!(encoded["future_object"], json!({"nested": [1, 2]}));
}

#[test]
fn transcript_discriminants_and_names() {
    let entry = CompactionEntry {
        entry_type: CompactionEntryType::Compaction,
        id: non_empty("entry"),
        parent_id: None,
        timestamp: timestamp(),
        summary: "s".into(),
        first_kept_entry_id: None,
        tokens_before: 10,
        message: local_message(),
        details: None,
    };
    let value = serde_json::to_value(TranscriptEntry::Compaction(entry))
        .unwrap_or_else(|error| panic!("transcript: {error}"));
    assert_eq!(value["type"], "compaction");
    assert!(value.get("parentId").is_some());
    assert!(value.get("firstKeptEntryId").is_some());
    assert!(value.get("tokensBefore").is_some());
    assert!(value.get("parent_id").is_none());
}

#[test]
fn nullable_presence_missing_null_and_value_round_trip() {
    let base =
        serde_json::to_value(channel_route()).unwrap_or_else(|error| panic!("route: {error}"));
    for (input, expected) in [
        (
            {
                let mut value = base.clone();
                value.as_object_mut().map(|map| map.remove("thread_id"));
                value
            },
            None,
        ),
        (
            {
                let mut value = base.clone();
                value["thread_id"] = Value::Null;
                value
            },
            Some(None),
        ),
        (
            {
                let mut value = base.clone();
                value["thread_id"] = json!("thread");
                value
            },
            Some(Some("thread".to_owned())),
        ),
    ] {
        let decoded: ChannelRoute =
            serde_json::from_value(input).unwrap_or_else(|error| panic!("decode: {error}"));
        assert_eq!(decoded.thread_id, expected);
        let encoded =
            serde_json::to_value(decoded).unwrap_or_else(|error| panic!("encode: {error}"));
        match expected {
            None => assert!(encoded.get("thread_id").is_none()),
            Some(None) => assert_eq!(encoded["thread_id"], Value::Null),
            Some(Some(value)) => assert_eq!(encoded["thread_id"], value),
        }
    }
}

#[test]
pub(super) fn channel_full_snake_and_camel_key_sets() {
    fn keys(value: &Value) -> BTreeSet<String> {
        value
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default()
    }
    for (name, canonical, runtime) in [
        (
            "ChannelAccount",
            serde_json::to_value(channel_account()),
            serde_json::to_value(channel_account().runtime_projection()),
        ),
        (
            "ChannelRoute",
            serde_json::to_value({
                let mut route = channel_route();
                route.thread_id = Some(Some("thread".into()));
                route
            }),
            serde_json::to_value(
                {
                    let mut route = channel_route();
                    route.thread_id = Some(Some("thread".into()));
                    route
                }
                .runtime_projection(),
            ),
        ),
    ] {
        let properties = schema()["$defs"][name]["properties"]
            .as_object()
            .map(|map| map.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let canonical_keys = keys(&canonical.unwrap_or_else(|error| panic!("canonical: {error}")));
        assert_eq!(canonical_keys, properties);
        let expected_runtime = properties
            .into_iter()
            .map(|key| {
                let mut output = String::new();
                let mut uppercase = false;
                for character in key.chars() {
                    if character == '_' {
                        uppercase = true;
                    } else if uppercase {
                        output.extend(character.to_uppercase());
                        uppercase = false;
                    } else {
                        output.push(character);
                    }
                }
                output
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys(&runtime.unwrap_or_else(|error| panic!("runtime: {error}"))),
            expected_runtime
        );
    }
}

#[allow(
    clippy::items_after_statements,
    reason = "local boundary fixture helpers"
)]
macro_rules! run_collection_boundary_classes { () => {
    fn strings(count: usize) -> Vec<String> {
        (0..count).map(|index| index.to_string()).collect()
    }
    for count in [STRING_ITEMS_MAX - 1, STRING_ITEMS_MAX] {
        assert!(BoundedVec::<String, STRING_ITEMS_MAX>::new(strings(count)).is_ok());
        assert!(
            serde_json::from_value::<BoundedVec<String, STRING_ITEMS_MAX>>(json!(strings(count)))
                .is_ok()
        );
    }
    assert!(BoundedVec::<String, STRING_ITEMS_MAX>::new(strings(STRING_ITEMS_MAX + 1)).is_err());
    assert!(
        serde_json::from_value::<BoundedVec<String, STRING_ITEMS_MAX>>(json!(strings(
            STRING_ITEMS_MAX + 1
        )))
        .is_err()
    );
    for count in [UNBOUNDED_COLLECTION_ITEMS_MAX - 1, UNBOUNDED_COLLECTION_ITEMS_MAX] {
        assert!(BoundedVec::<u8, UNBOUNDED_COLLECTION_ITEMS_MAX>::new(vec![0; count]).is_ok());
        assert!(
            serde_json::from_value::<BoundedVec<u8, UNBOUNDED_COLLECTION_ITEMS_MAX>>(json!(
                vec![0; count]
            ))
            .is_ok()
        );
    }
    assert!(
        BoundedVec::<u8, UNBOUNDED_COLLECTION_ITEMS_MAX>::new(vec![
            0;
            UNBOUNDED_COLLECTION_ITEMS_MAX
                + 1
        ])
        .is_err()
    );
    assert!(
        serde_json::from_value::<BoundedVec<u8, UNBOUNDED_COLLECTION_ITEMS_MAX>>(json!(
            vec![0; UNBOUNDED_COLLECTION_ITEMS_MAX + 1]
        ))
        .is_err()
    );
    fn object(count: usize) -> BTreeMap<String, Value> {
        (0..count).map(|index| (index.to_string(), Value::Null)).collect()
    }
    for count in [UNBOUNDED_MAP_FIELDS_MAX - 1, UNBOUNDED_MAP_FIELDS_MAX] {
        assert!(BoundedMap::<UNBOUNDED_MAP_FIELDS_MAX>::new(object(count)).is_ok());
        assert!(
            serde_json::from_value::<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>(
                serde_json::to_value(object(count)).unwrap_or_else(|e| panic!("object: {e}"))
            )
            .is_ok()
        );
    }
    assert!(
        BoundedMap::<UNBOUNDED_MAP_FIELDS_MAX>::new(object(UNBOUNDED_MAP_FIELDS_MAX + 1)).is_err()
    );
    assert!(
        serde_json::from_value::<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>(
            serde_json::to_value(object(UNBOUNDED_MAP_FIELDS_MAX + 1))
                .unwrap_or_else(|e| panic!("object: {e}"))
        )
        .is_err()
    );
    for count in [EXTRAS_FIELDS_MAX - 1, EXTRAS_FIELDS_MAX] {
        assert!(EntityExtras::new(object(count), &[]).is_ok());
    }
    assert!(EntityExtras::new(object(EXTRAS_FIELDS_MAX + 1), &[]).is_err());
    assert!(
        serde_json::from_value::<EntityExtras>(
            serde_json::to_value(object(EXTRAS_FIELDS_MAX + 1))
                .unwrap_or_else(|e| panic!("extras: {e}"))
        )
        .is_err()
    );
};
}

#[test]
fn collection_boundary_classes_below_at_above() {
    run_collection_boundary_classes!();
}

#[test]
fn nested_json_boundaries_are_enforced() {
    let below = Value::Array(vec![Value::Null; JSON_ITEMS_MAX - 1]);
    let at = Value::Array(vec![Value::Null; JSON_ITEMS_MAX]);
    let above = Value::Array(vec![Value::Null; JSON_ITEMS_MAX + 1]);
    assert!(BoundedMap::<1>::new(BTreeMap::from([("value".into(), below)])).is_ok());
    assert!(BoundedMap::<1>::new(BTreeMap::from([("value".into(), at)])).is_ok());
    assert!(BoundedMap::<1>::new(BTreeMap::from([("value".into(), above)])).is_err());

    let nested_object = |count| {
        Value::Object(
            (0..count)
                .map(|index| (index.to_string(), Value::Null))
                .collect(),
        )
    };
    assert!(
        BoundedMap::<1>::new(BTreeMap::from([(
            "value".into(),
            nested_object(JSON_PROPERTIES_MAX)
        )]))
        .is_ok()
    );
    assert!(
        BoundedMap::<1>::new(BTreeMap::from([(
            "value".into(),
            nested_object(JSON_PROPERTIES_MAX + 1)
        )]))
        .is_err()
    );

    let nested = |depth| {
        let mut value = Value::Null;
        for _ in 0..depth {
            value = json!([value]);
        }
        value
    };
    assert!(
        BoundedMap::<1>::new(BTreeMap::from([("value".into(), nested(JSON_DEPTH_MAX))])).is_ok()
    );
    assert!(
        BoundedMap::<1>::new(BTreeMap::from([(
            "value".into(),
            nested(JSON_DEPTH_MAX + 2)
        )]))
        .is_err()
    );
}

fn assert_presence_round_trip<T>(base: &Value, field: &str, valid: Value)
where
    T: serde::de::DeserializeOwned + Serialize,
{
    for state in [None, Some(Value::Null), Some(valid)] {
        let mut input = base.clone();
        if let Some(value) = state.clone() {
            input[field] = value;
        } else {
            input.as_object_mut().map(|map| map.remove(field));
        }
        let decoded: T =
            serde_json::from_value(input).unwrap_or_else(|error| panic!("{field} decode: {error}"));
        let encoded =
            serde_json::to_value(decoded).unwrap_or_else(|error| panic!("{field} encode: {error}"));
        match state {
            None => assert!(encoded.get(field).is_none(), "{field}"),
            Some(value) => assert_eq!(encoded[field], value, "{field}"),
        }
    }
}

#[test]
fn all_presence_aware_fields_preserve_three_states() {
    let timestamp = json!("2026-08-14T12:34:56Z");
    assert_presence_round_trip::<MemoryBlockInput>(
        &json!({"label":"human","value":"v"}),
        "description",
        json!("description"),
    );
    let agent = serde_json::to_value(agent()).unwrap_or_else(|error| panic!("agent: {error}"));
    for (field, valid) in [
        ("description", json!("description")),
        ("hidden", json!(true)),
        ("compaction_settings", json!({"mode":"auto"})),
    ] {
        assert_presence_round_trip::<Agent>(&agent, field, valid);
    }
    let conversation = serde_json::to_value(conversation())
        .unwrap_or_else(|error| panic!("conversation: {error}"));
    for (field, valid) in [
        ("archived_at", timestamp.clone()),
        ("last_message_at", timestamp.clone()),
        ("summary", json!("summary")),
        ("model", json!("provider/model")),
    ] {
        assert_presence_round_trip::<Conversation>(&conversation, field, valid);
    }
    let run = json!({
        "id":"local-run-1",
        "agent_id":"agent-local-test",
        "conversation_id":"default",
        "status":"running",
        "created_at":timestamp.clone()
    });
    for (field, valid) in [
        ("stop_reason", json!("complete")),
        ("completed_at", timestamp.clone()),
        ("background", json!(true)),
    ] {
        assert_presence_round_trip::<Run>(&run, field, valid);
    }
    let schedule =
        serde_json::to_value(schedule()).unwrap_or_else(|error| panic!("schedule: {error}"));
    for (field, valid) in [
        ("last_run_at", timestamp),
        ("last_run_outcome", json!("queued")),
        ("last_run_reason", json!("reason")),
        ("last_run_error", json!("error")),
    ] {
        assert_presence_round_trip::<Schedule>(&schedule, field, valid);
    }
    let route =
        serde_json::to_value(channel_route()).unwrap_or_else(|error| panic!("route: {error}"));
    assert_presence_round_trip::<ChannelRoute>(&route, "thread_id", json!("thread"));
}

#[test]
fn bounded_content_and_container_serde_reject_overbound_json() {
    fn message(content: &Value) -> Value {
        json!({"id":"ui-msg-1","role":"user","content":content,"timestamp":1})
    }
    for count in [JSON_ITEMS_MAX - 1, JSON_ITEMS_MAX] {
        assert!(serde_json::from_value::<LocalMessage>(message(&json!(vec![0; count]))).is_ok());
    }
    assert!(
        serde_json::from_value::<LocalMessage>(message(&json!(vec![0; JSON_ITEMS_MAX + 1])))
            .is_err()
    );
    let object = |count| {
        Value::Object(
            (0..count)
                .map(|index| (index.to_string(), Value::Null))
                .collect(),
        )
    };
    assert!(serde_json::from_value::<LocalMessage>(message(&object(JSON_PROPERTIES_MAX))).is_ok());
    assert!(
        serde_json::from_value::<LocalMessage>(message(&object(JSON_PROPERTIES_MAX + 1))).is_err()
    );
    let nested = |depth| {
        let mut value = Value::Null;
        for _ in 0..depth {
            value = json!([value]);
        }
        value
    };
    assert!(serde_json::from_value::<LocalMessage>(message(&nested(JSON_DEPTH_MAX))).is_ok());
    assert!(serde_json::from_value::<LocalMessage>(message(&nested(JSON_DEPTH_MAX + 1))).is_err());
    let map = |value| json!({"value":value});
    assert!(serde_json::from_value::<BoundedMap<1>>(map(json!(vec![0; JSON_ITEMS_MAX]))).is_ok());
    assert!(
        serde_json::from_value::<BoundedMap<1>>(map(json!(vec![0; JSON_ITEMS_MAX + 1]))).is_err()
    );
    assert!(serde_json::from_value::<EntityExtras>(map(object(JSON_PROPERTIES_MAX))).is_ok());
    assert!(serde_json::from_value::<EntityExtras>(map(object(JSON_PROPERTIES_MAX + 1))).is_err());
}
