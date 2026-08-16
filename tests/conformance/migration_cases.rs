use super::support;
use lotta_domain::{
    Agent, Conversation, ProviderStack, SessionEntry, SessionEntryType, TranscriptEntry,
    TranscriptManifest, TranscriptMessageFormat,
};
use lotta_runtime::ports::{AgentStore, ConversationStore};
use lotta_store::StoreErrorKind;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const DEFAULT_KEY: &str = "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl";
const ACTUAL_KEY: &str = "Y29udmVyc2F0aW9uOmNvbnZlcnNhdGlvbi1maXh0dXJl";
const LEGACY_TEXT: &str = "SANITIZED_FIXTURE_LEGACY_UI";
const VERSIONED_TEXT: &str = "SANITIZED_FIXTURE_VERSIONED_LEGACY";
const TOLERATED_TEXT: &str = "SANITIZED_FIXTURE_TOLERATED";

fn empty(name: &str) -> support::TestRoot {
    support::TestRoot::new(&format!("migration-{name}"))
}

fn prepare(name: &str, subdir: Option<&str>) -> support::TestRoot {
    let root = empty(name);
    let source = subdir.map_or_else(
        || support::fixture(name),
        |subdir| support::fixture(name).join(subdir),
    );
    support::copy_tree(&source, &root.backend()).expect("copy migration fixture");
    root
}

fn conversation_dir(root: &support::TestRoot) -> PathBuf {
    root.backend().join("conversations").join(DEFAULT_KEY)
}

fn messages(root: &support::TestRoot) -> PathBuf {
    conversation_dir(root).join("messages.jsonl")
}

fn manifest(root: &support::TestRoot) -> PathBuf {
    conversation_dir(root).join("manifest.json")
}

fn projected_messages(value: &Value) -> Vec<(String, String, String)> {
    value
        .as_array()
        .expect("message projection")
        .iter()
        .map(|message| {
            (
                message["id"].as_str().expect("message id").to_owned(),
                message["role"].as_str().expect("message role").to_owned(),
                message["text"].as_str().map_or_else(
                    || {
                        message["content"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|part| part["text"].as_str())
                            .collect()
                    },
                    str::to_owned,
                ),
            )
        })
        .collect()
}

fn rust_messages(
    loaded: &lotta_store::transcript::load::LoadedTranscript,
) -> Vec<(String, String, String)> {
    loaded
        .messages()
        .iter()
        .map(|message| {
            let value = serde_json::to_value(message).expect("message value");
            let text = value["content"]
                .as_array()
                .expect("content")
                .iter()
                .filter(|part| part["type"] == "text")
                .map(|part| part["text"].as_str().expect("text"))
                .collect::<String>();
            (
                value["id"].as_str().expect("id").to_owned(),
                value["role"].as_str().expect("role").to_owned(),
                text,
            )
        })
        .collect()
}

fn assert_current_manifest(value: &Value) {
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["message_format"], "pi-session-entry-jsonl");
    assert_eq!(value["provider_stack"], "pi-ai");
}

fn assert_rows_current(root: &support::TestRoot, expected_text: &str) {
    let rows = support::json_rows(&messages(root));
    assert_eq!(rows[0]["type"], "session");
    assert_eq!(rows[0]["version"], 3);
    assert!(rows[1..].iter().all(|row| row["type"] == "message"));
    assert!(
        rows[1..]
            .iter()
            .any(|row| row["message"]["content"][0]["text"] == expected_text),
        "expected {expected_text}; rows={rows:?}"
    );
}

fn one_backup(root: &support::TestRoot, prefix: &str) -> PathBuf {
    let mut backups = std::fs::read_dir(conversation_dir(root))
        .expect("scan conversation")
        .take(100)
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        backups.len(),
        1,
        "expected exactly one bounded backup for {prefix}"
    );
    backups.pop().expect("one backup")
}

fn synthetic_agent() -> Agent {
    serde_json::from_value(json!({
        "id": support::AGENT_ID, "name": "Rust Migration Agent", "description": null,
        "system": "Rust migration synthetic system", "tags": ["migration"],
        "model": "openai/gpt-4.1-mini", "model_settings": {}
    }))
    .expect("agent")
}

