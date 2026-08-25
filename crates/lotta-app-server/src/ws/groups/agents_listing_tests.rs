//! `ws::agents::listing` — repeated list calls return identical order and
//! each filter (name, query, tag, hidden) narrows through the Task 28 query
//! behaviors.

use serde_json::{Value, json};

use super::support::{SeedAgent, bridge, seed_agent_record};

/// Runs one list command and returns the ordered identifier column.
async fn listed_ids(fixture: &super::support::TestAgents, query: Value) -> Vec<String> {
    let mut command = json!({"type": "agent_list", "request_id": "listing"});
    if !query.is_null() {
        command["query"] = query;
    }
    fixture.send(&command).await;
    assert_eq!(fixture.last()["success"], true, "list succeeded");
    fixture.last()["agents"]
        .as_array()
        .expect("agent array")
        .iter()
        .map(|agent| agent["id"].as_str().expect("id").to_owned())
        .collect()
}

/// Seeds the shared three-agent population in scrambled insertion order.
fn seed_population(fixture: &super::support::TestAgents) {
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "c3-zulu",
            name: "zulu",
            tags: &["shared"],
            hidden: false,
            description: "Trailing entry.",
            model: "model-base",
        },
    );
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "alpha-1",
            name: "Archive Helper",
            tags: &["shared", "extra"],
            hidden: false,
            description: "Retrieves archived documents on demand.",
            model: "model-base",
        },
    );
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "mid-2",
            name: "mike",
            tags: &["shared"],
            hidden: false,
            description: "Plain assistant.",
            model: "model-gpt-9",
        },
    );
}

#[tokio::test]
async fn repeated_lists_return_identical_deterministic_order() {
    let fixture = bridge();
    seed_population(&fixture);
    let first = listed_ids(&fixture, json!(null)).await;
    let second = listed_ids(&fixture, json!(null)).await;
    assert_eq!(first.len(), 3);
    assert_eq!(first, second, "identical order across repeated calls");
    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(first, sorted, "the Task 28 ordering is ascending by id");
}

#[tokio::test]
async fn name_filter_narrows_case_insensitively() {
    let fixture = bridge();
    seed_population(&fixture);
    let ids = listed_ids(&fixture, json!({"name": "archive"})).await;
    assert_eq!(
        ids,
        vec!["agent-local-alpha-1".to_string()],
        "substring name match ignores case"
    );
}

#[tokio::test]
async fn query_filter_matches_description_id_and_model_text() {
    let fixture = bridge();
    seed_population(&fixture);
    // Matches through the description field only.
    let by_description = listed_ids(&fixture, json!({"query_text": "archived documents"})).await;
    assert_eq!(by_description, vec!["agent-local-alpha-1"]);
    // Matches through the model field only.
    let by_model = listed_ids(&fixture, json!({"query_text": "gpt-9"})).await;
    assert_eq!(by_model, vec!["agent-local-mid-2"]);
}

#[tokio::test]
async fn tag_filter_requires_every_requested_tag() {
    let fixture = bridge();
    seed_population(&fixture);
    let all_shared = listed_ids(&fixture, json!({"tags": ["shared"]})).await;
    assert_eq!(all_shared.len(), 3);
    let both = listed_ids(&fixture, json!({"tags": ["shared", "extra"]})).await;
    assert_eq!(
        both,
        vec!["agent-local-alpha-1"],
        "only agents carrying every required tag remain"
    );
}

#[tokio::test]
async fn hidden_filter_toggles_hidden_visibility() {
    let fixture = bridge();
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "ghost-1",
            name: "ghost",
            tags: &[],
            hidden: true,
            description: "Hidden from default listings.",
            model: "model-base",
        },
    );
    seed_agent_record(
        &fixture.root,
        SeedAgent {
            suffix: "open-1",
            name: "open",
            tags: &[],
            hidden: false,
            description: "Visible.",
            model: "model-base",
        },
    );

    let visible = listed_ids(&fixture, json!(null)).await;
    assert_eq!(
        visible,
        vec!["agent-local-open-1"],
        "hidden excluded by default"
    );
    let explicit_visible = listed_ids(&fixture, json!({"hidden": false})).await;
    assert_eq!(explicit_visible, vec!["agent-local-open-1"]);
    let only_hidden = listed_ids(&fixture, json!({"hidden": true})).await;
    assert_eq!(only_hidden, vec!["agent-local-ghost-1"]);
}

#[tokio::test]
async fn after_cursor_continues_the_page() {
    let fixture = bridge();
    seed_population(&fixture);
    let first = listed_ids(&fixture, json!({"limit": 2})).await;
    assert_eq!(first.len(), 2, "the limit bounds the page");
    let continuation = listed_ids(
        &fixture,
        json!({"limit": 10, "after": first.last().expect("nonempty")}),
    )
    .await;
    let mut expected: Vec<String> = Vec::new();
    // Deterministic ascending order means the remainder is everything after
    // the served cursor.
    let mut all = listed_ids(&fixture, json!(null)).await;
    all.sort();
    for id in all {
        if id > *first.last().expect("nonempty") {
            expected.push(id);
        }
    }
    assert_eq!(continuation, expected, "the after cursor resumes the order");

    // An unknown or absent cursor restarts from the first page like the
    // pinned slice semantics.
    let restarted = listed_ids(
        &fixture,
        json!({"limit": 10, "after": "agent-local-does-not-exist"}),
    )
    .await;
    assert_eq!(
        restarted,
        listed_ids(&fixture, json!(null)).await,
        "an unknown cursor serves the first page again"
    );
}
