use super::support::*;
use super::*;

#[test]
fn unversioned_legacy() {
    simple_unversioned_fixture();
    complex_unversioned_fixture();
}

fn simple_unversioned_fixture() {
    let root = copy_fixture("persistence/unversioned_legacy_transcript");
    let directory = conversation(root.path());
    let source = bytes(&directory.join("messages.jsonl"));
    let report = migrate_transcripts(root.path(), false).expect("migration");
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].disposition, MigrationDisposition::Converted);
    assert_eq!(report.items[0].message_count, 1);
    let backup = report.items[0].backup_path.as_ref().expect("backup");
    assert_eq!(bytes(backup), source);
    let converted = rows(&directory.join("messages.jsonl"));
    assert_eq!(converted.len(), 2);
    assert_eq!(converted[0]["type"], "session");
    assert_eq!(converted[0]["version"], 3);
    assert_eq!(converted[0]["id"], "conversation-fixture");
    assert_eq!(converted[1]["id"], "ui-msg-fixture");
    assert_eq!(converted[1]["message"]["id"], "ui-msg-fixture");
    assert_eq!(
        converted[1]["message"]["content"][0]["text"],
        "SANITIZED_FIXTURE_LEGACY_UI"
    );
    let projection: Value =
        serde_json::from_slice(&bytes(&directory.join("conversation.json"))).expect("conversation");
    assert_eq!(
        projection["in_context_message_ids"],
        serde_json::json!(["ui-msg-fixture"])
    );
    assert_manifest(
        &directory,
        "unversioned-legacy-local-message-jsonl",
        "2000-01-01T00:00:00Z",
    );
    assert_no_temp(&directory);
}

fn complex_unversioned_fixture() {
    let root = copy_fixture("persistence/unversioned_legacy_transcript");
    let directory = conversation(root.path());
    let messages = directory.join("messages.jsonl");
    let conversation_path = directory.join("conversation.json");
    let source_rows = [
        serde_json::json!({"id":"ui-msg-7","role":"system","parts":[{"type":"text","text":"look"},{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"JPEG"}},{"type":"file","mediaType":"image/png","url":"data:image/png;base64,PNG"},{"type":"file","mediaType":"image/png","url":"https://invalid.example/image.png"},{"type":"unknown","value":1},42],"metadata":{"created_at":"2001-01-01T00:00:00.000Z"}}),
        serde_json::json!({"id":"ui-msg-8","role":"user","parts":[{"type":"unknown"},false,9],"metadata":{"created_at":"2001-01-02T00:00:00.000Z"}}),
        serde_json::json!({"id":"ui-msg-12","role":"user","parts":[{"type":"text","text":"first summary"},{"type":"text","text":"ignored summary"}],"metadata":{"compaction":{"reason":"tokens"},"extra":"kept","created_at":"2001-01-03T00:00:00.000Z","updated_at":"2001-01-03T01:00:00.000Z"}}),
        serde_json::json!({"id":"ui-msg-9","role":"assistant","parts":[{"type":"step-start"},{"type":"reasoning","text":"think one"},{"type":"tool-read","toolCallId":"call-read","input":{"path":"a.txt"},"state":"output-available","output":{"ok":true,"lines":[1,2]}},{"type":"step-start"},{"type":"text","text":"done"},{"type":"tool-write","toolCallId":"call-write","input":"raw","state":"output-error","errorText":404},{"type":"tool-deny","toolCallId":"call-deny","input":false,"state":"output-denied","errorText":"denied"}],"metadata":{"created_at":"2001-01-04T00:00:00.000Z","updated_at":"2001-01-04T01:00:00.000Z","trace":"kept"}}),
        serde_json::json!({"role":"user","parts":[{"type":"text","text":"generated"}],"metadata":{"created_at":"2001-01-05T00:00:00.000Z"}}),
        serde_json::json!({"id":"ui-msg-20","role":"developer","parts":[{"type":"text","text":"skip role"}]}),
        serde_json::json!({"id":"pi-row","role":"user","content":[{"type":"text","text":"skip nonlegacy"}],"timestamp":1}),
    ];
    let mut source = source_rows
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    source.push(b'\n');
    std::fs::write(&messages, &source).expect("complex transcript");
    let mut projection: Value =
        serde_json::from_slice(&bytes(&conversation_path)).expect("conversation");
    projection["in_context_message_ids"] =
        serde_json::json!(["ui-msg-7", "ui-msg-9", "ui-msg-12", "ui-msg-8"]);
    std::fs::write(
        &conversation_path,
        serde_json::to_vec_pretty(&projection).expect("conversation json"),
    )
    .expect("context ids");

    let report = migrate_transcripts(root.path(), false).expect("complex migration");
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].disposition, MigrationDisposition::Converted);
    assert_eq!(report.items[0].message_count, 9);
    let backup = report.items[0].backup_path.as_ref().expect("backup");
    assert_eq!(bytes(backup), source);
    assert_complex_output(root.path(), &directory, &messages, &conversation_path);
}