fn synthetic_conversation() -> Conversation {
    serde_json::from_value(json!({
        "id": support::CONVERSATION_ID, "agent_id": support::AGENT_ID,
        "archived": false, "archived_at": null, "created_at": "2026-08-14T00:00:00Z",
        "updated_at": "2026-08-14T00:00:00Z", "last_message_at": null,
        "summary": "Rust migration conversation", "in_context_message_ids": []
    }))
    .expect("conversation")
}

fn current_manifest() -> TranscriptManifest {
    TranscriptManifest {
        schema_version: 2,
        message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
        provider_stack: ProviderStack::PiAi,
        created_at: timestamp(),
        migrated_from: None,
        migrated_at: None,
        backup_path: None,
    }
}

fn session() -> TranscriptEntry {
    TranscriptEntry::Session(SessionEntry {
        entry_type: SessionEntryType::Session,
        version: 3,
        id: lotta_domain::NonEmptyString::new("rust-migration-session").expect("session id"),
        timestamp: timestamp(),
        cwd: "/rust/migration/workspace".to_owned(),
    })
}

fn rust_message() -> TranscriptEntry {
    let row = json!({
        "type":"message", "id":"rust-entry-current", "parentId":null,
        "timestamp":"2026-08-14T00:00:00Z",
        "message":{"id":"rust-message-current","role":"user",
        "content":[{"type":"text","text":"Rust current migration message"}],
        "timestamp":1786665600000.0,
        "metadata":{"created_at":"2026-08-14T00:00:00.000Z",
        "updated_at":"2026-08-14T00:00:00.000Z"}}
    });
    serde_json::from_value(row).expect("valid schema2 message fixture row")
}

fn timestamp() -> lotta_domain::Timestamp {
    lotta_domain::Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z").expect("timestamp")
}

#[tokio::test]
async fn case_1_typescript_current_loaded_by_rust() {
    let root = empty("case1-ts-current");
    let response = support::run_ts(&root, "migration_write_current", "current");
    let expected = projected_messages(&response["value"]["messages"]);
    assert_eq!(response["value"]["agent"]["name"], "TS Migration Agent");
    assert!(
        expected.iter().any(|(_, role, text)| {
            role == "user" && text == "TS current representative message"
        }),
        "projected={expected:?}; response={response}"
    );
    assert!(
        expected
            .iter()
            .any(|(_, _, text)| text.contains("TS current packed summary"))
    );
    assert_current_manifest(&response["value"]["manifest"]);
    let before = super::tree::inventory_tree(&root.backend()).expect("inventory before Rust load");
    let store = support::backend_store(&root);
    let agent = AgentStore::load(&store, &support::agent_id())
        .await
        .expect("Rust loads TS agent");
    assert_eq!(agent.name.as_str(), "TS Migration Agent");
    let conversation =
        ConversationStore::load(&store, &support::agent_id(), &support::conversation_id())
            .await
            .expect("Rust loads TS conversation");
    assert_eq!(conversation.id.as_str(), support::CONVERSATION_ID);
    let loaded = store
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .expect("Rust loads TS transcript");
    assert_eq!(rust_messages(&loaded), expected);
    assert_eq!(loaded.manifest().expect("manifest").schema_version, 2);
    assert_eq!(
        before,
        super::tree::inventory_tree(&root.backend()).expect("inventory after")
    );
}

#[tokio::test]
async fn case_2_rust_current_loaded_by_typescript() {
    let root = empty("case2-rust-current");
    let lease = support::Lease::acquire(root.path(), "rust-current-writer").expect("lease");
    let store = support::backend_store(&root);
    AgentStore::save(&store, &synthetic_agent())
        .await
        .expect("save agent");
    ConversationStore::save(&store, &synthetic_conversation())
        .await
        .expect("save conversation");
    store
        .initialize_transcript(
            &support::agent_id(),
            &support::conversation_id(),
            &current_manifest(),
            &session(),
        )
        .await
        .expect("initialize transcript");
    store
        .append_transcript_entry(
            &support::agent_id(),
            &support::conversation_id(),
            &rust_message(),
        )
        .await
        .expect("append domain message");
    drop(store);
    drop(lease);
    let response = support::run_ts(&root, "migration_read_current", "current");
    assert_eq!(response["value"]["agent"]["name"], "Rust Migration Agent");
    assert_eq!(
        response["value"]["conversation"]["summary"],
        "Rust migration conversation"
    );
    assert_eq!(
        projected_messages(&response["value"]["messages"]),
        vec![(
            "rust-message-current".to_owned(),
            "user".to_owned(),
            "Rust current migration message".to_owned(),
        )]
    );
    assert_current_manifest(&response["value"]["manifest"]);
}

