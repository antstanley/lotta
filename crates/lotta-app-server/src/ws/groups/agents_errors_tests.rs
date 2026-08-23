//! `ws::agents::errors` — absent and wrong-prefix lookups answer the same
//! 404-class failure, and `AGENTS_MAX` rejects creation at and above the cap
//! while admitting creation below it.

use serde_json::json;

use super::support::{SeedAgent, bridge, bridge_with_limit, seed_agent_record};

#[tokio::test]
async fn retrieve_update_delete_absent_agent_answer_404_class() {
    let fixture = bridge();
    let absent = "agent-local-00000000-0000-4000-8000-000000000000";
    fixture
        .send(&json!({
            "type": "agent_retrieve",
            "request_id": "err-1",
            "agent_id": absent,
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_retrieve_response");
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(fixture.last()["agent"], json!(null));
    assert_eq!(fixture.last()["error"], "agent not found");

    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "err-2",
            "agent_id": absent,
            "body": {"name": "Ignored"},
        }))
        .await;
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(fixture.last()["error"], "agent not found");

    fixture
        .send(&json!({"type": "agent_delete", "request_id": "err-3", "agent_id": absent}))
        .await;
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(fixture.last()["error"], "agent not found");
}

#[tokio::test]
async fn wrong_local_prefix_answers_the_same_404_class() {
    let fixture = bridge();
    // A valid-looking but non-local identifier must never resolve.
    for foreign in ["conv-12345", "agent-local-", "../escape"] {
        fixture
            .send(&json!({
                "type": "agent_retrieve",
                "request_id": format!("prefix-{foreign}"),
                "agent_id": foreign,
            }))
            .await;
        assert_eq!(fixture.last()["success"], false, "{foreign} rejected");
        assert_eq!(fixture.last()["agent"], json!(null));
        assert_eq!(
            fixture.last()["error"],
            "agent not found",
            "one safe 404-class detail"
        );
    }
}

#[tokio::test]
async fn creation_below_the_cap_succeeds() {
    let fixture = bridge_with_limit(3);
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "seed-1",
            name: "Seed One",
            tags: &[],
            hidden: false,
            description: "",
            model: "m",
        },
    );
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "seed-2",
            name: "Seed Two",
            tags: &[],
            hidden: false,
            description: "",
            model: "m",
        },
    );
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "cap-1",
            "body": {"name": "Third Agent"},
        }))
        .await;
    assert_eq!(fixture.last()["success"], true, "two of three slots used");
}

#[tokio::test]
async fn creation_at_the_cap_is_rejected() {
    let fixture = bridge_with_limit(3);
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "seed-1",
            name: "Seed One",
            tags: &[],
            hidden: false,
            description: "",
            model: "m",
        },
    );
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "seed-2",
            name: "Seed Two",
            tags: &[],
            hidden: false,
            description: "",
            model: "m",
        },
    );
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "seed-3",
            name: "Seed Three",
            tags: &[],
            hidden: false,
            description: "",
            model: "m",
        },
    );
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "cap-2",
            "body": {"name": "Fourth Agent"},
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_create_response");
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(fixture.last()["agent"], json!(null));
    assert_eq!(fixture.last()["error"], "agent limit reached");
}

#[tokio::test]
async fn creation_above_the_cap_is_rejected_without_side_effects() {
    let fixture = bridge_with_limit(2);
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "seed-1",
            name: "Seed One",
            tags: &[],
            hidden: false,
            description: "",
            model: "m",
        },
    );
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "seed-2",
            name: "Seed Two",
            tags: &[],
            hidden: false,
            description: "",
            model: "m",
        },
    );
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "seed-3",
            name: "Seed Three",
            tags: &[],
            hidden: false,
            description: "",
            model: "m",
        },
    );
    fixture
        .send(&json!({
            "type": "create_agent",
            "request_id": "cap-3",
            "personality": "blank",
        }))
        .await;
    assert_eq!(fixture.last()["type"], "create_agent_response");
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(fixture.last()["error"], "agent limit reached");
    // No side effect leaked before the rejection.
    let seeded = std::fs::read_dir(fixture.agents_dir())
        .expect("agents dir")
        .count();
    assert_eq!(seeded, 3, "no fourth record was created");
}

#[tokio::test]
async fn shortcut_unknown_model_rejects_with_the_pinned_detail() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "create_agent",
            "request_id": "err-model-1",
            "personality": "memo",
            "model": "no-such-model",
        }))
        .await;
    assert_eq!(fixture.last()["type"], "create_agent_response");
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(fixture.last()["error"], "Unknown model \"no-such-model\"");
}

#[tokio::test]
async fn compaction_settings_with_an_unknown_mode_reject_creation() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "err-cmp-1",
            "body": {
                "name": "Bad Compaction",
                "compaction_settings": {"mode": "explode"},
            },
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_create_response");
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(
        fixture.last()["error"],
        "Local backend compaction currently supports only modes \"all\" and \
         \"sliding_window\" (received \"explode\")."
    );
    assert!(fixture.last()["agent"].is_null());
    // No record leaked before the rejection.
    let seeded = std::fs::read_dir(fixture.agents_dir()).map_or(0, std::iter::Iterator::count);
    assert_eq!(seeded, 0, "creation never touched storage");
}

#[tokio::test]
async fn compaction_settings_with_an_unknown_mode_reject_update() {
    let fixture = bridge();
    let created = fixture.create_agent("Update Target").await;
    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "err-cmp-2",
            "agent_id": created,
            "body": {"compaction_settings": {"mode": 42}},
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_update_response");
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(
        fixture.last()["error"],
        "Local backend compaction currently supports only modes \"all\" and \
         \"sliding_window\" (received \"42\")."
    );
}

#[tokio::test]
async fn compaction_unknown_mode_renders_objects_like_the_baseline_coercion() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "agent_create",
            "request_id": "err-cmp-3",
            "body": {
                "name": "Object Mode",
                "compaction_settings": {"mode": {"kind": "exotic"}},
            },
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_create_response");
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(
        fixture.last()["error"],
        "Local backend compaction currently supports only modes \"all\" and \
         \"sliding_window\" (received \"[object Object]\")."
    );
}

#[tokio::test]
async fn compaction_unknown_mode_renders_arrays_like_the_baseline_coercion() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "agent_update",
            "request_id": "err-cmp-4",
            "agent_id": fixture.create_agent("Array Mode").await,
            "body": {"compaction_settings": {"mode": ["all", 42, null]}},
        }))
        .await;
    assert_eq!(fixture.last()["type"], "agent_update_response");
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(
        fixture.last()["error"],
        "Local backend compaction currently supports only modes \"all\" and \
         \"sliding_window\" (received \"all,42,null\")."
    );
}
