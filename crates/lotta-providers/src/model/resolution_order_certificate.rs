use super::test_support::*;

fn resolve(
    request: &ModelOverride,
    conversation: &ModelOverride,
    agent: &ModelOverride,
) -> ResolvedModel {
    resolve_model(
        request,
        conversation,
        agent,
        &level(Some("default"), Some(&json!({"v":0}))),
    )
    .unwrap_or_else(|error| panic!("resolve: {error}"))
}
#[test]
fn request_wins() {
    assert_eq!(
        resolve(
            &level(Some("request"), None),
            &level(Some("conversation"), None),
            &level(Some("agent"), None)
        )
        .handle,
        handle("request")
    );
}
#[test]
fn conversation_wins() {
    assert_eq!(
        resolve(
            &level(None, None),
            &level(Some("conversation"), None),
            &level(Some("agent"), None)
        )
        .handle,
        handle("conversation")
    );
}
#[test]
fn agent_wins() {
    assert_eq!(
        resolve(
            &level(None, None),
            &level(None, None),
            &level(Some("agent"), None)
        )
        .handle,
        handle("agent")
    );
}
#[test]
fn default_wins() {
    assert_eq!(
        resolve(&level(None, None), &level(None, None), &level(None, None)).handle,
        handle("default")
    );
}
#[test]
fn settings_only_level_merges() {
    let resolved = resolve(
        &level(None, Some(&json!({"top":true}))),
        &level(Some("conversation"), Some(&json!({"base":1}))),
        &level(Some("agent"), Some(&json!({"inherited":true}))),
    );
    assert_eq!(resolved.handle, handle("conversation"));
    assert_eq!(resolved.settings.get("base"), Some(&Value::from(1)));
    assert_eq!(resolved.settings.get("top"), Some(&Value::Bool(true)));
    assert_eq!(resolved.settings.get("inherited"), Some(&Value::Bool(true)));
    assert_eq!(resolved.settings_source, SettingsSource::MergedByPrecedence);
}
