//! `ws::agents::commands` — one case per Agent management command proves each
//! of the six pinned discriminants decodes, routes, and responds with the
//! pinned success shape.

use serde_json::{Value, json};

use super::support::{bridge, discriminant_of};

/// Extracts a `&str` field from the latest forwarded frame.
fn text_field(frame: &Value, field: &str) -> String {
    frame[field].as_str().expect(field).to_owned()
}

#[tokio::test]
async fn create_shortcut_responds_with_the_created_agent_identity() {
    let fixture = bridge();
    fixture
        .send(&json!({"type": "create_agent", "request_id": "sc-0", "personality": "memo"}))
        .await;
    let frame = fixture.last();
    assert_eq!(frame["type"], "create_agent_response");
    assert_eq!(frame["success"], true);
    assert!(
        text_field(&frame, "agent_id").starts_with("agent-local-"),
        "canonical local agent id prefix"
    );
    assert_eq!(frame["name"], "Letta Code");
    assert_eq!(frame["model"], "auto-chat");
}

#[tokio::test]
async fn agent_list_serves_the_created_agents() {
    let fixture = bridge();
    let created = fixture.create_agent("Listed Agent").await;
    fixture
        .send(&json!({"type": "agent_list", "request_id": "ls-1"}))
        .await;
    assert_eq!(fixture.last()["type"], "agent_list_response");
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let agents = frame["agents"].as_array().expect("agent list");
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], created.as_str());
    assert_eq!(agents[0]["name"], "Listed Agent");
}

#[tokio::test]
async fn agent_retrieve_returns_the_full_snapshot() {
    let fixture = bridge();
    let created = fixture.create_agent("Retrieved Agent").await;
    fixture
        .send(&json!({
            "type": "agent_retrieve",
            "request_id": "rt-2",
            "agent_id": created,
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_retrieve_response");
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["agent"]["name"], "Retrieved Agent");
    assert_eq!(fixture.last()["agent"]["id"], created.as_str());
}

#[tokio::test]
async fn agent_create_returns_the_full_created_snapshot() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "cr-3",
            "body": {
                "name": "Created Agent",
                "system": "Custom system prompt.",
                "tags": ["team-a"],
            },
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_create_response");
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let agent = frame["agent"].as_object().expect("snapshot");
    assert_eq!(agent["name"], "Created Agent");
    assert_eq!(agent["system"], "Custom system prompt.");
    let tags = agent["tags"].as_array().expect("tags");
    assert!(
        tags.iter().any(|tag| tag == "git-memory-enabled"),
        "the Git-memory tag is stamped at creation"
    );
    assert!(
        tags.iter().any(|tag| tag == "team-a"),
        "requested tags kept"
    );
}

#[tokio::test]
async fn agent_update_persists_and_serves_the_new_snapshot() {
    let fixture = bridge();
    let created = fixture.create_agent("Original Name").await;
    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "up-4",
            "agent_id": created,
            "body": {"name": "Renamed Agent", "description": null},
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_update_response");
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["agent"]["name"], "Renamed Agent");
    assert_eq!(fixture.last()["agent"]["description"], json!(null));
    // The snapshot response is authoritative after persistence.
    fixture
        .send(&json!({
            "type": "agent_retrieve",
            "request_id": "up-5",
            "agent_id": created,
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["agent"]["name"], "Renamed Agent");
}

#[tokio::test]
async fn agent_delete_acknowledges_by_identifier() {
    let fixture = bridge();
    let created = fixture.create_agent("Doomed Agent").await;
    fixture
        .send(&json!({"type": "agent_delete", "request_id": "dl-6", "agent_id": created}))
        .await;
    assert_eq!(
        discriminant_of(fixture.messages().last().expect("response")),
        "agent_delete_response"
    );
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["agent_id"], created.as_str());
    // Deletion is observable: the identifier no longer resolves.
    fixture
        .send(&json!({
            "type": "agent_retrieve",
            "request_id": "dl-7",
            "agent_id": created,
        }))
        .await;
    assert_eq!(fixture.last()["success"], false);
}
