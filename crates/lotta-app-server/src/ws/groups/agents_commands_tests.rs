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
    // The memo preset has no default model, so the catalog default resolves.
    assert_eq!(frame["model"], "letta/auto");
}

#[tokio::test]
async fn shortcut_pins_by_default_before_responding() {
    let fixture = bridge();
    fixture
        .send(&json!({"type": "create_agent", "request_id": "pin-1", "personality": "linus"}))
        .await;
    let agent_id = text_field(&fixture.last(), "agent_id");
    // At the moment the response is observed, the pin is already durable in
    // the pinned-agent side store.
    assert!(
        fixture.is_pinned(&agent_id),
        "the created agent is pinned when the response is emitted"
    );

    // A second default-pinned creation updates the existing document through
    // its revision token instead of clobbering it.
    fixture
        .send(&json!({"type": "create_agent", "request_id": "pin-2", "personality": "memo"}))
        .await;
    let second = text_field(&fixture.last(), "agent_id");
    assert!(fixture.is_pinned(&second));
    assert!(
        fixture.is_pinned(&agent_id),
        "the earlier pin survived the second write"
    );

    // An explicit false opts out of pinning.
    fixture
        .send(&json!({
            "type": "create_agent",
            "request_id": "pin-3",
            "personality": "blank",
            "pin_global": false,
        }))
        .await;
    let unpinned = text_field(&fixture.last(), "agent_id");
    assert!(
        !fixture.is_pinned(&unpinned),
        "pin_global: false skips the pin"
    );
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

#[tokio::test]
async fn shortcut_resolves_model_ids_to_handles_and_rejects_unknown_models() {
    let fixture = bridge();
    // A catalog id resolves to its canonical handle before any side effect.
    fixture
        .send(&json!({
            "type": "create_agent",
            "request_id": "model-1",
            "personality": "memo",
            "model": "auto-chat",
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["model"], "letta/auto-chat");

    // A slash-bearing handle outside the catalog passes through unchanged.
    fixture
        .send(&json!({
            "type": "create_agent",
            "request_id": "model-2",
            "personality": "memo",
            "model": "self-hosted/custom-model",
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["model"], "self-hosted/custom-model");

    // Unknown identifiers reject with the pinned detail and zero side
    // effects: no record, no MemFS root, no pin entry.
    let seeded = std::fs::read_dir(fixture.agents_dir()).map_or(0, std::iter::Iterator::count);
    fixture
        .send(&json!({
            "type": "create_agent",
            "request_id": "model-3",
            "personality": "memo",
            "model": "not-a-real-model",
        }))
        .await;
    assert_eq!(fixture.last()["type"], "create_agent_response");
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(
        fixture.last()["error"],
        "Unknown model \"not-a-real-model\""
    );
    let after_rejection =
        std::fs::read_dir(fixture.agents_dir()).map_or(0, std::iter::Iterator::count);
    assert_eq!(seeded, after_rejection, "no record was created");
}

#[tokio::test]
async fn shortcut_stamps_canonical_tags_and_preset_metadata() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "create_agent",
            "request_id": "tags-1",
            "personality": "tutorial",
            "tags": ["team-b"],
        }))
        .await;
    let frame = fixture.last();
    assert_eq!(frame["success"], true);
    let agent_id = text_field(&frame, "agent_id");

    // Retrieve serves the authoritative snapshot for content assertions.
    fixture
        .send(&json!({
            "type": "agent_retrieve",
            "request_id": "tags-2",
            "agent_id": agent_id,
        }))
        .await;
    let agent = fixture.last()["agent"].clone();

    // Tag order mirrors the pinned stamping: origin, Git-memory, personality,
    // then the caller's tags — deduplicated on first occurrence.
    let tags: Vec<String> = agent["tags"]
        .as_array()
        .expect("tag list")
        .iter()
        .map(|tag| tag.as_str().expect("tag").to_owned())
        .collect();
    assert_eq!(
        tags,
        vec![
            "origin:letta-code".to_owned(),
            "git-memory-enabled".to_owned(),
            "personality:tutorial".to_owned(),
            "team-b".to_owned(),
        ],
        "canonical creation tags in pinned order"
    );

    // The description is the canonical preset metadata string.
    assert_eq!(
        agent["description"],
        "I help with getting started with Letta. I can answer any questions about Letta, \
         and also help you create and configure agents."
    );
}

#[tokio::test]
async fn shortcut_seeds_canonical_memory_blocks_and_system_prompt() {
    let fixture = bridge();
    fixture
        .send(&json!({"type": "create_agent", "request_id": "mem-1", "personality": "kawaii"}))
        .await;
    let agent_id = text_field(&fixture.last(), "agent_id");
    let persona_path = fixture.memory_root(&agent_id).join("system/persona.md");
    let persona = std::fs::read_to_string(&persona_path).expect("persona file");
    // The persona file carries the pinned asset's description and body.
    assert!(
        persona.contains("\"A sparkly memory for my kawaii self~"),
        "the persona block carries the pinned asset description"
    );
    assert!(
        persona.contains("My name is Letta Code~"),
        "the persona block carries the pinned asset body"
    );
    let human_path = fixture.memory_root(&agent_id).join("system/human.md");
    let human = std::fs::read_to_string(&human_path).expect("human file");
    assert!(
        !human.contains("Letta Code~"),
        "the human block renders its own asset, not the persona asset"
    );

    fixture
        .send(&json!({
            "type": "agent_retrieve",
            "request_id": "mem-2",
            "agent_id": agent_id,
        }))
        .await;
    let system = fixture.last()["agent"]["system"]
        .as_str()
        .expect("system prompt")
        .to_owned();
    assert!(
        system.len() > 1_000,
        "the canonical default memfs system prompt asset is embedded verbatim"
    );
}

#[tokio::test]
async fn tutorial_shortcut_seeds_the_onboarding_block() {
    let fixture = bridge();
    fixture
        .send(&json!({"type": "create_agent", "request_id": "tut-1", "personality": "tutorial"}))
        .await;
    let agent_id = text_field(&fixture.last(), "agent_id");
    let onboarding_path = fixture.memory_root(&agent_id).join("system/onboarding.md");
    let onboarding = std::fs::read_to_string(&onboarding_path).expect("onboarding file");
    assert!(
        onboarding.contains("The person you are working with is new to Letta Code."),
        "the onboarding block renders the local onboarding asset"
    );
}

#[tokio::test]
async fn compaction_settings_persist_through_creation() {
    let fixture = bridge();
    // Creation persists a validated record verbatim.
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "cmp-1",
            "body": {
                "name": "Compacted",
                "compaction_settings": {"mode": "sliding_window"},
            },
        }))
        .await;
    let created = fixture.last()["agent"]["id"]
        .as_str()
        .expect("created id")
        .to_owned();
    assert_eq!(
        fixture.last()["agent"]["compaction_settings"],
        json!({"mode": "sliding_window"}),
        "creation returns the persisted record"
    );

    // An unrelated field replacement leaves the record untouched, and an
    // explicit null clears it.
    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "cmp-3",
            "agent_id": created,
            "body": {"name": "Renamed Only"},
        }))
        .await;
    assert_eq!(
        fixture.last()["agent"]["compaction_settings"],
        json!({"mode": "sliding_window"})
    );
    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "cmp-4",
            "agent_id": created,
            "body": {"compaction_settings": null},
        }))
        .await;
    assert_eq!(fixture.last()["agent"]["compaction_settings"], json!(null));
}

