use super::*;

const GLOBAL: &str = "side_stores/home/.letta/settings.json";
const CRONS: &str = "side_stores/letta_home/crons.json";
const RUNS: &str = "side_stores/letta_home/runs/schedule-fixture.jsonl";
const AUTH: &str = "side_stores/providers/auth.json";
const PENDING: &str = "side_stores/home/.letta/channels/pending-control-requests.json";
const CONFIG: &str = "side_stores/home/.letta/channels/telegram/config.yaml";
const ACCOUNTS: &str = "side_stores/home/.letta/channels/telegram/accounts.json";
const ROUTING: &str = "side_stores/home/.letta/channels/telegram/routing.yaml";
const PAIRING: &str = "side_stores/home/.letta/channels/telegram/pairing.yaml";
const TARGETS: &str = "side_stores/home/.letta/channels/telegram/targets.json";
const PROJECT: &str = "side_stores/workspace/.letta/settings.json";
const LOCAL: &str = "side_stores/workspace/.letta/settings.local.json";

fn assert_types(value: &Value, fields: &[(&str, &str)]) {
    for (field, kind) in fields {
        let item = &value[*field];
        let valid = match *kind {
            "string" => item.is_string(),
            "bool" => item.is_boolean(),
            "number" => item.is_number(),
            "array" => item.is_array(),
            "object" => item.is_object(),
            "null" => item.is_null(),
            _ => false,
        };
        assert!(valid, "{field} must be {kind}");
    }
}

#[test]
fn home_settings() {
    assert_index_format(GLOBAL, "json");
    let value = load_json(GLOBAL);
    let required = [
        "autoConversationTitles",
        "autoSwapOnQuotaLimit",
        "conversationSwitchAlertEnabled",
        "includeWorktreeTool",
        "lastAgent",
        "memoryReminderInterval",
        "reasoningTabCycleEnabled",
        "recentModels",
        "reflectionMerge",
        "reflectionMergeInstructions",
        "reflectionStepCount",
        "reflectionTrigger",
        "sessionContextEnabled",
        "tokenStreaming",
    ];
    assert!(
        required
            .iter()
            .all(|field| object(&value).contains_key(*field))
    );
    assert_types(
        &value,
        &[
            ("tokenStreaming", "bool"),
            ("recentModels", "array"),
            ("reflectionStepCount", "number"),
            ("reflectionMergeInstructions", "string"),
        ],
    );
    assert_eq!(value["lastAgent"], Value::Null);
    assert_eq!(value["sessionContextEnabled"], true);
    assert!(value.get("env").is_none());
}

#[test]
fn crons() {
    assert_index_format(CRONS, "json");
    let value = load_json(CRONS);
    assert_eq!(keys(&value), ["scheduler_owner", "tasks", "version"]);
    assert_eq!(value["version"], 1);
    assert_eq!(value["scheduler_owner"], Value::Null);
    let task = &value["tasks"][0];
    let expected = [
        "agent_id",
        "cancel_reason",
        "conversation_id",
        "created_at",
        "cron",
        "description",
        "expires_at",
        "failed_count",
        "fire_count",
        "fired_at",
        "id",
        "jitter_offset_ms",
        "last_fired_at",
        "last_missed_at",
        "last_run_at",
        "last_run_error",
        "last_run_outcome",
        "last_run_reason",
        "missed_at",
        "missed_count",
        "name",
        "prompt",
        "recurring",
        "scheduled_for",
        "status",
        "timezone",
    ];
    assert_eq!(keys(task), expected);
    assert_types(
        task,
        &[
            ("id", "string"),
            ("recurring", "bool"),
            ("fire_count", "number"),
            ("missed_count", "number"),
            ("failed_count", "number"),
        ],
    );
    assert_eq!(task["agent_id"], AGENT_ID);
    assert_eq!(task["conversation_id"], CONVERSATION_ID);
    assert_eq!(task["status"], "active");
}

#[test]
fn schedule_run() {
    assert_index_format(RUNS, "jsonl");
    let values = rows(RUNS);
    assert_eq!(values.len(), 1);
    for row in values {
        assert_types(
            &row,
            &[("ts", "number"), ("jobId", "string"), ("action", "string")],
        );
        assert_eq!(row["jobId"], "schedule-fixture");
        assert_eq!(row["action"], "finished");
        assert_eq!(row["status"], "ok");
        assert_eq!(row["agentId"], AGENT_ID);
        assert_eq!(row["conversationId"], CONVERSATION_ID);
    }
}

