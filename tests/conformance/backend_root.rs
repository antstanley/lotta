use super::support;
use lotta_domain::{
    Agent, Conversation, ProviderStack, SessionEntry, SessionEntryType, TranscriptEntry,
    TranscriptManifest, TranscriptMessageFormat,
};
use lotta_memfs::prompt::{CacheRoot, CompiledPromptRecord};
use lotta_runtime::ports::{AgentStore, ConversationStore};
use serde_json::Value;

async fn ts_to_rust(artifact: &str) {
    let root = support::TestRoot::new(&format!("backend-ts-{artifact}"));
    let response = support::run_ts(&root, "backend_write", artifact);
    let store = support::backend_store(&root);
    let value = rust_read(&store, artifact).await;
    support::assert_contains(
        &response["value"],
        &value,
        "",
        "typescript-to-rust",
        artifact,
    );
    if artifact == "agent" {
        let _lease = support::Lease::acquire(root.path(), "rust-agent-update")
            .expect("agent update handoff lease");
        let name = lotta_domain::NonEmptyString::new("Rust touched").expect("name");
        store
            .update_agent_name(&support::agent_id(), name)
            .await
            .expect("agent update");
        let after = support::read_json(
            &store
                .paths()
                .agent_record(&support::agent_id())
                .expect("path"),
        );
        support::assert_contains(
            &response["value"]["ts_extension"],
            &after["ts_extension"],
            "/ts_extension",
            "typescript-to-rust-update",
            artifact,
        );
    }
}

async fn rust_to_ts(artifact: &str) {
    let root = support::TestRoot::new(&format!("backend-rust-{artifact}"));
    let lease = support::Lease::acquire(root.path(), "rust-backend-writer")
        .expect("backend writer handoff lease");
    let store = support::backend_store(&root);
    rust_write(&store, artifact).await;
    drop(store);
    drop(lease);
    let before = disk_value(&root, artifact);
    let response = support::run_ts(&root, "backend_read_update", artifact);
    support::assert_contains(
        &before,
        &response["value"],
        "",
        "rust-to-typescript",
        artifact,
    );
}

async fn rust_read(store: &lotta_store::LocalStore, artifact: &str) -> Value {
    match artifact {
        "agent" => serde_json::to_value(
            AgentStore::load(store, &support::agent_id())
                .await
                .expect("load agent"),
        )
        .expect("agent value"),
        "conversation" => serde_json::to_value(
            ConversationStore::load(store, &support::agent_id(), &support::conversation_id())
                .await
                .expect("load conversation"),
        )
        .expect("conversation value"),
        "transcript" => {
            store
                .load_transcript(&support::agent_id(), &support::conversation_id())
                .await
                .expect("load transcript");
            Value::Array(support::json_rows(
                &conversation_dir(store).join("messages.jsonl"),
            ))
        }
        "manifest" => serde_json::to_value(
            store
                .read_transcript_manifest(&support::agent_id(), &support::conversation_id())
                .await
                .expect("load manifest"),
        )
        .expect("manifest value"),
        "system_prompt" => {
            let cache = prompt_cache(store);
            let value = cache
                .load()
                .expect("load prompt cache")
                .expect("prompt record");
            serde_json::to_value(value).expect("prompt value")
        }
        _ => panic!("unsupported artifact"),
    }
}

async fn rust_write(store: &lotta_store::LocalStore, artifact: &str) {
    let agent = agent_value();
    AgentStore::save(store, &agent).await.expect("save agent");
    let conversation = conversation_value();
    ConversationStore::save(store, &conversation)
        .await
        .expect("save conversation");
    if matches!(artifact, "transcript" | "manifest" | "system_prompt") {
        initialize(store).await;
    }
    if artifact == "system_prompt" {
        let record: CompiledPromptRecord = serde_json::from_value(serde_json::json!({
            "content": "synthetic Rust system prompt",
            "coreMemory": "synthetic Rust memory",
            "compiledAt": "2026-08-14T00:00:00.000Z",
            "rawSystemHash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "memfsRevision": "rust-revision"
        }))
        .expect("compiled prompt record");
        prompt_cache(store)
            .persist(&record)
            .expect("persist prompt record");
    }
}

