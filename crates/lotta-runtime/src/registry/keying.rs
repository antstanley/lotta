use super::*;
use lotta_domain::NonEmptyString;

fn agent(value: impl Into<String>) -> AgentId {
    AgentId::accept(value).unwrap_or_else(|error| panic!("agent: {error}"))
}

fn conversation(value: u64) -> ConversationId {
    ConversationId::generate(value).unwrap_or_else(|error| panic!("conversation: {error}"))
}

fn scope(agent_value: impl Into<String>, conversation_value: u64) -> RuntimeScope {
    RuntimeScope::new(agent(agent_value), conversation(conversation_value), None)
}

fn create(registry: &mut ListenerRuntime, runtime_scope: &RuntimeScope) -> RuntimeHandle {
    registry
        .get_or_create(runtime_scope)
        .unwrap_or_else(|error| panic!("create runtime: {error}"))
}

#[test]
fn idempotent_create() {
    let mut registry = ListenerRuntime::new();
    let first = create(&mut registry, &scope("agent-a", 1));
    let second = create(&mut registry, &scope("agent-a", 1));
    assert_eq!(first, second);
    assert_eq!(registry.len(), 1);
}

#[test]
fn default_conversation_is_agent_scoped() {
    let mut registry = ListenerRuntime::new();
    let first = RuntimeScope::new(agent("agent-a"), ConversationId::default_for_agent(), None);
    let second = RuntimeScope::new(agent("agent-b"), ConversationId::default_for_agent(), None);
    assert_ne!(
        create(&mut registry, &first),
        create(&mut registry, &second)
    );
    assert_eq!(registry.len(), 2);
}

#[test]
fn rejects_at_runtimes_max() {
    let mut registry = full_registry();
    let result = registry.get_or_create(&scope("overflow", 1));
    assert_eq!(registry.len(), RUNTIMES_MAX.value);
    assert!(
        matches!(result, Err(RuntimeError::LimitExceeded { context }) if
        context == RUNTIMES_MAX.name)
    );
}

#[test]
fn rejects_corrupted_over_capacity_without_harming_existing() {
    let mut registry = full_registry();
    let preserved_scope = scope("agent-0", 1);
    let preserved = registry.lookup(&RuntimeKey::from(&preserved_scope));
    registry.entries.insert(
        RuntimeKey::from(&scope("corrupt", 1)),
        RuntimeEntry {
            generation: 99_999,
            residency: RuntimeResidency::new(TurnStateKind::Command, 0, 0, false, 0),
        },
    );
    let result = registry.get_or_create(&scope("rejected", 1));
    assert!(matches!(result, Err(RuntimeError::LimitExceeded { .. })));
    assert_eq!(
        registry.lookup(&RuntimeKey::from(&preserved_scope)),
        preserved
    );
    assert_eq!(registry.len(), RUNTIMES_MAX.value + 1);
}

#[test]
fn generation_exhaustion_does_not_insert() {
    let mut registry = ListenerRuntime::new();
    registry.next_generation = u64::MAX;
    let result = registry.get_or_create(&scope("agent", 1));
    assert!(matches!(result, Err(RuntimeError::InvalidData { .. })));
    assert!(registry.is_empty());
}

#[test]
fn existing_key_allowed_at_limit() {
    let mut registry = full_registry();
    let runtime_scope = scope("agent-0", 1);
    let existing = registry.lookup(&RuntimeKey::from(&runtime_scope));
    assert_eq!(registry.get_or_create(&runtime_scope).ok(), existing);
}

#[test]
fn acting_user_not_identity() {
    let mut registry = ListenerRuntime::new();
    let base = scope("agent-a", 1);
    let attributed = RuntimeScope::new(
        agent("agent-a"),
        conversation(1),
        Some(NonEmptyString::new("user").unwrap_or_else(|error| panic!("user: {error}"))),
    );
    assert_eq!(
        create(&mut registry, &base),
        create(&mut registry, &attributed)
    );
}

fn full_registry() -> ListenerRuntime {
    let mut registry = ListenerRuntime::new();
    for index in 0..RUNTIMES_MAX.value {
        let _handle = create(&mut registry, &scope(format!("agent-{index}"), 1));
    }
    registry
}