fn assert_complex_output(root: &Path, directory: &Path, messages: &Path, conversation_path: &Path) {
    let output = rows(messages);
    assert_eq!(output.len(), 10);
    assert_eq!(
        output[0],
        serde_json::json!({"type":"session","version":3,"id":"conversation-fixture","timestamp":"2000-01-01T00:00:00Z","cwd":""})
    );
    let actual = output[1..]
        .iter()
        .map(|row| row["message"].clone())
        .collect::<Vec<_>>();
    let metadata = |created: &str, updated: &str, extra: Value| {
        let mut value = serde_json::json!({"created_at":created,"updated_at":updated});
        if let (Some(target), Some(source)) = (value.as_object_mut(), extra.as_object()) {
            target.extend(source.clone());
        }
        value
    };
    assert_eq!(
        actual[0],
        serde_json::json!({"id":"ui-msg-7","role":"user","content":[{"type":"text","text":"look"},{"type":"image","mimeType":"image/jpeg","data":"JPEG"},{"type":"image","mimeType":"image/png","data":"PNG"}],"timestamp":978_307_200_000.0,"metadata":metadata("2001-01-01T00:00:00.000Z","2001-01-01T00:00:00.000Z",serde_json::json!({}))})
    );
    assert_eq!(
        actual[1],
        serde_json::json!({"id":"ui-msg-8","role":"user","content":[{"type":"text","text":""}],"timestamp":978_393_600_000.0,"metadata":metadata("2001-01-02T00:00:00.000Z","2001-01-02T00:00:00.000Z",serde_json::json!({}))})
    );
    assert_eq!(
        actual[2],
        serde_json::json!({"id":"ui-msg-12","role":"user","content":[{"type":"text","text":"first summary"}],"timestamp":978_480_000_000.0,"metadata":metadata("2001-01-03T00:00:00.000Z","2001-01-03T01:00:00.000Z",serde_json::json!({"compaction":{"reason":"tokens"},"extra":"kept"}))})
    );
    let assistant_meta = metadata(
        "2001-01-04T00:00:00.000Z",
        "2001-01-04T01:00:00.000Z",
        serde_json::json!({"trace":"kept"}),
    );
    let usage = serde_json::json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}});
    assert_eq!(
        actual[3],
        serde_json::json!({"id":"ui-msg-21","role":"assistant","content":[{"type":"thinking","thinking":"think one"},{"type":"toolCall","id":"call-read","name":"read","arguments":{"path":"a.txt"}}],"api":"legacy-local","provider":"legacy-local","model":"legacy-local","usage":usage,"stopReason":"stop","timestamp":978_566_400_000.0,"metadata":assistant_meta})
    );
    assert_eq!(
        actual[4],
        serde_json::json!({"id":"ui-msg-22","role":"toolResult","toolCallId":"call-read","toolName":"read","content":[{"type":"text","text":"{\"lines\":[1,2],\"ok\":true}"}],"isError":false,"timestamp":978_566_400_000.0,"metadata":assistant_meta})
    );
    assert_eq!(
        actual[5],
        serde_json::json!({"id":"ui-msg-23","role":"assistant","content":[{"type":"text","text":"done"},{"type":"toolCall","id":"call-write","name":"write","arguments":{"input":"raw"}},{"type":"toolCall","id":"call-deny","name":"deny","arguments":{"input":false}}],"api":"legacy-local","provider":"legacy-local","model":"legacy-local","usage":usage,"stopReason":"stop","timestamp":978_566_400_000.0,"metadata":assistant_meta})
    );
    assert_eq!(
        actual[6],
        serde_json::json!({"id":"ui-msg-24","role":"toolResult","toolCallId":"call-write","toolName":"write","content":[{"type":"text","text":"404"}],"isError":true,"timestamp":978_566_400_000.0,"metadata":assistant_meta})
    );
    assert_eq!(
        actual[7],
        serde_json::json!({"id":"ui-msg-25","role":"toolResult","toolCallId":"call-deny","toolName":"deny","content":[{"type":"text","text":"denied"}],"isError":true,"timestamp":978_566_400_000.0,"metadata":assistant_meta})
    );
    assert_eq!(
        actual[8],
        serde_json::json!({"id":"ui-msg-26","role":"user","content":[{"type":"text","text":"generated"}],"timestamp":978_652_800_000.0,"metadata":metadata("2001-01-05T00:00:00.000Z","2001-01-05T00:00:00.000Z",serde_json::json!({}))})
    );
    assert!(!actual.iter().any(|message| message["id"] == "ui-msg-9"
        || message["id"] == "ui-msg-20"
        || message["id"] == "pi-row"));
    assert_complex_projection(root, directory, conversation_path, &actual);
}

