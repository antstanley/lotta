use super::*;
use serde_json::{Map, Value};

mod cases;
mod integrity;
mod keys;
mod sanitization;
mod side_stores;

const AGENT_KEY: &str = "YWdlbnQtbG9jYWwtZml4dHVyZQ";
const DEFAULT_KEY: &str = "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl";
const NAMED_KEY: &str = "Y29udmVyc2F0aW9uOmNvbnZlcnNhdGlvbi1maXh0dXJl";
const AGENT_ID: &str = "agent-local-fixture";
const CONVERSATION_ID: &str = "conversation-fixture";
const TIME: &str = "2000-01-01T00:00:00.000Z";

fn index() -> PersistenceIndex {
    load_index(&FixtureLoader::new()).expect("valid persistence index")
}

fn load_json(relative: &str) -> Value {
    FixtureLoader::new()
        .load(format!("persistence/{relative}"))
        .expect("valid JSON fixture")
}

fn load_text(relative: &str) -> String {
    FixtureLoader::new()
        .load_text(format!("persistence/{relative}"))
        .expect("valid text fixture")
}

fn object(value: &Value) -> &Map<String, Value> {
    value.as_object().expect("JSON object")
}

fn keys(value: &Value) -> Vec<&str> {
    let mut values = object(value).keys().map(String::as_str).collect::<Vec<_>>();
    values.sort_unstable();
    values
}

fn rows(relative: &str) -> Vec<Value> {
    load_text(relative)
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid JSONL row"))
        .collect()
}

fn conversation_root(case: &str, key: &str) -> String {
    format!("{case}/conversations/{key}")
}

fn assert_manifest(path: &str, schema: u64, format: &str) {
    let value = load_json(path);
    assert_eq!(value["schema_version"], schema);
    assert_eq!(value["message_format"], format);
    assert_eq!(value["provider_stack"], "pi-ai");
    assert_eq!(value["created_at"], TIME);
}

fn assert_index_format(path: &str, expected: &str) {
    let value = index();
    let item = value
        .side_stores
        .iter()
        .find(|item| item.path == path)
        .expect("indexed side store");
    assert_eq!(item.format, expected);
}
