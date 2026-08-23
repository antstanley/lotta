//! `ws::agents::errors` — absent and wrong-prefix lookups answer the same
//! 404-class failure, and `AGENTS_MAX` rejects creation at and above the cap
//! while admitting creation below it.

use serde_json::json;

use super::support::{bridge, bridge_with_limit, seed_agent_record};

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
    seed_agent_record(&fixture.root, "seed-1", "Seed One", &[], false, "", "m");
    seed_agent_record(&fixture.root, "seed-2", "Seed Two", &[], false, "", "m");
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
    seed_agent_record(&fixture.root, "seed-1", "Seed One", &[], false, "", "m");
    seed_agent_record(&fixture.root, "seed-2", "Seed Two", &[], false, "", "m");
    seed_agent_record(&fixture.root, "seed-3", "Seed Three", &[], false, "", "m");
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
    seed_agent_record(&fixture.root, "seed-1", "Seed One", &[], false, "", "m");
    seed_agent_record(&fixture.root, "seed-2", "Seed Two", &[], false, "", "m");
    seed_agent_record(&fixture.root, "seed-3", "Seed Three", &[], false, "", "m");
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