#[tokio::test]
async fn case_3_legacy_migration_behaviors() {
    unversioned_initial_rejections().await;
    unversioned_typescript().await;
    unversioned_rust().await;
    versioned_typescript().await;
    versioned_rust().await;
}

async fn unversioned_initial_rejections() {
    let ts_root = prepare("unversioned_legacy_transcript", None);
    let ts_messages = std::fs::read(messages(&ts_root)).expect("TS legacy messages before");
    let ts_manifest = manifest(&ts_root);
    assert!(!ts_manifest.exists());
    let local = support::run_ts(&ts_root, "migration_load", "legacy");
    assert_eq!(
        local["value"]["name"],
        "LocalTranscriptMigrationRequiredError"
    );
    assert_eq!(
        ts_messages,
        std::fs::read(messages(&ts_root)).expect("TS legacy after")
    );
    assert!(!ts_manifest.exists());

    let rust_root = prepare("unversioned_legacy_transcript", None);
    let store = support::backend_store(&rust_root);
    let before = super::tree::inventory_tree(&rust_root.backend()).expect("Rust before");
    let error = store
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .expect_err("Rust must require migration");
    assert!(error.to_string().contains("transcript_migration_required"));
    drop(store);
    assert_eq!(
        before,
        super::tree::inventory_tree(&rust_root.backend()).expect("Rust after")
    );
}

async fn unversioned_typescript() {
    let root = prepare("unversioned_legacy_transcript", None);
    let original = std::fs::read(messages(&root)).expect("legacy source");
    let result = support::run_ts(&root, "migration_convert", "legacy");
    assert_eq!(
        result["value"]["converted"]
            .as_array()
            .expect("converted")
            .len(),
        1
    );
    assert_unversioned_conversion(&root, &original);
    let loaded = support::backend_store(&root)
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .expect("Rust loads TS conversion");
    assert_eq!(rust_messages(&loaded)[0].2, LEGACY_TEXT);
}

async fn unversioned_rust() {
    let root = prepare("unversioned_legacy_transcript", None);
    let original = std::fs::read(messages(&root)).expect("legacy source");
    let lease = support::Lease::acquire(root.path(), "rust-unversioned-migrator").expect("lease");
    let report = lotta_store::migration::migrate_transcripts(root.backend(), false)
        .expect("Rust explicit migration");
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].message_count, 1);
    drop(lease);
    assert_unversioned_conversion(&root, &original);
    let result = support::run_ts(&root, "migration_read_current", "current");
    let projected = projected_messages(&result["value"]["messages"]);
    assert_eq!(
        projected,
        vec![(
            "ui-msg-fixture".to_owned(),
            "user".to_owned(),
            LEGACY_TEXT.to_owned()
        )],
        "TS structured read={result}"
    );
}

fn assert_unversioned_conversion(root: &support::TestRoot, original: &[u8]) {
    let backup = std::fs::read_dir(conversation_dir(root))
        .expect("scan conversation")
        .take(100)
        .map(|entry| entry.expect("directory entry").path())
        .find(|path| std::fs::read(path).is_ok_and(|bytes| bytes == original))
        .expect("exact original backup in bounded scan");
    assert_eq!(std::fs::read(&backup).expect("backup bytes"), original);
    assert_rows_current(root, LEGACY_TEXT);
    let value = support::read_json(&manifest(root));
    assert_current_manifest(&value);
    assert_eq!(
        value["migrated_from"],
        "unversioned-legacy-local-message-jsonl"
    );
    let backup_path = value["backup_path"].as_str().expect("manifest backup path");
    let resolved = if Path::new(backup_path).is_absolute() {
        PathBuf::from(backup_path)
    } else {
        conversation_dir(root).join(backup_path)
    };
    assert!(resolved.is_file());
    let conversation = support::read_json(&conversation_dir(root).join("conversation.json"));
    assert_eq!(
        conversation["in_context_message_ids"],
        json!(["ui-msg-fixture"])
    );
}

