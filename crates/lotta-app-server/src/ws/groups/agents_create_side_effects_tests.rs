//! `ws::agents::create_side_effects` — all four §Agent creation side effects
//! (Git-memory tag, initialized memory files, created repository, compiled
//! default prompt) are asserted at the moment the success response is
//! observed, and compilation is proven to complete before that emission.

use serde_json::{Value, json};

use super::support::{FIXTURE_TIMESTAMP_TEXT, bridge};
use lotta_domain::Timestamp;

/// Reads a nested field of the latest frame as text.
fn text_at(frame: &Value, pointer: &str) -> String {
    frame
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn creation_completes_all_four_side_effects_before_the_response() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "se-0",
            "body": {
                "name": "Memory Agent",
                "system": "System with memory.",
                "memory_blocks": [
                    {
                        "label": "persona",
                        "value": "Remembers project facts.",
                        "description": "Persona",
                    },
                ],
            },
        }))
        .await;
    let frame = fixture.last();
    assert_eq!(frame["type"], "agent_create_response");
    assert_eq!(frame["success"], true);
    let agent_id = text_at(&frame, "/agent/id");
    assert!(!agent_id.is_empty(), "creation succeeded");

    // Side effect (a): the Git-memory tag is stamped on the agent itself.
    let tags = frame["agent"]["tags"].as_array().expect("tag list");
    assert!(
        tags.iter().any(|tag| tag == "git-memory-enabled"),
        "git-memory-enabled stamped at creation"
    );

    // Side effect (b): every memory block was initialized as a file.
    let persona = fixture.memory_root(&agent_id).join("system/persona.md");
    let contents = std::fs::read_to_string(&persona).expect("initialized persona file");
    assert!(
        contents.contains("Remembers project facts."),
        "memory file rendered from the wire memory_blocks"
    );

    // Side effect (c): the Git repository exists under the canonical root.
    assert!(
        fixture.memory_root(&agent_id).join(".git").is_dir(),
        "the MemFS repository was created"
    );

    // Side effect (d): the default conversation prompt was compiled and
    // persisted before the response was emitted.
    let record = fixture
        .load_prompt_record(&agent_id)
        .expect("compiled prompt record");
    assert!(
        !record.content.is_empty(),
        "the compiled default prompt carries content"
    );
    assert!(
        record.memfs_revision.is_some(),
        "compilation rendered the committed repository revision"
    );
    assert_eq!(
        record.compiled_at,
        Timestamp::parse_persisted_rfc3339(FIXTURE_TIMESTAMP_TEXT).expect("fixture timestamp"),
        "the persisted record was produced by this handler's clock"
    );
}

#[tokio::test]
async fn shortcut_creation_runs_the_same_creation_side_effects() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "create_agent",
            "request_id": "se-1",
            "personality": "tutorial",
        }))
        .await;
    let frame = fixture.last();
    assert_eq!(frame["success"], true);
    let agent_id = text_at(&frame, "/agent_id");

    // The shortcut routes through the same creation core: retrieve proves the
    // tag side effect, the layout proves repository, files, and prompt.
    fixture
        .send(&json!({
            "type": "agent_retrieve",
            "request_id": "se-2",
            "agent_id": agent_id,
        }))
        .await;
    let frame = fixture.last();
    let tags = frame["agent"]["tags"].as_array().expect("tag list");
    assert!(tags.iter().any(|tag| tag == "git-memory-enabled"));
    assert!(
        fixture.memory_root(&agent_id).join(".git").is_dir(),
        "the shortcut created the repository"
    );
    assert!(
        fixture
            .memory_root(&agent_id)
            .join("system/persona.md")
            .is_file(),
        "the shortcut initialized preset memory files"
    );
    let record = fixture
        .load_prompt_record(&agent_id)
        .expect("shortcut compiled the default prompt");
    assert!(!record.content.is_empty());
}

#[tokio::test]
async fn deletion_removes_conversations_prompt_cache_memfs_and_record() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "del-1",
            "body": {"name": "Doomed With Artifacts"},
        }))
        .await;
    let agent_id = fixture.last()["agent"]["id"]
        .as_str()
        .expect("created id")
        .to_owned();

    // The prompt-only default-conversation cache exists after creation but
    // carries no conversation record, so only explicit removal can clear it.
    assert!(
        fixture.default_conversation_dir(&agent_id).is_dir(),
        "the prompt cache directory exists before deletion"
    );

    fixture
        .send(&json!({
            "type": "agent_delete",
            "request_id": "del-2",
            "agent_id": agent_id,
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_delete_response");
    assert_eq!(fixture.last()["success"], true);

    // Every artifact class is gone: the ownerless prompt cache, any encoded
    // conversation directories, the MemFS repository, and the record.
    assert!(
        !fixture.default_conversation_dir(&agent_id).exists(),
        "the ownerless prompt cache was removed"
    );
    let conversations_root = fixture.root.join("conversations");
    let leftovers = std::fs::read_dir(&conversations_root).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(&agent_id))
            .count()
    });
    assert_eq!(leftovers, 0, "no conversation directories survive");
    assert!(
        !fixture.root.join("memfs").join(&agent_id).exists(),
        "the MemFS repository was removed"
    );

    // The record itself no longer resolves.
    fixture
        .send(&json!({
            "type": "agent_retrieve",
            "request_id": "del-3",
            "agent_id": agent_id,
        }))
        .await;
    assert_eq!(fixture.last()["success"], false);
}
