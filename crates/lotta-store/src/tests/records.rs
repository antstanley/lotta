use crate::{LocalStore, StorePaths};
use lotta_domain::{Agent, Conversation};
use lotta_runtime::ports::{AgentStore, ConversationStore};
use lotta_testkit::fixtures::{FixtureLoader, persistence};
use lotta_testkit::roots::TemporaryRoot;
use std::path::Path;

const AGENT_PATHS: [&str; 9] = [
    "baseline_tolerated_versioned_rows/agents/YWdlbnQtbG9jYWwtZml4dHVyZQ.json",
    "current_typescript_state/agents/YWdlbnQtbG9jYWwtZml4dHVyZQ.json",
    "interrupted_append/input/agents/YWdlbnQtbG9jYWwtZml4dHVyZQ.json",
    "interrupted_replacement/candidate/agents/YWdlbnQtbG9jYWwtZml4dHVyZQ.json",
    "interrupted_replacement/input/agents/YWdlbnQtbG9jYWwtZml4dHVyZQ.json",
    "orphan_result_repair_input/agents/YWdlbnQtbG9jYWwtZml4dHVyZQ.json",
    "rust_target_state/agents/YWdlbnQtbG9jYWwtZml4dHVyZQ.json",
    "unversioned_legacy_transcript/agents/YWdlbnQtbG9jYWwtZml4dHVyZQ.json",
    "versioned_legacy_transcript/agents/YWdlbnQtbG9jYWwtZml4dHVyZQ.json",
];

const CONVERSATION_PATHS: [&str; 10] = [
    concat!(
        "baseline_tolerated_versioned_rows/conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/conversation.json"
    ),
    concat!(
        "current_typescript_state/conversations/",
        "Y29udmVyc2F0aW9uOmNvbnZlcnNhdGlvbi1maXh0dXJl/conversation.json"
    ),
    concat!(
        "current_typescript_state/conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/conversation.json"
    ),
    concat!(
        "interrupted_append/input/conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/conversation.json"
    ),
    concat!(
        "interrupted_replacement/candidate/conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/conversation.json"
    ),
    concat!(
        "interrupted_replacement/input/conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/conversation.json"
    ),
    concat!(
        "orphan_result_repair_input/conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/conversation.json"
    ),
    concat!(
        "rust_target_state/conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/conversation.json"
    ),
    concat!(
        "unversioned_legacy_transcript/conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/conversation.json"
    ),
    concat!(
        "versioned_legacy_transcript/conversations/",
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/conversation.json"
    ),
];

fn root(label: &str) -> (TemporaryRoot, StorePaths) {
    let label = label.replace(['/', '_'], "-");
    let owned = TemporaryRoot::new(&label).expect("temporary root");
    let paths = StorePaths::new(owned.path().join("local-backend")).expect("paths");
    (owned, paths)
}

fn source(path: &str) -> Vec<u8> {
    FixtureLoader::new()
        .load_bytes(format!("persistence/{path}"))
        .expect("indexed fixture")
}

fn pretty<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("serialize");
    bytes.push(b'\n');
    bytes
}

mod agent {
    use super::*;

    pub(super) async fn round_trip(path: &str) {
        let bytes = source(path);
        let typed: Agent = serde_json::from_slice(&bytes).expect("agent");
        let (_owned, paths) = root("corpus-record-case");
        let record = paths.agent_record(&typed.id).expect("record path");
        std::fs::create_dir_all(record.parent().expect("parent")).expect("directory");
        std::fs::write(&record, &bytes).expect("copy source bytes");
        let store = LocalStore::new(paths);
        let loaded = AgentStore::load(&store, &typed.id)
            .await
            .expect("public load");
        assert_eq!(loaded, typed);
        AgentStore::save(&store, &loaded)
            .await
            .expect("public save");
        assert_eq!(
            AgentStore::load(&store, &typed.id).await.expect("reload"),
            typed
        );
        let written = std::fs::read(record).expect("written bytes");
        assert_eq!(written, pretty(&typed));
        let source_json: serde_json::Value = serde_json::from_slice(&bytes).expect("source json");
        let output_json: serde_json::Value = serde_json::from_slice(&written).expect("output json");
        assert_eq!(source_json, output_json);
    }

