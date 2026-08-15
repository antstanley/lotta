use super::*;

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent").unwrap_or_else(|error| panic!("agent: {error}")),
        ConversationId::default_for_agent(),
        None,
    )
}

fn quiescent() -> RuntimeResidency {
    RuntimeResidency::new(TurnStateKind::Idle, 0, 0, false, 0)
}

fn create(registry: &mut ListenerRuntime) -> RuntimeHandle {
    registry
        .get_or_create(&scope())
        .unwrap_or_else(|error| panic!("create runtime: {error}"))
}

#[test]
fn evicts_immediately_when_quiescent() {
    let mut registry = ListenerRuntime::new();
    let handle = create(&mut registry);
    assert_eq!(
        registry.set_residency(&handle, quiescent()),
        Ok(ResidencyUpdate::Evicted)
    );
    assert!(registry.is_empty());
}

#[test]
fn stays_resident_while_lifecycle() {
    assert_one_hot(RuntimeResidency::new(TurnStateKind::Active, 0, 0, false, 0));
}

#[test]
fn stays_resident_while_queue() {
    assert_one_hot(RuntimeResidency::new(TurnStateKind::Idle, 1, 0, false, 0));
}

#[test]
fn stays_resident_while_approval() {
    assert_one_hot(RuntimeResidency::new(TurnStateKind::Idle, 0, 1, false, 0));
}

#[test]
fn stays_resident_while_interrupted_result() {
    assert_one_hot(RuntimeResidency::new(TurnStateKind::Idle, 0, 0, true, 0));
}

#[test]
fn stays_resident_while_sandbox_subscription() {
    assert_one_hot(RuntimeResidency::new(TurnStateKind::Idle, 0, 0, false, 1));
}

#[test]
fn stale_handle_cannot_mutate_recreated_scope() {
    let mut registry = ListenerRuntime::new();
    let stale = create(&mut registry);
    let _evicted = registry.set_residency(&stale, quiescent());
    let current = create(&mut registry);
    assert_ne!(stale, current);
    assert!(registry.set_residency(&stale, quiescent()).is_err());
    assert_eq!(registry.lookup(&current.key), Some(current));
}

#[test]
fn no_runtime_idle_timer() {
    let source = include_str!("../registry.rs");
    assert!(!source.contains("IDLE_EVICT"));
    assert!(!source.contains("Duration"));
}

fn assert_one_hot(snapshot: RuntimeResidency) {
    let mut registry = ListenerRuntime::new();
    let handle = create(&mut registry);
    assert!(snapshot.requires_residency());
    assert_eq!(
        registry.set_residency(&handle, snapshot),
        Ok(ResidencyUpdate::Retained)
    );
    assert_eq!(registry.len(), 1);
}