async fn versioned_typescript() {
    let root = prepare("versioned_legacy_transcript", None);
    let initial = support::run_ts(&root, "migration_load", "current");
    assert_eq!(project_messages_from_loader(&initial)[0].2, VERSIONED_TEXT);
    let original = std::fs::read(messages(&root)).expect("versioned source");
    support::run_ts(&root, "migration_persist", "legacy");
    let backup = one_backup(&root, "messages.jsonl.pre-pi-backup-");
    assert_eq!(std::fs::read(backup).expect("backup"), original);
    assert_versioned_upgrade(&root, VERSIONED_TEXT);
    let loaded = support::backend_store(&root)
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .expect("Rust loads TS versioned upgrade");
    let messages = rust_messages(&loaded);
    assert!(messages.iter().any(|(_, _, text)| text == VERSIONED_TEXT));
}

async fn versioned_rust() {
    let root = prepare("versioned_legacy_transcript", None);
    let store = support::backend_store(&root);
    let loaded = store
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .expect("Rust loads versioned legacy");
    assert_eq!(rust_messages(&loaded)[0].2, VERSIONED_TEXT);
    let original = std::fs::read(messages(&root)).expect("versioned source");
    let lease = support::Lease::acquire(root.path(), "rust-versioned-upgrade").expect("lease");
    store
        .persist_loaded_projection(&support::agent_id(), &support::conversation_id(), loaded)
        .await
        .expect("public Rust persistence upgrade");
    drop(store);
    drop(lease);
    let backup = one_backup(&root, "messages.jsonl.lotta-upgrade-");
    assert_eq!(std::fs::read(backup).expect("backup"), original);
    assert_versioned_upgrade(&root, VERSIONED_TEXT);
    let result = support::run_ts(&root, "migration_load", "current");
    assert_eq!(project_messages_from_loader(&result)[0].2, VERSIONED_TEXT);
}

fn assert_versioned_upgrade(root: &support::TestRoot, expected_text: &str) {
    assert_rows_current(root, expected_text);
    let value = support::read_json(&manifest(root));
    assert_current_manifest(&value);
    assert_eq!(value["migrated_from"], "versioned-pi-ai-message-jsonl");
    let backup = value["backup_path"].as_str().expect("backup path");
    assert!(conversation_dir(root).join(backup).is_file());
}

fn inventory(root: &support::TestRoot) -> Vec<super::tree::InventoryEntry> {
    super::tree::inventory_tree(&root.backend()).expect("bounded full inventory")
}

fn entry<'a>(
    inventory: &'a [super::tree::InventoryEntry],
    relative: &str,
) -> &'a super::tree::InventoryEntry {
    inventory
        .iter()
        .find(|entry| entry.path == relative)
        .expect("inventory entry")
}

fn actual_dir(root: &support::TestRoot) -> PathBuf {
    root.backend().join("conversations").join(ACTUAL_KEY)
}

fn assert_json_repair(source: &Value, repaired: &Value, expected_ids: &[String]) {
    let mut expected = source.clone();
    expected["in_context_message_ids"] = serde_json::to_value(expected_ids).unwrap();
    assert_json_equal(&expected, repaired, "");
}

