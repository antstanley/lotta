use super::*;
use crate::tests::TestRoot;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;

#[test]
fn persisted_record_shape() {
    let fixture = fixture_bytes();
    let record: CompiledPromptRecord = serde_json::from_slice(&fixture).expect("fixture record");
    let absent = serde_json::to_value(&record).expect("absent JSON");
    assert_eq!(
        keys(&absent),
        set(["content", "coreMemory", "compiledAt", "rawSystemHash"])
    );
    let populated = CompiledPromptRecord {
        mid_conversation_system_prompt: Some("transient".into()),
        memfs_revision: Some("revision".into()),
        ..record.clone()
    };
    assert_eq!(
        keys(&serde_json::to_value(&populated).expect("JSON")),
        set([
            "content",
            "coreMemory",
            "midConversationSystemPrompt",
            "compiledAt",
            "rawSystemHash",
            "memfsRevision",
        ])
    );
    let root = TestRoot::new();
    let directory = root.0.join("record");
    fs::create_dir(&directory).expect("directory");
    let cache = CacheRoot::new(&fs::canonicalize(directory).expect("canonical")).expect("cache");
    cache.persist(&populated).expect("persist");
    let bytes = fs::read(root.0.join("record/system-prompt.json")).expect("actual bytes");
    let actual: Value = serde_json::from_slice(&bytes).expect("actual JSON");
    assert_eq!(
        keys(&actual),
        set([
            "content",
            "coreMemory",
            "compiledAt",
            "rawSystemHash",
            "memfsRevision",
        ])
    );
    assert!(actual.get("midConversationSystemPrompt").is_none());
    assert!(
        String::from_utf8(bytes)
            .expect("UTF-8")
            .contains("2000-01-01T00:00:00.000Z")
    );
    let roundtrip: CompiledPromptRecord = serde_json::from_slice(&fixture).expect("roundtrip");
    assert_eq!(serde_json::to_value(roundtrip).expect("value"), absent);
    for added in ["model", "tool", "contentHash", "extra"] {
        let mut value = absent.clone();
        value
            .as_object_mut()
            .expect("object")
            .insert(added.into(), json!("x"));
        assert!(serde_json::from_value::<CompiledPromptRecord>(value).is_err());
    }
}

fn fixture_bytes() -> Vec<u8> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read(root.join(
        "../../fixtures/persistence/current_typescript_state/conversations/\
        ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/system-prompt.json",
    ))
    .expect("fixture")
}
fn keys(value: &Value) -> BTreeSet<String> {
    value.as_object().expect("object").keys().cloned().collect()
}
fn set<const N: usize>(values: [&str; N]) -> BTreeSet<String> {
    values.into_iter().map(str::to_owned).collect()
}
