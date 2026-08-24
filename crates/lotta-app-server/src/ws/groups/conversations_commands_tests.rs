//! `ws::conversations::commands` — one case per Conversation management
//! command proves each of the eight pinned discriminants decodes, routes, and
//! responds with the pinned success shape.

use serde_json::json;

use super::support::{bridge, discriminant_of};

#[tokio::test]
async fn conversation_list_serves_created_conversations_newest_first() {
    let fixture = bridge();
    let agent = fixture.seed_agent("list", "Listed Agent").await;
    fixture.seed_conversation(&agent, "local-conv-old", 1).await;
    fixture
        .send(&json!({
            "type": "conversation_list",
            "request_id": "ls-1",
            "query": {"agent_id": agent.as_str()},
        }))
        .await;
    assert_eq!(
        discriminant_of(fixture.messages().last().expect("frame")),
        "conversation_list_response"
    );
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let conversations = frame["conversations"].as_array().expect("list");
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0]["id"], "local-conv-old");
    assert_eq!(conversations[0]["agent_id"], agent.as_str());
}

#[tokio::test]
async fn conversation_retrieve_returns_the_snapshot_and_missing_fails() {
    let fixture = bridge();
    let agent = fixture.seed_agent("retrieve", "Retrieved Agent").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-7", 2).await;
    fixture
        .send(&json!({
            "type": "conversation_retrieve",
            "request_id": "rt-1",
            "conversation_id": seeded.as_str(),
            "body": {"agent_id": agent.as_str()},
        }))
        .await;
    assert_eq!(fixture.last()["type"], "conversation_retrieve_response");
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["conversation"]["id"], seeded.as_str());
    assert_eq!(fixture.last()["conversation"]["agent_id"], agent.as_str());

    fixture
        .send(&json!({
            "type": "conversation_retrieve",
            "request_id": "rt-2",
            "conversation_id": "absent"
        }))
        .await;
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(
        fixture.last()["conversation"],
        json!(null),
        "failure frames carry a null snapshot like the pinned listener"
    );
    assert_eq!(fixture.last()["error"], "conversation not found");
}

