use super::*;

fn case(position: usize) -> PersistenceCaseRecord {
    let value = index();
    let record = value.cases[position].clone();
    assert_eq!(record, expected_case(position));
    record
}

fn default_root(name: &str) -> String {
    conversation_root(name, DEFAULT_KEY)
}

fn assert_current_rows(name: &str, entry_id: &str, text: &str) {
    let root = default_root(name);
    assert_manifest(
        &format!("{root}/manifest.json"),
        2,
        "pi-session-entry-jsonl",
    );
    let values = rows(&format!("{root}/messages.jsonl"));
    assert_eq!(values.len(), 2);
    assert_eq!(values[0]["type"], "session");
    assert_eq!(values[0]["version"], 3);
    assert_eq!(values[0]["id"], CONVERSATION_ID);
    assert_eq!(values[1]["type"], "message");
    assert_eq!(values[1]["id"], entry_id);
    assert_eq!(values[1]["parentId"], Value::Null);
    assert_eq!(values[1]["message"]["role"], "user");
    assert_eq!(values[1]["message"]["content"][0]["text"], text);
    assert_eq!(
        load_json(&format!("{root}/conversation.json"))["agent_id"],
        AGENT_ID
    );
}

#[test]
fn current_typescript_state() {
    let record = case(0);
    assert_eq!(record.expected_rust_behavior, RustBehavior::ReadsCurrent);
    assert_current_rows(
        &record.relative_root,
        "entry-user-fixture",
        "SANITIZED_FIXTURE_CURRENT",
    );
    let named = conversation_root(&record.relative_root, NAMED_KEY);
    assert_manifest(
        &format!("{named}/manifest.json"),
        2,
        "pi-session-entry-jsonl",
    );
    let values = rows(&format!("{named}/messages.jsonl"));
    assert_eq!(values[1]["id"], "entry-named-fixture");
    assert_eq!(values[1]["message"]["id"], "msg-user-fixture");
    assert_eq!(
        load_json(&format!("{named}/conversation.json"))["agent_id"],
        AGENT_ID
    );
}

#[test]
fn rust_target_state() {
    let record = case(1);
    assert_eq!(
        record.expected_typescript_behavior,
        TypescriptBehavior::ReadsExactWire
    );
    assert_current_rows(
        &record.relative_root,
        "entry-rust-fixture",
        "SANITIZED_FIXTURE_RUST_TARGET",
    );
    let conversation = load_json(&format!(
        "{}/conversation.json",
        default_root(&record.relative_root)
    ));
    assert_eq!(
        conversation["in_context_message_ids"],
        serde_json::json!(["msg-user-fixture"])
    );
}

#[test]
fn unversioned_legacy_transcript() {
    let record = case(2);
    let root = default_root(&record.relative_root);
    let tree = FixtureLoader::new()
        .list_children(format!("persistence/{root}"))
        .expect("legacy tree");
    assert!(!tree.iter().any(|path| path == "manifest.json"));
    assert!(tree.contains(&"messages.jsonl".into()));
    let values = rows(&format!("{root}/messages.jsonl"));
    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["id"], "ui-msg-fixture");
    assert_eq!(values[0]["role"], "user");
    assert!(
        !values[0]["parts"]
            .as_array()
            .expect("legacy parts")
            .is_empty()
    );
    assert_eq!(
        record.expected_rust_behavior,
        RustBehavior::RequiresExplicitMigration
    );
    assert_eq!(
        record.expected_recovery_or_mutation,
        MutationDisposition::BackupThenConversion
    );
}

#[test]
fn versioned_legacy_transcript() {
    let record = case(3);
    let root = default_root(&record.relative_root);
    assert_manifest(&format!("{root}/manifest.json"), 1, "pi-ai-message-jsonl");
    let values = rows(&format!("{root}/messages.jsonl"));
    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["id"], "msg-legacy-fixture");
    assert_eq!(values[0]["role"], "user");
    assert!(values[0]["content"].is_array());
    let conversation = load_json(&format!("{root}/conversation.json"));
    assert_eq!(
        conversation["in_context_message_ids"],
        serde_json::json!(["msg-legacy-fixture"])
    );
}

#[test]
fn baseline_tolerated_versioned_rows() {
    let record = case(4);
    let root = default_root(&record.relative_root);
    let values = rows(&format!("{root}/messages.jsonl"));
    assert_eq!(values.len(), 2);
    assert_eq!(values[0]["type"], "session");
    assert_eq!(values[0]["version"], 3);
    assert_eq!(values[1]["type"], "message");
    assert_eq!(values[1]["id"], "entry-tolerated-fixture");
    assert_eq!(values[1]["parentId"], Value::Null);
    assert_eq!(values[1]["message"]["id"], "msg-tolerated-fixture");
    assert_eq!(
        record.expected_rust_behavior,
        RustBehavior::LoadsMessageEntries
    );
}

