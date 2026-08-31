use super::{resolve, test_support::agent};

#[test]
fn unique_name_advertised() {
    let agents = vec![agent("agent-local-alpha", "friendly", false)];
    assert_eq!(resolve::advertised_ids(&agents), ["friendly"]);
    let found = resolve::resolve(&agents, "friendly").unwrap_or_else(|_| panic!("unique name"));
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
    assert!(resolve::resolve(&agents, "shared").is_err());
}

#[test]
fn raw_agent_id_resolves() {
    let agents = vec![agent("agent-local-alpha", "friendly", false)];
    let found =
        resolve::resolve(&agents, "agent-local-alpha").unwrap_or_else(|_| panic!("raw agent id"));
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
        resolve::resolve(&agents, "agent-local-beta").unwrap_or_else(|_| panic!("raw id wins"));
    assert_eq!(found.id.as_str(), "agent-local-beta");
}

#[test]
fn hidden_raw_id_does_not_resolve() {
    let agents = vec![agent("agent-local-hidden", "secret", true)];
    assert!(resolve::resolve(&agents, "agent-local-hidden").is_err());
}

#[test]
fn hidden_name_does_not_resolve_or_advertise() {
    let agents = vec![agent("agent-local-hidden", "secret", true)];
    assert!(resolve::advertised_ids(&agents).is_empty());
    assert!(resolve::resolve(&agents, "secret").is_err());
}

#[test]
fn hidden_duplicate_does_not_make_visible_name_ambiguous() {
    let agents = vec![
        agent("agent-local-visible", "shared", false),
        agent("agent-local-hidden", "shared", true),
    ];
    assert_eq!(resolve::advertised_ids(&agents), ["shared"]);
    let found = resolve::resolve(&agents, "shared").unwrap_or_else(|_| panic!("visible name"));
    assert_eq!(found.id.as_str(), "agent-local-visible");
}
