use super::{resolve, test_support::agent};

#[test]
fn unique_name_advertised() {
    let agents = vec![agent("agent-local-alpha", "friendly", false)];
    assert_eq!(resolve::advertised_ids(&agents), ["friendly"]);
    let found = resolve::resolve(&agents, "friendly").unwrap_or_else(|| panic!("unique name"));
    assert_eq!(found.id.as_str(), "agent-local-alpha");
}

#[test]
fn colliding_name_falls_back_to_id() {
    let agents = vec![
        agent("agent-local-alpha", "shared", false),
        agent("agent-local-beta", "shared", false),
    ];
    assert_eq!(
        resolve::advertised_ids(&agents),
        ["agent-local-alpha", "agent-local-beta"]
    );
    assert!(resolve::resolve(&agents, "shared").is_none());
}

#[test]
fn raw_agent_id_resolves() {
    let agents = vec![agent("agent-local-alpha", "friendly", false)];
    let found =
        resolve::resolve(&agents, "agent-local-alpha").unwrap_or_else(|| panic!("raw agent id"));
    assert_eq!(found.name.as_str(), "friendly");
}

#[test]
fn name_cannot_shadow_another_raw_agent_id() {
    let agents = vec![
        agent("agent-local-alpha", "agent-local-beta", false),
        agent("agent-local-beta", "other", false),
    ];
    assert_eq!(
        resolve::advertised_ids(&agents),
        ["agent-local-alpha", "other"]
    );
    let found =
        resolve::resolve(&agents, "agent-local-beta").unwrap_or_else(|| panic!("raw id wins"));
    assert_eq!(found.id.as_str(), "agent-local-beta");
}

#[test]
fn model_id_bound_below() {
    let name = "n".repeat(resolve::OPENAI_MODEL_ID_BYTES_MAX - 1);
    let agents = vec![agent("agent-local-below", &name, false)];
    assert!(resolve::resolve(&agents, &name).is_some());
}

#[test]
fn model_id_bound_at() {
    let name = "n".repeat(resolve::OPENAI_MODEL_ID_BYTES_MAX);
    let agents = vec![agent("agent-local-at", &name, false)];
    assert!(resolve::resolve(&agents, &name).is_some());
}

#[test]
fn model_id_bound_above() {
    let model = "n".repeat(resolve::OPENAI_MODEL_ID_BYTES_MAX + 1);
    let agents = vec![agent("agent-local-above", "short", false)];
    assert!(resolve::resolve(&agents, &model).is_none());
}
