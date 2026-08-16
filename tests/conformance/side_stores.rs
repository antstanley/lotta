use super::support;
use lotta_store::ChannelFile;
use lotta_store::side::{channels, crons, settings};
use serde_json::Value;
use std::path::Path;

fn ts_to_rust(artifact: &str) {
    let root = support::TestRoot::new(&format!("side-ts-{artifact}"));
    let response = support::run_ts(&root, "side_write", artifact);
    let paths = support::side_paths(&root);
    let store = support::backend_store(&root);
    let value = rust_read(&store, &paths, artifact);
    support::assert_contains(
        &response["value"],
        &value,
        "",
        "typescript-to-rust",
        artifact,
    );
    if artifact == "provider_auth" {
        assert_provider_mode(&root.backend().join("providers/auth.json"));
    }
}

fn rust_to_ts(artifact: &str) {
    let root = support::TestRoot::new(&format!("side-rust-{artifact}"));
    let lease = support::Lease::acquire(root.path(), "rust-side-writer")
        .expect("side writer handoff lease");
    let paths = support::side_paths(&root);
    let store = support::backend_store(&root);
    rust_write(&store, &paths, artifact);
    let before = rust_read(&store, &paths, artifact);
    drop(store);
    drop(paths);
    drop(lease);
    let response = support::run_ts(&root, "side_read_update", artifact);
    support::assert_contains(
        &before,
        &response["value"],
        "",
        "rust-to-typescript",
        artifact,
    );
}

fn rust_read(
    store: &lotta_store::LocalStore,
    paths: &lotta_store::SidePaths,
    artifact: &str,
) -> Value {
    match artifact {
        "provider_auth" => support::read_json(&store.paths().provider_auth()),
        "crons" => parse_json(crons::read(paths).expect("read crons").bytes()),
        "run_log" => Value::Array(parse_rows(
            crons::read_run(paths, support::SCHEDULE_ID)
                .expect("read run")
                .bytes(),
        )),
        "settings" => parse_json(settings::read(paths).expect("read settings").bytes()),
        "channels" => channel_value(paths),
        _ => panic!("unsupported side artifact"),
    }
}

fn rust_write(store: &lotta_store::LocalStore, paths: &lotta_store::SidePaths, artifact: &str) {
    match artifact {
        "provider_auth" => {
            let path = store.paths().provider_auth();
            lotta_store::atomic_write(
                &path,
                &provider_bytes(),
                lotta_store::WriteMode::ProviderAuth,
            )
            .expect("provider write");
        }
        "crons" => crons::write(paths, &cron_bytes()).expect("cron write"),
        "run_log" => {
            crons::write_run(paths, support::SCHEDULE_ID, &run_bytes()).expect("run write")
        }
        "settings" => settings::write(paths, &settings_bytes()).expect("settings write"),
        "channels" => write_channels(paths),
        _ => panic!("unsupported side artifact"),
    }
}

fn write_channels(paths: &lotta_store::SidePaths) {
    channels::write_pending(paths, &pending_control_bytes()).expect("pending");
    for (file, bytes) in [
        (
            ChannelFile::Config,
            b"enabled: true\nagent_id: agent-local-fixture\n".to_vec(),
        ),
        (ChannelFile::Accounts, channel_accounts_bytes()),
        (ChannelFile::Routing, channel_routes_bytes()),
        (ChannelFile::Pairing, channel_pairing_bytes()),
        (ChannelFile::Targets, channel_targets_bytes()),
    ] {
        channels::write(paths, "telegram", file, &bytes).expect("channel write");
    }
    let opaque = paths
        .channel_dir("telegram")
        .expect("channel dir")
        .join("plugin/nested/opaque.bin");
    std::fs::create_dir_all(opaque.parent().expect("opaque parent")).expect("opaque dir");
    std::fs::write(opaque, b"SANITIZED_PLUGIN_BYTES").expect("opaque write");
}

fn channel_value(paths: &lotta_store::SidePaths) -> Value {
    let root = paths.channels_root().expect("channels root");
    let channel = paths.channel_dir("telegram").expect("channel dir");
    let mut map = serde_json::Map::new();
    map.insert(
        "pending-control-requests.json".to_owned(),
        Value::String(read_text(&root.join("pending-control-requests.json"))),
    );
    for relative in channels::list(paths, "telegram").expect("channel inventory") {
        map.insert(
            format!("telegram/{}", relative.display()),
            Value::String(read_text(&channel.join(relative))),
        );
    }
    Value::Object(map)
}

fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read side text")
}

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("parse side json")
}

fn parse_rows(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse side row"))
        .collect()
}

fn json_fixture(value: Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&value).expect("fixture JSON");
    bytes.push(b'\n');
    bytes
}

fn pending_control_bytes() -> Vec<u8> {
    json_fixture(serde_json::json!({
        "requests": [{
            "requestId": "request-1",
            "source": {
                "channel": "telegram",
                "chatId": "chat-1",
                "agentId": "agent-local-fixture",
                "conversationId": "default"
            },
            "toolName": "synthetic_tool",
            "input": {"synthetic": true}
        }]
    }))
}

