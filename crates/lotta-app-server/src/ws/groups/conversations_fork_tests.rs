//! `ws::conversations::fork` — the fork command writes a brand-new key-form
//! directory carrying the inherited history and leaves every source byte
//! untouched (§Transcript contract full-rewrite case).

use lotta_store::ConversationKey;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::support::{TestConversations, bridge};

fn hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn source_hashes(
    fixture: &TestConversations,
    agent: &lotta_domain::AgentId,
    source: &lotta_domain::ConversationId,
) -> (String, String) {
    let transcript =
        std::fs::read(fixture.transcript_path(agent, source)).expect("source transcript");
    let record = fixture
        .root
        .join("conversations")
        .join(
            ConversationKey::from_ids(agent, source)
                .encoded()
                .expect("encoded key"),
        )
        .join("conversation.json");
    let record_bytes = std::fs::read(record).expect("source record");
    (hash(&transcript), hash(&record_bytes))
}

fn target_dir(
    fixture: &TestConversations,
    agent: &lotta_domain::AgentId,
    id: &lotta_domain::ConversationId,
) -> std::path::PathBuf {
    let encoded = ConversationKey::from_ids(agent, id)
        .encoded()
        .expect("encoded key");
    fixture.root.join("conversations").join(encoded)
}

#[tokio::test]
async fn fork_writes_a_new_key_directory_inherits_history_and_keeps_the_source_byte_identical() {
    let fixture = bridge();
    let agent = fixture.seed_agent("fork", "Fork Agent").await;
    let source = fixture.seed_conversation(&agent, "local-conv-20", 4).await;
    let before = source_hashes(&fixture, &agent, &source);

    fixture
        .send(&json!({
            "type": "conversation_fork",
            "request_id": "fork-1",
            "conversation_id": source.as_str(),
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    let forked_text = fixture.last()["conversation"]["id"]
        .as_str()
        .expect("new id")
        .to_owned();
    let forked = lotta_domain::ConversationId::accept(forked_text.clone()).expect("forked id");

    // The new conversation owns its own encoded key-form directory.
    let directory = target_dir(&fixture, &agent, &forked);
    assert!(
        directory.is_dir(),
        "the fork owns a distinct key-form directory"
    );
    assert!(directory.join("manifest.json").is_file());
    assert!(directory.join("messages.jsonl").is_file());

    // The inherited history is complete inside the fork's own directory.
    let inherited = fixture
        .store
        .load_transcript(&agent, &forked)
        .await
        .expect("forked transcript");
    assert_eq!(inherited.messages().len(), 4);
    assert!(
        inherited.messages()[0]
            .id
            .as_str()
            .starts_with(&format!("ui-msg-{}", source.as_str())),
        "inherited entries keep their source identities"
    );

    // The source conversation is byte-identical after the rewrite.
    let after = source_hashes(&fixture, &agent, &source);
    assert_eq!(before.0, after.0, "source transcript bytes unchanged");
    assert_eq!(before.1, after.1, "source conversation record unchanged");
}

#[tokio::test]
async fn fork_through_message_id_keeps_only_the_prefix_and_still_leaves_the_source_untouched() {
    let fixture = bridge();
    let agent = fixture.seed_agent("forkcut", "Fork Cut Agent").await;
    let source = fixture.seed_conversation(&agent, "local-conv-21", 4).await;
    let before = source_hashes(&fixture, &agent, &source);
    let ids = fixture.projected_ids(&agent, &source).await;
    let cursor = ids[1].clone();

    fixture
        .send(&json!({
            "type": "conversation_fork",
            "request_id": "fork-2",
            "conversation_id": source.as_str(),
            "body": {"message_id": cursor},
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    let Value::String(forked_text) = fixture.last()["conversation"]["id"].clone() else {
        panic!("fork reference missing");
    };
    let forked = lotta_domain::ConversationId::accept(forked_text.clone()).expect("forked id");

    let inherited = fixture
        .store
        .load_transcript(&agent, &forked)
        .await
        .expect("forked transcript");
    assert_eq!(
        inherited.messages().len(),
        2,
        "the inclusive message_id cursor keeps only its prefix"
    );
    let record = fixture.conversation_value(&agent, &forked).await;
    assert_eq!(
        record["in_context_message_ids"].as_array().map(Vec::len),
        Some(2),
        "context ids match the kept prefix"
    );

    let after = source_hashes(&fixture, &agent, &source);
    assert_eq!(before.0, after.0, "source transcript bytes unchanged");
}
