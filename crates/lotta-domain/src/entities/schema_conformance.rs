use super::*;
use jsonschema::Validator;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fmt::Debug};

const EXPECTED_NAMES: [&str; 14] = [
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
    serde_json::from_str(include_str!(
        "../../../../.specs/canonical-types.schema.json"
    ))
    .unwrap_or_else(|e| panic!("schema: {e}"))
}
fn validator(name: &str) -> Validator {
    let root = schema();
    let mut definition = root["$defs"][name].clone();
    if let Value::Object(map) = &mut definition {
        map.insert("$defs".into(), root["$defs"].clone());
    }
    jsonschema::validator_for(&definition).unwrap_or_else(|e| panic!("validator: {e}"))
}
fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}
fn check<T>(name: &str, minimal: Value, complete: Value)
where
    T: DeserializeOwned + Serialize + PartialEq + Debug,
{
    let definition = schema()["$defs"][name].clone();
    let required = definition["required"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let properties = definition["properties"]
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    assert_eq!(keys(&minimal), required, "{name} minimal");
    assert_eq!(keys(&complete), properties, "{name} complete");
    for sample in [minimal, complete] {
        assert!(validator(name).is_valid(&sample), "{name}: {sample}");
        let decoded: T =
            serde_json::from_value(sample.clone()).unwrap_or_else(|e| panic!("{name} decode: {e}"));
        let encoded =
            serde_json::to_value(&decoded).unwrap_or_else(|e| panic!("{name} encode: {e}"));
        assert!(validator(name).is_valid(&encoded));
        let again: T =
            serde_json::from_value(encoded).unwrap_or_else(|e| panic!("{name} re-decode: {e}"));
        assert_eq!(decoded, again);
    }
}
fn t() -> Value {
    json!("2026-08-14T12:34:56Z")
}
fn msg(minimal: bool) -> Value {
    if minimal {
        json!({"id":"ui-msg-1","role":"user","timestamp":1})
    } else {
        json!({"id":"ui-msg-1",
            "role":"assistant",
            "content":{"text":"hi"},
            "timestamp":1,
            "metadata":{"source":"test"}})
    }
}

macro_rules! run_all_fixture_cases { () => {
    let mut covered = BTreeSet::new();
    macro_rules! case {
        ($name:literal,$ty:ty,$minimal:expr,$complete:expr) => {{
            covered.insert($name);
            check::<$ty>($name, $minimal, $complete);
        }};
    }
    case!(
        "MemoryBlockInput",
        MemoryBlockInput,
        json!({"label":"human","value":"v"}),
        json!({"label":"human","value":"v","description":null})
    );
    case!(
        "Agent",
        Agent,
        json!({"id":"agent-local-test",
            "name":"Agent",
            "system":"",
            "tags":[],
            "model":"p/m",
            "model_settings":{}}),
        json!({"id":"agent-local-test",
            "name":"Agent",
            "description":null,
            "system":"s",
            "tags":["x"],
            "model":"p/m",
            "model_settings":{"temperature":1},
            "hidden":null,
            "compaction_settings":{"mode":"x"}})
    );
    case!(
        "Conversation",
        Conversation,
        json!({"id":"default",
            "agent_id":"agent-local-test",
            "archived":false,
            "created_at":t(),
            "updated_at":t(),
            "in_context_message_ids":[]}),
        json!({"id":"default",
            "agent_id":"agent-local-test",
            "archived":true,
            "archived_at":null,
            "created_at":t(),
            "updated_at":t(),
            "last_message_at":null,
            "summary":null,
            "in_context_message_ids":["ui-msg-1"],
            "model":null,
            "model_settings":{"x":1},
            "context_window_limit":1,
            "hidden":false,
            "tags":["x"]})
    );
    case!("LocalMessage", LocalMessage, msg(true), msg(false));
    case!(
        "TranscriptManifest",
        TranscriptManifest,
        json!({"schema_version":2,
            "message_format":"pi-session-entry-jsonl",
            "provider_stack":"pi-ai",
            "created_at":t()}),
        json!({"schema_version":2,
            "message_format":"pi-session-entry-jsonl",
            "provider_stack":"pi-ai",
            "created_at":t(),
            "migrated_from":"legacy",
            "migrated_at":t(),
            "backup_path":"/tmp/x"})
    );
    case!(
        "SessionEntry",
        SessionEntry,
        json!({"type":"session","version":3,"id":"s","timestamp":t(),"cwd":"/tmp"}),
        json!({"type":"session","version":3,"id":"s","timestamp":t(),"cwd":"/tmp"})
    );
    case!(
        "MessageEntry",
        MessageEntry,
        json!({"type":"message","id":"e","parentId":null,"timestamp":t(),"message":msg(true)}),
        json!({"type":"message","id":"e","parentId":"p","timestamp":t(),"message":msg(false)})
    );
    case!(
        "CompactionEntry",
        CompactionEntry,
        json!({"type":"compaction",
            "id":"e",
            "parentId":null,
            "timestamp":t(),
            "summary":"s",
            "firstKeptEntryId":null,
            "tokensBefore":0,
            "message":msg(true)}),
        json!({"type":"compaction",
            "id":"e",
            "parentId":"p",
            "timestamp":t(),
            "summary":"s",
            "firstKeptEntryId":"k",
            "tokensBefore":1,
            "message":msg(false),
            "details":{"x":1}})
    );
    case!(
        "ProviderConnection",
        ProviderConnection,
        json!({"provider_id":"p","auth_method":"key","connected":false}),
        json!({"provider_id":"p","auth_method":"key","connected":true,"metadata":{"x":1}})
    );
    case!(
        "ModelDescriptor",
        ModelDescriptor,
        json!({"handle":"p/m","provider_id":"p","available":false}),
        json!({"handle":"p/m",
            "provider_id":"p",
            "available":true,
            "context_window":1,
            "model_settings":{"x":1}})
    );
    case!(
        "Run",
        Run,
        json!({"id":"local-run-1",
            "agent_id":"agent-local-test",
            "conversation_id":"default",
            "status":"running",
            "created_at":t()}),
        json!({"id":"local-run-1",
            "agent_id":"agent-local-test",
            "conversation_id":"default",
            "status":"completed",
            "stop_reason":null,
            "created_at":t(),
            "completed_at":null,
            "background":null,
            "metadata":{"x":1},
            "usage":{"tokens":1}})
    );
    case!(
        "Schedule",
        Schedule,
        json!({"id":"s",
            "agent_id":"agent-local-test",
            "conversation_id":"default",
            "name":"n",
            "description":"",
            "cron":"* * * * *",
            "timezone":"UTC",
            "recurring":false,
            "prompt":"p",
            "status":"active",
            "created_at":t(),
            "expires_at":null,
            "last_fired_at":null,
            "fire_count":0,
            "cancel_reason":null,
            "jitter_offset_ms":0,
            "scheduled_for":null,
            "fired_at":null,
            "missed_at":null}),
        json!({"id":"s",
            "agent_id":"agent-local-test",
            "conversation_id":"default",
            "name":"n",
            "description":"d",
            "cron":"* * * * *",
            "timezone":"UTC",
            "recurring":true,
            "prompt":"p",
            "status":"active",
            "created_at":t(),
            "expires_at":null,
            "last_fired_at":null,
            "fire_count":1,
            "cancel_reason":null,
            "jitter_offset_ms":-1,
            "last_run_at":null,
            "last_run_outcome":null,
            "last_run_reason":null,
            "last_run_error":null,
            "missed_count":0,
            "failed_count":0,
            "scheduled_for":null,
            "fired_at":null,
            "missed_at":null})
    );
    case!(
        "ChannelAccount",
        ChannelAccount,
        json!({"channel_id":"c",
            "account_id":"a",
            "enabled":true,
            "configured":true,
            "running":false,
            "dm_policy":"open",
            "allowed_users":[],
            "config":{},
            "created_at":t(),
            "updated_at":t()}),
        json!({"channel_id":"c",
            "account_id":"a",
            "display_name":"A",
            "enabled":true,
            "configured":true,
            "running":true,
            "dm_policy":"allowlist",
            "group_policy":"open",
            "allowed_users":["u"],
            "admin_users":["a"],
            "user_allowed_commands":["status"],
            "config":{"x":1},
            "created_at":t(),
            "updated_at":t()})
    );
    case!(
        "ChannelRoute",
        ChannelRoute,
        json!({"channel_id":"c",
            "account_id":"a",
            "chat_id":"x",
            "agent_id":"agent-local-test",
            "conversation_id":"default",
            "enabled":true,
            "created_at":t(),
            "updated_at":t()}),
        json!({"channel_id":"c",
            "account_id":"a",
            "chat_id":"x",
            "chat_type":"direct",
            "thread_id":null,
            "agent_id":"agent-local-test",
            "conversation_id":"default",
            "enabled":true,
            "outbound_enabled":true,
            "created_at":t(),
            "updated_at":t()})
    );
    assert_eq!(covered, EXPECTED_NAMES.into_iter().collect());
};
}

#[test]
fn all_fourteen_minimal_and_complete_pairs() {
    run_all_fixture_cases!();
}