fn channel_accounts_bytes() -> Vec<u8> {
    json_fixture(serde_json::json!({"accounts": [{
        "channel": "telegram",
        "accountId": "account-1",
        "displayName": "Telegram",
        "enabled": true,
        "dmPolicy": "pairing",
        "allowedUsers": [],
        "binding": {"agentId": null, "conversationId": null},
        "groupMode": "open",
        "transcribeVoice": false,
        "richPrivateChatDefault": true,
        "richDraftStreaming": false,
        "createdAt": "2026-08-14T00:00:00.000Z",
        "updatedAt": "2026-08-14T00:00:00.000Z"
    }]}))
}

fn channel_routes_bytes() -> Vec<u8> {
    json_fixture(serde_json::json!({"routes": [{
        "accountId": "account-1",
        "chatId": "chat-1",
        "chatType": "direct",
        "threadId": null,
        "agentId": "agent-local-fixture",
        "conversationId": "default",
        "enabled": true,
        "outboundEnabled": true,
        "createdAt": "2026-08-14T00:00:00.000Z",
        "updatedAt": "2026-08-14T00:00:00.000Z"
    }]}))
}

fn channel_pairing_bytes() -> Vec<u8> {
    json_fixture(serde_json::json!({
        "pending": [{
            "accountId": "account-1",
            "code": "ABC123",
            "senderId": "sender-1",
            "senderName": "synthetic",
            "chatId": "chat-1",
            "createdAt": "2026-08-14T00:00:00.000Z",
            "expiresAt": "2099-08-14T00:00:00.000Z"
        }],
        "approved": []
    }))
}

fn channel_targets_bytes() -> Vec<u8> {
    json_fixture(serde_json::json!({"targets": [{
        "accountId": "account-1",
        "targetId": "chat-1",
        "targetType": "direct",
        "chatId": "chat-1",
        "label": "synthetic",
        "discoveredAt": "2026-08-14T00:00:00.000Z",
        "lastSeenAt": "2026-08-14T00:00:00.000Z",
        "lastMessageId": "message-1"
    }]}))
}

fn provider_bytes() -> Vec<u8> {
    json_fixture(serde_json::json!({
        "version": 1,
        "providers": {"synthetic-openai": {
            "id": "local-provider-synthetic-openai",
            "name": "synthetic-openai",
            "provider_type": "openai",
            "provider_category": "byok",
            "auth": {"type": "api", "key": "<redacted-fixture>"},
            "created_at": "2026-08-14T00:00:00.000Z",
            "updated_at": "2026-08-14T00:00:00.000Z"
        }}
    }))
}

fn cron_bytes() -> Vec<u8> {
    json_fixture(serde_json::json!({
        "version": 1,
        "scheduler_owner": null,
        "tasks": [{
            "id": "schedule-fixture",
            "agent_id": "agent-local-fixture",
            "conversation_id": "default",
            "name": "synthetic",
            "description": "synthetic",
            "cron": "0 * * * *",
            "timezone": "UTC",
            "recurring": true,
            "prompt": "synthetic",
            "status": "active",
            "created_at": "2026-08-14T00:00:00.000Z",
            "expires_at": null,
            "last_fired_at": null,
            "fire_count": 0,
            "cancel_reason": null,
            "jitter_offset_ms": 0,
            "last_run_at": null,
            "last_run_outcome": null,
            "last_run_reason": null,
            "last_run_error": null,
            "last_missed_at": null,
            "missed_count": 0,
            "failed_count": 0,
            "scheduled_for": null,
            "fired_at": null,
            "missed_at": null
        }]
    }))
}

fn run_bytes() -> Vec<u8> {
    json_fixture(serde_json::json!({
        "ts": 1_700_000_000_000_i64,
        "jobId": "schedule-fixture",
        "action": "finished",
        "status": "ok",
        "summary": "synthetic"
    }))
}

fn settings_bytes() -> Vec<u8> {
    json_fixture(serde_json::json!({
        "version": 1,
        "model": "openai/gpt-4.1-mini",
        "extension": {"retained": true}
    }))
}

#[cfg(unix)]
fn assert_provider_mode(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(path)
            .expect("provider metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[cfg(not(unix))]
fn assert_provider_mode(_: &Path) {}

macro_rules! directional_tests {
    ($module:ident, $artifact:literal) => {
        mod $module {
            #[test]
            fn typescript_to_rust() {
                super::ts_to_rust($artifact);
            }
            #[test]
            fn rust_to_typescript() {
                super::rust_to_ts($artifact);
            }
        }
    };
}

directional_tests!(provider_auth_v1, "provider_auth");
directional_tests!(crons_json, "crons");
directional_tests!(run_log_jsonl, "run_log");
directional_tests!(settings_json, "settings");
directional_tests!(channel_tree, "channels");