fn assert_complex_projection(
    root: &Path,
    directory: &Path,
    conversation_path: &Path,
    actual: &[Value],
) {
    let output = rows(&directory.join("messages.jsonl"));
    let ids = output[1..]
        .iter()
        .map(|row| row["id"].as_str().expect("id"))
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "ui-msg-7",
            "ui-msg-8",
            "ui-msg-12",
            "ui-msg-21",
            "ui-msg-22",
            "ui-msg-23",
            "ui-msg-24",
            "ui-msg-25",
            "ui-msg-26"
        ]
    );
    let projection: Value =
        serde_json::from_slice(&bytes(conversation_path)).expect("conversation");
    assert_eq!(
        projection["in_context_message_ids"],
        serde_json::json!([
            "ui-msg-7",
            "ui-msg-21",
            "ui-msg-22",
            "ui-msg-23",
            "ui-msg-24",
            "ui-msg-25",
            "ui-msg-12",
            "ui-msg-8"
        ])
    );
    assert_manifest(
        directory,
        "unversioned-legacy-local-message-jsonl",
        "2000-01-01T00:00:00Z",
    );
    let context_expected = [
        actual[0].clone(),
        actual[3].clone(),
        actual[4].clone(),
        actual[5].clone(),
        actual[6].clone(),
        actual[7].clone(),
        actual[2].clone(),
        actual[1].clone(),
    ];
    assert_public_messages(root, &context_expected);
    assert_no_temp(directory);
}

#[test]
fn versioned_legacy() {
    simple_versioned_fixture();
    mixed_versioned_repair_fixture();
}

fn simple_versioned_fixture() {
    let root = copy_fixture("persistence/versioned_legacy_transcript");
    let directory = conversation(root.path());
    let report = migrate_transcripts(root.path(), false).expect("migration");
    assert_eq!(report.items[0].message_count, 1);
    let converted = rows(&directory.join("messages.jsonl"));
    assert_eq!(converted[1]["id"], "msg-legacy-fixture");
    assert_eq!(converted[1]["message"]["id"], "msg-legacy-fixture");
    assert_eq!(
        converted[1]["message"]["content"][0]["text"],
        "SANITIZED_FIXTURE_VERSIONED_LEGACY"
    );
    assert_public_messages(root.path(), &[converted[1]["message"].clone()]);
    assert_no_temp(&directory);
}