    pub mod corpus_round_trip {
        use super::*;

        #[test]
        fn inventory_is_complete() {
            let index = persistence::load_index(&FixtureLoader::new()).expect("index");
            let mut actual = index
                .inventory
                .iter()
                .filter(|entry| {
                    entry.path.contains("/agents/")
                        && Path::new(&entry.path)
                            .extension()
                            .is_some_and(|ext| ext == "json")
                })
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>();
            actual.sort_unstable();
            assert_eq!(actual, AGENT_PATHS);
        }

        macro_rules! cases {
            ($($name:ident => $index:expr),+ $(,)?) => {$ (
                #[tokio::test]
                async fn $name() {
                    super::round_trip(AGENT_PATHS[$index]).await;
                }
            )+ };
        }

        cases!(
            tolerated => 0,
            current => 1,
            interrupted_append => 2,
            replacement_candidate => 3,
            replacement_input => 4,
            orphan_repair => 5,
            rust_target => 6,
            unversioned_legacy => 7,
            versioned_legacy => 8,
        );
    }
}

mod conversation {
    use super::*;

    fn normalize_timestamps(mut value: serde_json::Value) -> serde_json::Value {
        for field in ["created_at", "updated_at", "last_message_at", "archived_at"] {
            if let Some(text) = value.get(field).and_then(serde_json::Value::as_str)
                && let Ok(timestamp) = lotta_domain::Timestamp::parse_persisted_rfc3339(text)
            {
                value[field] = serde_json::to_value(timestamp).expect("timestamp json");
            }
        }
        value
    }

    pub(super) async fn round_trip(path: &str) {
        let bytes = source(path);
        let typed: Conversation = serde_json::from_slice(&bytes).expect("conversation");
        let (_owned, paths) = root("corpus-record-case");
        let record = paths
            .conversation_dir(&typed.agent_id, &typed.id)
            .expect("record directory")
            .join("conversation.json");
        std::fs::create_dir_all(record.parent().expect("parent")).expect("directory");
        std::fs::write(&record, &bytes).expect("copy source bytes");
        let store = LocalStore::new(paths);
        let loaded = ConversationStore::load(&store, &typed.agent_id, &typed.id)
            .await
            .expect("public load");
        assert_eq!(loaded, typed);
        ConversationStore::save(&store, &loaded)
            .await
            .expect("public save");
        let reloaded = ConversationStore::load(&store, &typed.agent_id, &typed.id)
            .await
            .expect("reload");
        assert_eq!(reloaded, typed);
        let written = std::fs::read(record).expect("written bytes");
        assert_eq!(written, pretty(&typed));
        let source_json: serde_json::Value = serde_json::from_slice(&bytes).expect("source json");
        let output_json: serde_json::Value = serde_json::from_slice(&written).expect("output json");
        assert_eq!(normalize_timestamps(source_json), output_json);
    }

    pub mod corpus_round_trip {
        use super::*;

        #[test]
        fn inventory_is_complete() {
            let index = persistence::load_index(&FixtureLoader::new()).expect("index");
            let mut actual = index
                .inventory
                .iter()
                .filter(|entry| entry.path.ends_with("/conversation.json"))
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>();
            actual.sort_unstable();
            assert_eq!(actual, CONVERSATION_PATHS);
        }

        macro_rules! cases {
            ($($name:ident => $index:expr),+ $(,)?) => {$ (
                #[tokio::test]
                async fn $name() {
                    super::round_trip(CONVERSATION_PATHS[$index]).await;
                }
            )+ };
        }

        cases!(
            tolerated => 0,
            current_named => 1,
            current_default => 2,
            interrupted_append => 3,
            replacement_candidate => 4,
            replacement_input => 5,
            orphan_repair => 6,
            rust_target => 7,
            unversioned_legacy => 8,
            versioned_legacy => 9,
        );
    }
}