async fn initialize(store: &lotta_store::LocalStore) {
    let manifest = TranscriptManifest {
        schema_version: 2,
        message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
        provider_stack: ProviderStack::PiAi,
        created_at: timestamp(),
        migrated_from: None,
        migrated_at: None,
        backup_path: None,
    };
    let session = TranscriptEntry::Session(SessionEntry {
        entry_type: SessionEntryType::Session,
        version: 3,
        id: lotta_domain::NonEmptyString::new("rust-session").expect("session id"),
        timestamp: timestamp(),
        cwd: "/synthetic/workspace".to_owned(),
    });
    store
        .initialize_transcript(
            &support::agent_id(),
            &support::conversation_id(),
            &manifest,
            &session,
        )
        .await
        .expect("initialize transcript");
}

fn agent_value() -> Agent {
    serde_json::from_value(serde_json::json!({
        "id": support::AGENT_ID, "name": "Rust Agent", "description": null,
        "system": "synthetic", "tags": ["rust"], "model": "openai/gpt-4.1-mini",
        "model_settings": {}, "rust_extension": {"nested": {"retained": true}}
    }))
    .expect("agent")
}

fn conversation_value() -> Conversation {
    serde_json::from_value(serde_json::json!({
        "id": support::CONVERSATION_ID, "agent_id": support::AGENT_ID,
        "archived": false, "archived_at": null, "created_at": "2026-08-14T00:00:00Z",
        "updated_at": "2026-08-14T00:00:00Z", "last_message_at": null,
        "summary": null, "in_context_message_ids": [], "rust_extension": {"retained": true}
    }))
    .expect("conversation")
}

fn timestamp() -> lotta_domain::Timestamp {
    lotta_domain::Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z").expect("timestamp")
}

fn conversation_dir(store: &lotta_store::LocalStore) -> std::path::PathBuf {
    store
        .paths()
        .conversation_dir(&support::agent_id(), &support::conversation_id())
        .expect("conversation dir")
}

fn prompt_cache(store: &lotta_store::LocalStore) -> CacheRoot {
    let directory = conversation_dir(store);
    std::fs::create_dir_all(&directory).expect("create conversation directory");
    let canonical = std::fs::canonicalize(directory).expect("canonical conversation directory");
    CacheRoot::new(&canonical).expect("prompt cache authority")
}

fn disk_value(root: &support::TestRoot, artifact: &str) -> Value {
    let store = support::backend_store(root);
    let directory = conversation_dir(&store);
    match artifact {
        "agent" => support::read_json(
            &store
                .paths()
                .agent_record(&support::agent_id())
                .expect("agent path"),
        ),
        "conversation" => support::read_json(&directory.join("conversation.json")),
        "transcript" => Value::Array(support::json_rows(&directory.join("messages.jsonl"))),
        "manifest" => support::read_json(&directory.join("manifest.json")),
        "system_prompt" => support::read_json(&directory.join("system-prompt.json")),
        _ => panic!("unsupported artifact"),
    }
}

macro_rules! directional_tests {
    ($module:ident, $artifact:literal) => {
        mod $module {
            #[tokio::test]
            async fn typescript_to_rust() {
                super::ts_to_rust($artifact).await;
            }
            #[tokio::test]
            async fn rust_to_typescript() {
                super::rust_to_ts($artifact).await;
            }
        }
    };
}

directional_tests!(agent_json, "agent");
directional_tests!(conversation_json, "conversation");
directional_tests!(transcript_messages_jsonl, "transcript");
directional_tests!(manifest_json, "manifest");
directional_tests!(system_prompt_json, "system_prompt");