fn mixed_versioned_repair_fixture() {
    let root = copy_fixture("persistence/versioned_legacy_transcript");
    let directory = conversation(root.path());
    let messages = directory.join("messages.jsonl");
    let manifest_path = directory.join("manifest.json");
    let conversation_path = directory.join("conversation.json");
    let source_created = "1999-12-31T23:59:59.000Z";
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&serde_json::json!({"schema_version":2,"message_format":"pi-session-entry-jsonl","provider_stack":"pi-ai","created_at":source_created})).expect("manifest")).expect("schema2 manifest");
    let preserved = serde_json::json!({"id":"pi-msg-30","role":"user","content":[{"type":"text","text":"preserved exactly"}],"timestamp":946_684_799_000.0,"metadata":{"created_at":"1999-12-31T23:59:59.000Z","custom":{"x":1}},"future":"field"});
    let source_rows = [
        preserved.clone(),
        serde_json::json!({"id":"ui-msg-31","role":"assistant","parts":[{"type":"text","text":"converted assistant"}],"metadata":{"created_at":"2000-01-02T00:00:00.000Z","tag":"a"}}),
        serde_json::json!({"id":"ui-msg-32","role":"user","parts":[{"type":"text","text":"converted user"}],"metadata":{"created_at":"2000-01-03T00:00:00.000Z"}}),
        serde_json::json!({"id":"invalid","role":"developer","unexpected":true}),
    ];
    let mut source = source_rows
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    source.push(b'\n');
    std::fs::write(&messages, &source).expect("mixed rows");
    let mut projection: Value =
        serde_json::from_slice(&bytes(&conversation_path)).expect("conversation");
    projection["in_context_message_ids"] =
        serde_json::json!(["pi-msg-30", "ui-msg-31", "ui-msg-32"]);
    std::fs::write(
        &conversation_path,
        serde_json::to_vec_pretty(&projection).expect("conversation"),
    )
    .expect("context");
    let report = migrate_transcripts(root.path(), false).expect("repair");
    assert_eq!(report.items[0].disposition, MigrationDisposition::Converted);
    assert_eq!(report.items[0].message_count, 3);
    assert_eq!(
        bytes(report.items[0].backup_path.as_ref().expect("backup")),
        source
    );
    let converted = rows(&messages);
    assert_eq!(converted.len(), 4);
    assert_eq!(
        converted[0],
        serde_json::json!({"type":"session","version":3,"id":"conversation-fixture","timestamp":"2000-01-01T00:00:00Z","cwd":""})
    );
    let actual = converted[1..]
        .iter()
        .map(|row| row["message"].clone())
        .collect::<Vec<_>>();
    assert_eq!(actual[0], preserved);
    assert_eq!(
        actual[1],
        serde_json::json!({"id":"ui-msg-31","role":"assistant","content":[{"type":"text","text":"converted assistant"}],"api":"legacy-local","provider":"legacy-local","model":"legacy-local","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":946_771_200_000.0,"metadata":{"created_at":"2000-01-02T00:00:00.000Z","updated_at":"2000-01-02T00:00:00.000Z","tag":"a"}})
    );
    assert_eq!(
        actual[2],
        serde_json::json!({"id":"ui-msg-32","role":"user","content":[{"type":"text","text":"converted user"}],"timestamp":946_857_600_000.0,"metadata":{"created_at":"2000-01-03T00:00:00.000Z","updated_at":"2000-01-03T00:00:00.000Z"}})
    );
    assert_eq!(converted[1]["parentId"], Value::Null);
    assert_eq!(converted[2]["parentId"], "pi-msg-30");
    assert_eq!(converted[3]["parentId"], "ui-msg-31");
    let projection: Value =
        serde_json::from_slice(&bytes(&conversation_path)).expect("conversation");
    assert_eq!(
        projection["in_context_message_ids"],
        serde_json::json!(["pi-msg-30", "ui-msg-31", "ui-msg-32"])
    );
    assert_manifest(
        &directory,
        "versioned-pi-transcript-with-legacy-ui-message-rows",
        source_created,
    );
    assert_public_messages(root.path(), &actual);
    let before = snapshot(root.path());
    let rerun = migrate_transcripts(root.path(), false).expect("rerun");
    assert_eq!(
        rerun.items[0].disposition,
        MigrationDisposition::AlreadyCurrent
    );
    assert_eq!(snapshot(root.path()), before);
    assert_no_temp(&directory);
}

fn assert_manifest(directory: &Path, migrated_from: &str, created_at: &str) {
    let manifest = transcript::manifest::read_current(&directory.join("manifest.json"))
        .expect("current manifest");
    assert_eq!(manifest.schema_version, 2);
    assert_eq!(
        manifest.message_format,
        lotta_domain::TranscriptMessageFormat::PiSessionEntryJsonl
    );
    assert_eq!(manifest.provider_stack, lotta_domain::ProviderStack::PiAi);
    assert_eq!(
        manifest.created_at,
        lotta_domain::Timestamp::parse_persisted_rfc3339(created_at).expect("expected created_at")
    );
    assert_eq!(manifest.migrated_from.as_deref(), Some(migrated_from));
    lotta_domain::Timestamp::parse_persisted_rfc3339(
        &manifest.migrated_at.expect("migrated at").to_string(),
    )
    .expect("valid migrated_at");
    assert!(manifest.backup_path.as_deref().is_some_and(|path| {
        path.starts_with("messages.jsonl.lotta-upgrade-")
            && Path::new(path)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("bak"))
    }));
}

fn assert_public_messages(root: &Path, expected: &[Value]) {
    let store = crate::LocalStore::new(StorePaths::new(root).expect("paths"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let loaded = runtime
        .block_on(store.load_transcript(
            &lotta_domain::AgentId::accept("agent-local-fixture").expect("agent"),
            &lotta_domain::ConversationId::default_for_agent(),
        ))
        .expect("Task25 reload");
    let actual = loaded
        .messages()
        .iter()
        .map(|message| serde_json::to_value(message).expect("message json"))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

pub(super) fn assert_no_temp(directory: &Path) {
    for entry in std::fs::read_dir(directory).expect("directory") {
        let name = entry.expect("entry").file_name();
        assert!(!name.to_string_lossy().contains(".tmp"));
    }
}