#[test]
fn provider_auth_v1() {
    assert_index_format(AUTH, "json");
    let value = load_json(AUTH);
    assert_eq!(keys(&value), ["providers", "version"]);
    assert_eq!(value["version"], 1);
    let record = &value["providers"]["fixture-provider"];
    assert_eq!(
        keys(record),
        [
            "auth",
            "base_url",
            "created_at",
            "id",
            "name",
            "provider_category",
            "provider_type",
            "updated_at"
        ]
    );
    assert_types(
        record,
        &[
            ("id", "string"),
            ("base_url", "string"),
            ("created_at", "string"),
            ("updated_at", "string"),
        ],
    );
    assert_eq!(record["provider_category"], "byok");
    assert_eq!(record["provider_type"], "openai");
    assert_eq!(keys(&record["auth"]), ["key", "type"]);
    assert_eq!(record["auth"]["type"], "api");
    assert_eq!(record["auth"]["key"], "<redacted-fixture>");
}

#[test]
fn pending_control_requests() {
    assert_index_format(PENDING, "json");
    let value = load_json(PENDING);
    assert_eq!(keys(&value), ["requests"]);
    assert!(value["requests"].as_array().expect("requests").is_empty());
}

fn parse_yaml(text: &str) -> Map<String, Value> {
    let mut output = Map::new();
    for line in text.lines() {
        let (key, raw) = line.split_once(':').expect("bounded YAML key/value");
        let raw = raw.trim();
        let value = match raw {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "[]" => Value::Array(Vec::new()),
            _ => Value::String(raw.to_owned()),
        };
        assert!(output.insert(key.to_owned(), value).is_none());
    }
    output
}

#[test]
fn channel_config() {
    assert_index_format(CONFIG, "yaml");
    let value = parse_yaml(&load_text(CONFIG));
    let mut actual = value.keys().map(String::as_str).collect::<Vec<_>>();
    actual.sort_unstable();
    assert_eq!(
        actual,
        [
            "allowed_users",
            "dm_policy",
            "enabled",
            "group_mode",
            "rich_draft_streaming",
            "rich_private_chat_default",
            "token",
            "transcribe_voice"
        ]
    );
    assert_eq!(value["enabled"], false);
    assert_eq!(value["token"], "<redacted-fixture>");
    assert_eq!(value["dm_policy"], "pairing");
    assert_eq!(value["group_mode"], "open");
    assert_eq!(value["allowed_users"], Value::Array(Vec::new()));
    assert_eq!(value["transcribe_voice"], false);
}

#[test]
fn channel_accounts() {
    assert_index_format(ACCOUNTS, "json");
    let value = load_json(ACCOUNTS);
    assert_eq!(keys(&value), ["accounts"]);
    assert!(value["accounts"].as_array().expect("accounts").is_empty());
}

#[test]
fn channel_routing() {
    assert_index_format(ROUTING, "json");
    let value = load_json(ROUTING);
    assert_eq!(keys(&value), ["routes"]);
    for route in value["routes"].as_array().expect("routes") {
        assert_types(
            route,
            &[
                ("chatId", "string"),
                ("agentId", "string"),
                ("conversationId", "string"),
                ("enabled", "bool"),
                ("createdAt", "string"),
            ],
        );
    }
}

#[test]
fn channel_pairing() {
    assert_index_format(PAIRING, "json");
    let value = load_json(PAIRING);
    assert_eq!(keys(&value), ["approved", "pending"]);
    assert!(value["pending"].as_array().expect("pending").is_empty());
    assert!(value["approved"].as_array().expect("approved").is_empty());
}

#[test]
fn channel_targets() {
    assert_index_format(TARGETS, "json");
    let value = load_json(TARGETS);
    assert_eq!(keys(&value), ["targets"]);
    for target in value["targets"].as_array().expect("targets") {
        assert_types(
            target,
            &[
                ("targetId", "string"),
                ("targetType", "string"),
                ("chatId", "string"),
                ("label", "string"),
                ("discoveredAt", "string"),
                ("lastSeenAt", "string"),
            ],
        );
    }
}

#[test]
fn workspace_settings() {
    assert_index_format(PROJECT, "json");
    let value = load_json(PROJECT);
    assert!(
        object(&value)
            .keys()
            .all(|key| matches!(key.as_str(), "hooks" | "windowTitle"))
    );
    assert!(object(&value).is_empty());
}

#[test]
fn workspace_local_settings() {
    assert_index_format(LOCAL, "json");
    let value = load_json(LOCAL);
    let supported = [
        "hooks",
        "lastAgent",
        "lastSession",
        "listenerEnvName",
        "memoryReminderInterval",
        "permissions",
        "profiles",
        "reflectionMerge",
        "reflectionMergeInstructions",
        "reflectionSettingsByAgent",
        "reflectionStepCount",
        "reflectionTrigger",
        "sessionsByServer",
        "windowTitle",
    ];
    assert!(
        object(&value)
            .keys()
            .all(|key| supported.contains(&key.as_str()))
    );
    assert_eq!(keys(&value), ["lastAgent"]);
    assert_eq!(value["lastAgent"], AGENT_ID);
}