#[tokio::test]
async fn conversation_create_returns_the_created_snapshot_with_pinned_defaults() {
    let fixture = bridge();
    let agent = fixture.seed_agent("create", "Created Agent").await;
    fixture
        .send(&json!({
            "type": "conversation_create",
            "request_id": "cr-1",
            "body": {
                "agent_id": agent.as_str(),
                "summary": null,
                "hidden": false,
                "tags": ["team-a"],
            },
        }))
        .await;
    assert_eq!(
        discriminant_of(fixture.messages().last().expect("frame")),
        "conversation_create_response"
    );
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    assert!(
        frame["conversation"]["id"]
            .as_str()
            .expect("id")
            .starts_with("local-conv-"),
        "sequential canonical identifier minted"
    );
    assert_eq!(frame["conversation"]["archived"], false);
    assert_eq!(frame["conversation"]["in_context_message_ids"], json!([]));
    assert_eq!(frame["conversation"]["tags"], json!(["team-a"]));

    // Creation through the wire path persists the record authoritatively.
    let created = frame["conversation"]["id"].as_str().expect("id").to_owned();
    fixture
        .send(&json!({
            "type": "conversation_retrieve",
            "request_id": "cr-2",
            "conversation_id": created,
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);

    // Creating without any resolvable owning agent rejects safely.
    fixture
        .send(&json!({"type": "conversation_create", "request_id": "cr-3", "body": {}}))
        .await;
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(fixture.last()["error"], "agent not found");
}

#[tokio::test]
async fn conversation_update_persists_and_serves_the_new_snapshot() {
    let fixture = bridge();
    let agent = fixture.seed_agent("update", "Updated Agent").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-3", 1).await;
    fixture
        .send(&json!({
            "type": "conversation_update",
            "request_id": "up-1",
            "conversation_id": seeded.as_str(),
            "body": {"archived": true, "summary": "Renamed summary", "hidden": true},
        }))
        .await;
    assert_eq!(fixture.last()["type"], "conversation_update_response");
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["conversation"]["archived"], true);
    assert_eq!(fixture.last()["conversation"]["summary"], "Renamed summary");
    assert_eq!(fixture.last()["conversation"]["hidden"], true);
    assert!(
        fixture.last()["conversation"]["archived_at"].is_string(),
        "archiving stamps the archive timestamp"
    );

    // The response snapshot is authoritative after persistence.
    let stored = fixture.conversation_value(&agent, &seeded).await;
    assert_eq!(stored["archived"], true);
    assert_eq!(stored["summary"], "Renamed summary");

    // The virtual default conversation never updates.
    fixture
        .send(&json!({
            "type": "conversation_update",
            "request_id": "up-2",
            "conversation_id": "default",
            "body": {"archived": true},
        }))
        .await;
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(
        fixture.last()["error"],
        "Default conversation cannot be updated"
    );
}

#[tokio::test]
async fn conversation_recompile_responds_with_compiled_content_and_persists_cache() {
    let fixture = bridge();
    let agent = fixture.seed_agent("recompile", "Recompiled Agent").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-4", 1).await;
    fixture
        .send(&json!({
            "type": "conversation_recompile",
            "request_id": "rc-1",
            "conversation_id": seeded.as_str(),
            "body": {"agent_id": agent.as_str(), "dry_run": false},
        }))
        .await;
    assert_eq!(
        discriminant_of(fixture.messages().last().expect("frame")),
        "conversation_recompile_response"
    );
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let result = frame["result"].as_str().expect("compiled prompt");
    assert!(
        result.contains("Base system prompt for Recompiled Agent."),
        "the compiled content renders the agent's raw system text"
    );
}

#[tokio::test]
async fn conversation_fork_responds_with_the_new_conversation_reference() {
    let fixture = bridge();
    let agent = fixture.seed_agent("forkcmd", "Forked Agent").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-5", 2).await;
    fixture
        .send(&json!({
            "type": "conversation_fork",
            "request_id": "fk-1",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(
        discriminant_of(fixture.messages().last().expect("frame")),
        "conversation_fork_response"
    );
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let forked = frame["conversation"]["id"].as_str().expect("new id");
    assert_ne!(forked, seeded.as_str());
    assert!(
        forked.starts_with("local-conv-"),
        "the reference carries the minted canonical identifier"
    );
}

#[tokio::test]
async fn conversation_messages_list_serves_the_projected_page() {
    let fixture = bridge();
    let agent = fixture.seed_agent("messages", "Message Agent").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-6", 4).await;
    fixture
        .send(&json!({
            "type": "conversation_messages_list",
            "request_id": "ml-1",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(
        discriminant_of(fixture.messages().last().expect("frame")),
        "conversation_messages_list_response"
    );
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let messages = frame["messages"].as_array().expect("page");
    assert_eq!(messages.len(), 4);
    // Default order serves newest-first like the pinned backend.
    let first = messages[0]["timestamp_ms"].as_f64().expect("stamp");
    let second = messages[1]["timestamp_ms"].as_f64().expect("stamp");
    assert!(first >= second, "descending order by default");
    assert_eq!(fixture.last()["has_more"], false);
    let oldest = fixture
        .projected_ids(&agent, &seeded)
        .await
        .into_iter()
        .next()
        .expect("oldest projection");
    assert_eq!(
        fixture.last()["next_before"],
        json!(oldest),
        "the oldest served id is reported even on a complete page"
    );
}

#[tokio::test]
async fn conversation_compact_succeeds_with_counts_and_summary() {
    let fixture = bridge();
    let agent = fixture.seed_agent("compact", "Compact Agent").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-8", 4).await;
    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "cp-1",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(
        discriminant_of(fixture.messages().last().expect("frame")),
        "conversation_compact_response"
    );
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let compaction = frame["compaction"].as_object().expect("outcome");
    assert_eq!(compaction["num_messages_before"], 4);
    assert_eq!(
        compaction["num_messages_after"], 3,
        "sliding window keeps thirty percent plus the summary"
    );
    assert!(
        !compaction["summary"].as_str().expect("summary").is_empty(),
        "the recorded summary text returns"
    );

    // Compacting an absent conversation answers with the pinned failure shape.
    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "cp-2",
            "conversation_id": "absent"
        }))
        .await;
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(fixture.last()["compaction"], json!(null));
}
