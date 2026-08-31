use serde_json::Value;

use super::test_support::{agent, get, launch, roots, seed};

const VISIBLE_AGENTS_ABOVE_CAP: usize = 1_001;

#[tokio::test(flavor = "multi_thread")]
async fn more_than_1000_is_capped_and_collision_uses_complete_visible_set() {
    let roots = roots("listing-cap");
    let mut agents = Vec::with_capacity(VISIBLE_AGENTS_ABOVE_CAP);
    for index in 0..VISIBLE_AGENTS_ABOVE_CAP {
        let id = format!("agent-local-{index:04}");
        let name = if index == 0 || index == 1_000 {
            "outside-page-collision".to_owned()
        } else {
            format!("name-{index:04}")
        };
        agents.push(agent(&id, &name, false));
    }
    seed(&roots.storage, &agents).await;
    let handle = launch(&roots, true, None).await;
    let response = get(&handle, "/v1/models", "").await;
    assert_eq!(response.status, 200);
    let body: Value =
        serde_json::from_str(&response.body).unwrap_or_else(|error| panic!("models JSON: {error}"));
    let data = body["data"]
        .as_array()
        .unwrap_or_else(|| panic!("model data"));
    assert_eq!(data.len(), 1_000);
    assert_eq!(data[0]["id"], "agent-local-0000");
    assert!(data.iter().all(|model| model["object"] == "model"));
    handle
        .wait()
        .await
        .unwrap_or_else(|error| panic!("stop: {error}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_excluded() {
    let roots = roots("hidden");
    let agents = [
        agent("agent-local-visible", "visible", false),
        agent("agent-local-hidden", "hidden-secret", true),
    ];
    seed(&roots.storage, &agents).await;
    let handle = launch(&roots, true, None).await;
    let response = get(&handle, "/v1/models", "").await;
    assert_eq!(response.status, 200);
    let body: Value =
        serde_json::from_str(&response.body).unwrap_or_else(|error| panic!("models JSON: {error}"));
    assert_eq!(body["data"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["data"][0]["id"], "visible");
    assert!(!response.body.contains("hidden-secret"));
    handle
        .wait()
        .await
        .unwrap_or_else(|error| panic!("stop: {error}"));
}