#[test]
fn orphan_result_repair_input() {
    let record = case(5);
    let root = default_root(&record.relative_root);
    let source = load_text(&format!("{root}/messages.jsonl"));
    let values = rows(&format!("{root}/messages.jsonl"));
    assert_eq!(values.len(), 4);
    assert_eq!(values[1]["message"]["content"][0]["type"], "toolCall");
    assert_eq!(
        values[1]["message"]["content"][0]["id"],
        "call-valid-fixture"
    );
    assert_eq!(values[2]["message"]["toolCallId"], "call-valid-fixture");
    assert_eq!(values[3]["message"]["toolCallId"], "call-orphan-fixture");
    let projection = load_json(&format!(
        "{}/expected-active-projection.json",
        record.relative_root
    ));
    assert_eq!(
        projection["message_ids"],
        serde_json::json!(["msg-assistant-fixture", "msg-result-valid-fixture"])
    );
    assert_eq!(
        projection["removed_message_ids"],
        serde_json::json!(["msg-result-orphan-fixture"])
    );
    assert_eq!(projection["transcript_mutated"], false);
    assert_eq!(source, load_text(&format!("{root}/messages.jsonl")));
}

#[test]
fn interrupted_append() {
    let record = case(6);
    let root = format!("{}/input/conversations/{DEFAULT_KEY}", record.relative_root);
    let active = load_text(&format!("{root}/messages.jsonl"));
    let prefix = load_text(&format!(
        "{}/expected/complete-prefix.jsonl",
        record.relative_root
    ));
    assert!(active.starts_with(&prefix));
    assert!(active[prefix.len()..].starts_with("{\"type\":\"message\""));
    assert!(serde_json::from_str::<Value>(&active[prefix.len()..]).is_err());
    assert_eq!(prefix.lines().count(), 2);
    for line in prefix.lines() {
        serde_json::from_str::<Value>(line).expect("complete prefix row");
    }
    assert_eq!(
        record.expected_typescript_behavior,
        TypescriptBehavior::NotApplicableRustHardening
    );
    assert_eq!(
        record.expected_rust_behavior,
        RustBehavior::RecoversCompletePrefix
    );
}

#[test]
fn interrupted_replacement() {
    let record = case(7);
    let input = format!(
        "{}/input/conversations/{DEFAULT_KEY}/messages.jsonl",
        record.relative_root
    );
    let candidate = format!(
        "{}/candidate/conversations/{DEFAULT_KEY}/messages.jsonl",
        record.relative_root
    );
    let expected = format!("{}/expected/active-messages.jsonl", record.relative_root);
    assert_eq!(load_text(&input), load_text(&expected));
    assert_ne!(load_text(&candidate), load_text(&expected));
    assert_eq!(rows(&input)[1]["message"]["id"], "msg-active-fixture");
    assert_eq!(
        rows(&candidate)[1]["message"]["id"],
        "msg-candidate-fixture"
    );
    assert_eq!(
        record.expected_typescript_behavior,
        TypescriptBehavior::NotApplicableRustHardening
    );
    assert_eq!(
        record.expected_recovery_or_mutation,
        MutationDisposition::CandidateNotCommitted
    );
}

#[test]
fn corrupt_unsupported_manifests() {
    let record = case(8);
    let base = &record.relative_root;
    let corrupt = format!("{base}/corrupt_json/input/conversations/{DEFAULT_KEY}");
    assert!(
        serde_json::from_str::<Value>(&load_text(&format!("{corrupt}/manifest.json"))).is_err()
    );
    for (name, field, expected) in [
        ("unsupported_schema", "schema_version", Value::from(99)),
        (
            "unsupported_provider",
            "provider_stack",
            Value::from("fixture-unsupported-provider"),
        ),
    ] {
        let root = format!("{base}/{name}/input/conversations/{DEFAULT_KEY}");
        let manifest = load_json(&format!("{root}/manifest.json"));
        assert_eq!(manifest[field], expected);
        let active = load_text(&format!("{root}/messages.jsonl"));
        let expected_bytes = load_text(&format!("{base}/{name}/expected/active-bytes"));
        assert_eq!(active, expected_bytes);
    }
    let corrupt_active = load_text(&format!("{corrupt}/messages.jsonl"));
    assert_eq!(
        corrupt_active,
        load_text(&format!("{base}/corrupt_json/expected/active-bytes"))
    );
    assert_eq!(
        record.expected_rust_behavior,
        RustBehavior::RejectsWithoutMutation
    );
}