#[tokio::test]
async fn compaction_settings_follow_pinned_storage_coercion() {
    let fixture = bridge();
    let created = fixture.create_agent("Replaced Record").await;
    // Update replaces when the record carries a local key.
    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "cmp-2",
            "agent_id": created,
            "body": {"compaction_settings": {"mode": "all"}},
        }))
        .await;
    assert_eq!(
        fixture.last()["agent"]["compaction_settings"],
        json!({"mode": "all"})
    );

    // A record without a local key reads as absent at creation...
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "cmp-5",
            "body": {
                "name": "Cleared Twice",
                "compaction_settings": {},
            },
        }))
        .await;
    let second = fixture.last()["agent"]["id"]
        .as_str()
        .expect("second id")
        .to_owned();
    assert!(
        fixture.last()["agent"].get("compaction_settings").is_none(),
        "a record without local keys does not persist at creation"
    );

    // ...and leaves the stored value untouched on update.
    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "cmp-6",
            "agent_id": second,
            "body": {"compaction_settings": {"unrelated": true}},
        }))
        .await;
    assert!(
        fixture.last()["agent"].get("compaction_settings").is_none(),
        "a record without local keys does not replace on update"
    );

    // A non-object value is treated as undefined, not a rejection.
    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "cmp-7",
            "agent_id": second,
            "body": {"compaction_settings": "sliding_window"},
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert!(
        fixture.last()["agent"].get("compaction_settings").is_none(),
        "a non-object compaction value is ignored"
    );

    // An explicit null clears the stored settings.
    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "cmp-8",
            "agent_id": second,
            "body": {"compaction_settings": null},
        }))
        .await;
    assert_eq!(fixture.last()["agent"]["compaction_settings"], json!(null));

    // A record carrying a local key persists completely, extra fields
    // included.
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "cmp-9",
            "body": {
                "name": "Complete Record",
                "compaction_settings": {"mode": "sliding_window", "prompt": null},
            },
        }))
        .await;
    assert_eq!(
        fixture.last()["agent"]["compaction_settings"],
        json!({"mode": "sliding_window", "prompt": null}),
        "the complete sent object persists when a local key is present"
    );
}