fn assert_json_equal(expected: &Value, actual: &Value, pointer: &str) {
    match (expected, actual) {
        (Value::Object(left), Value::Object(right)) => {
            assert_eq!(left.len(), right.len(), "field count at {pointer}");
            for (key, value) in left {
                let child = format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"));
                let actual = right
                    .get(key)
                    .unwrap_or_else(|| panic!("missing field {child}"));
                assert_json_equal(value, actual, &child);
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            assert_eq!(left.len(), right.len(), "array length at {pointer}");
            for (index, value) in left.iter().enumerate() {
                assert_json_equal(value, &right[index], &format!("{pointer}/{index}"));
            }
        }
        _ => assert_eq!(expected, actual, "value mismatch at {pointer}"),
    }
}

fn added_paths(
    before: &[super::tree::InventoryEntry],
    after: &[super::tree::InventoryEntry],
) -> Vec<String> {
    after
        .iter()
        .filter(|item| !before.iter().any(|old| old.path == item.path))
        .map(|item| item.path.clone())
        .collect()
}

fn expected_named_paths() -> Vec<String> {
    vec![
        format!("conversations/{ACTUAL_KEY}"),
        format!("conversations/{ACTUAL_KEY}/conversation.json"),
        format!("conversations/{ACTUAL_KEY}/manifest.json"),
        format!("conversations/{ACTUAL_KEY}/system-prompt.json"),
    ]
}

fn project_messages_from_loader(result: &Value) -> Vec<(String, String, String)> {
    let rows = result["value"].as_array().expect("loader messages");
    rows.iter()
        .map(|message| {
            let text = message["content"]
                .as_array()
                .expect("content")
                .iter()
                .filter(|part| part["type"] == "text")
                .map(|part| part["text"].as_str().expect("text"))
                .collect::<String>();
            (
                message["id"].as_str().expect("id").to_owned(),
                message["role"].as_str().expect("role").to_owned(),
                text,
            )
        })
        .collect()
}

#[tokio::test]
async fn case_4_tolerated_versioned_rows() {
    let ts_root = prepare("baseline_tolerated_versioned_rows", None);
    let ts_before = super::tree::inventory_tree(&ts_root.backend()).expect("TS before");
    let ts_result = support::run_ts(&ts_root, "migration_load", "tolerated");
    let ts_messages = project_messages_from_loader(&ts_result);
    assert_eq!(
        ts_messages,
        vec![(
            "msg-tolerated-fixture".to_owned(),
            "user".to_owned(),
            TOLERATED_TEXT.to_owned(),
        )]
    );
    assert_eq!(
        ts_before,
        super::tree::inventory_tree(&ts_root.backend()).expect("TS after")
    );

    let rust_root = prepare("baseline_tolerated_versioned_rows", None);
    let rust_before = super::tree::inventory_tree(&rust_root.backend()).expect("Rust before");
    let loaded = support::backend_store(&rust_root)
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .expect("Rust tolerated loader");
    let rust_projection = rust_messages(&loaded);
    assert_eq!(rust_projection, ts_messages);
    assert_eq!(
        rust_before,
        super::tree::inventory_tree(&rust_root.backend()).expect("Rust after")
    );
}

#[tokio::test]
async fn case_5_orphan_projection_repair() {
    let expected = support::read_json(
        &support::fixture("orphan_result_repair_input").join("expected-active-projection.json"),
    );
    let expected_ids = expected["message_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let expected_projection = vec![
        (
            "msg-assistant-fixture".into(),
            "assistant".into(),
            String::new(),
        ),
        (
            "msg-result-valid-fixture".into(),
            "toolResult".into(),
            "SANITIZED_FIXTURE_VALID_RESULT".into(),
        ),
    ];
    let ts_root = prepare("orphan_result_repair_input", None);
    std::fs::write(ts_root.backend().join(".lotta-storage.lock"), b"").unwrap();
    let ts_before = inventory(&ts_root);
    let source = support::read_json(&conversation_dir(&ts_root).join("conversation.json"));
    let result = support::run_ts(&ts_root, "migration_load", "orphan");
    assert_eq!(
        projected_messages(&result["value"]["messages"]),
        expected_projection
    );
    assert_eq!(result["value"]["actual"]["key"], ACTUAL_KEY);
    let ts_after = inventory(&ts_root);
    assert_eq!(
        entry(
            &ts_before,
            &format!("conversations/{DEFAULT_KEY}/conversation.json")
        ),
        entry(
            &ts_after,
            &format!("conversations/{DEFAULT_KEY}/conversation.json")
        )
    );
    assert_eq!(
        entry(
            &ts_before,
            &format!("conversations/{DEFAULT_KEY}/messages.jsonl")
        ),
        entry(
            &ts_after,
            &format!("conversations/{DEFAULT_KEY}/messages.jsonl")
        )
    );
    assert_eq!(added_paths(&ts_before, &ts_after), expected_named_paths());
    let named = support::read_json(&actual_dir(&ts_root).join("conversation.json"));
    assert_json_repair(&source, &named, &expected_ids);
    assert!(!actual_dir(&ts_root).join("messages.jsonl").exists());
    assert_current_manifest(&support::read_json(
        &actual_dir(&ts_root).join("manifest.json"),
    ));
    assert!(support::read_json(&actual_dir(&ts_root).join("system-prompt.json")).is_object());

    let rust_root = prepare("orphan_result_repair_input", None);
    std::fs::write(rust_root.backend().join(".lotta-storage.lock"), b"").unwrap();
    let rust_before = inventory(&rust_root);
    let lease = support::Lease::acquire(rust_root.path(), "rust-orphan-repair").unwrap();
    let loaded = support::backend_store(&rust_root)
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .unwrap();
    drop(lease);
    assert_eq!(rust_messages(&loaded), expected_projection);
    let rust_after = inventory(&rust_root);
    assert_eq!(
        entry(
            &rust_before,
            &format!("conversations/{DEFAULT_KEY}/messages.jsonl")
        ),
        entry(
            &rust_after,
            &format!("conversations/{DEFAULT_KEY}/messages.jsonl")
        )
    );
    let rust_source = support::read_json(&conversation_dir(&rust_root).join("conversation.json"));
    assert_eq!(rust_source["in_context_message_ids"], json!(expected_ids));
    drop(loaded);

    assert_opposite_reloads(
        &ts_root,
        &rust_root,
        &rust_source,
        &expected_ids,
        &expected_projection,
    )
    .await;
}

async fn assert_opposite_reloads(
    ts_root: &support::TestRoot,
    rust_root: &support::TestRoot,
    source: &Value,
    expected_ids: &[String],
    expected: &[(String, String, String)],
) {
    let before = inventory(ts_root);
    let named_id = lotta_domain::ConversationId::accept("conversation-fixture").unwrap();
    let conversation = ConversationStore::load(
        &support::backend_store(ts_root),
        &support::agent_id(),
        &named_id,
    )
    .await
    .unwrap();
    assert_eq!(conversation.id.as_str(), "conversation-fixture");
    assert_eq!(before, inventory(ts_root));

    let before = inventory(rust_root);
    let default_conversation =
        std::fs::read(conversation_dir(rust_root).join("conversation.json")).unwrap();
    let default_transcript = std::fs::read(messages(rust_root)).unwrap();
    let result = support::run_ts(rust_root, "migration_load", "orphan");
    assert_eq!(projected_messages(&result["value"]["messages"]), expected);
    assert_eq!(
        std::fs::read(conversation_dir(rust_root).join("conversation.json")).unwrap(),
        default_conversation
    );
    assert_eq!(
        std::fs::read(messages(rust_root)).unwrap(),
        default_transcript
    );
    let after = inventory(rust_root);
    assert_eq!(before, after);
    assert_eq!(result["value"]["actual"]["exists"], false);
    assert!(!actual_dir(rust_root).exists());
    assert_json_repair(
        source,
        &support::read_json(&conversation_dir(rust_root).join("conversation.json")),
        expected_ids,
    );
}

#[tokio::test]
async fn case_6_interrupted_append_and_replacement() {
    assert_case6_index();
    interrupted_append().await;
    interrupted_replacement().await;
}

fn assert_case6_index() {
    let index = support::read_json(&support::fixture("index.json"));
    let cases = index["cases"].as_array().unwrap();
    let extracted = cases
        .iter()
        .filter(|case| case["obligation"] == 6)
        .collect::<Vec<_>>();
    assert_eq!(extracted.len(), 2);
    assert_eq!(extracted[0]["case_id"], 7);
    assert_eq!(extracted[1]["case_id"], 8);
    assert!(
        extracted.iter().all(|case| {
            case["expected_typescript_behavior"] == "not_applicable_rust_hardening"
        })
    );
}

async fn interrupted_append() {
    let root = prepare("interrupted_append", Some("input"));
    let before = inventory(&root);
    let original = std::fs::read(messages(&root)).unwrap();
    let prefix = std::fs::read(
        support::fixture("interrupted_append").join("expected/complete-prefix.jsonl"),
    )
    .unwrap();
    assert!(original.starts_with(&prefix));
    let tail = b"{\"type\":\"message\",\"id\":\"entry-truncated-fixture\",\
        \"marker\":\"SANITIZED_FIXTURE_TRUNCATED\"";
    assert_eq!(&original[prefix.len()..], tail);
    let lease = support::Lease::acquire(root.path(), "rust-append-recovery").unwrap();
    let store = support::backend_store(&root);
    let loaded = store
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .expect("recover complete prefix");
    assert_eq!(
        rust_messages(&loaded),
        vec![(
            "msg-user-fixture".into(),
            "user".into(),
            "SANITIZED_FIXTURE_APPEND_PREFIX".into()
        )]
    );
    assert_eq!(before, inventory(&root));
    assert_eq!(std::fs::read(messages(&root)).unwrap(), original);
    let entries = prefix
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
        .map(|row| serde_json::from_slice::<TranscriptEntry>(row).expect("prefix entry"))
        .collect::<Vec<_>>();
    store
        .persist_loaded_transcript(&support::agent_id(), &support::conversation_id(), entries)
        .await
        .expect("atomic clean cross-runtime handoff");
    drop(store);
    drop(lease);
    let cleaned = std::fs::read(messages(&root)).unwrap();
    let cleaned_rows = cleaned
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
        .map(|row| serde_json::from_slice::<TranscriptEntry>(row).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(cleaned_rows.len(), 2);
    let result = support::run_ts(&root, "migration_load", "current");
    assert_eq!(
        project_messages_from_loader(&result),
        vec![(
            "msg-user-fixture".into(),
            "user".into(),
            "SANITIZED_FIXTURE_APPEND_PREFIX".into()
        )]
    );
}

async fn interrupted_replacement() {
    let root = prepare("interrupted_replacement", Some("input"));
    let candidate = support::fixture("interrupted_replacement").join("candidate");
    let candidate_before = super::tree::inventory_tree(&candidate).unwrap();
    let before = inventory(&root);
    let expected = std::fs::read(
        support::fixture("interrupted_replacement").join("expected/active-messages.jsonl"),
    )
    .unwrap();
    let lease = support::Lease::acquire(root.path(), "rust-replacement-load").unwrap();
    let loaded = support::backend_store(&root)
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .expect("load active replacement state");
    drop(lease);
    let projection = vec![(
        "msg-active-fixture".into(),
        "user".into(),
        "SANITIZED_FIXTURE_ACTIVE_ORIGINAL".into(),
    )];
    assert_eq!(rust_messages(&loaded), projection);
    assert_eq!(before, inventory(&root));
    assert_eq!(std::fs::read(messages(&root)).unwrap(), expected);
    let active_inventory = inventory(&root);
    assert!(active_inventory.iter().all(|entry| {
        !entry.path.contains("candidate") && !entry.path.contains("msg-candidate-fixture")
    }));
    for entry in &active_inventory {
        if entry.kind == "file" {
            let bytes = std::fs::read(root.backend().join(&entry.path)).unwrap();
            assert!(
                !bytes
                    .windows(b"msg-candidate-fixture".len())
                    .any(|window| { window == b"msg-candidate-fixture" })
            );
            assert!(
                !bytes
                    .windows(b"SANITIZED_FIXTURE_COMPLETE_CANDIDATE".len())
                    .any(|window| window == b"SANITIZED_FIXTURE_COMPLETE_CANDIDATE")
            );
        }
    }
    assert_eq!(rust_messages(&loaded), projection);
    let ts = support::run_ts(&root, "migration_load", "current");
    assert_eq!(project_messages_from_loader(&ts), projection);
    assert_eq!(before, inventory(&root));
    assert_eq!(
        candidate_before,
        super::tree::inventory_tree(&candidate).unwrap()
    );
}

#[tokio::test]
async fn case_7_corrupt_unsupported_rejected_without_mutation() {
    let mut completed = Vec::new();
    for case in ["corrupt_json", "unsupported_schema", "unsupported_provider"] {
        validate_invalid_typescript(case);
        validate_invalid_rust(case).await;
        completed.push(case);
    }
    assert_eq!(
        completed,
        ["corrupt_json", "unsupported_schema", "unsupported_provider"]
    );
}

fn prepare_invalid(case: &str) -> support::TestRoot {
    let root = prepare(
        "corrupt_unsupported_manifests",
        Some(&format!("{case}/input")),
    );
    let conversation = conversation_dir(&root).join("conversation.json");
    let value = synthetic_conversation();
    std::fs::write(
        conversation,
        serde_json::to_vec_pretty(&value).expect("conversation scaffolding"),
    )
    .expect("write temp-only strict-loader scaffolding");
    root
}

fn validate_invalid_typescript(case: &str) {
    let root = prepare_invalid(case);
    let before = inventory(&root);
    let result = support::run_ts(&root, "migration_validate", "invalid");
    assert_eq!(result["value"]["rejected"], true);
    assert!(
        result["value"]["name"]
            .as_str()
            .is_some_and(|name| !name.is_empty())
    );
    assert_eq!(before, inventory(&root));
}

async fn validate_invalid_rust(case: &str) {
    let root = prepare_invalid(case);
    let before = inventory(&root);
    let lease = support::Lease::acquire(root.path(), "rust-invalid-load").unwrap();
    let error = support::backend_store(&root)
        .load_transcript(&support::agent_id(), &support::conversation_id())
        .await
        .expect_err("Rust rejects invalid fixture");
    assert_eq!(error.kind(), StoreErrorKind::Parse, "{case}: {error}");
    assert!(error.to_string().contains("manifest"), "{case}: {error}");
    drop(lease);
    assert_eq!(before, inventory(&root));
}
