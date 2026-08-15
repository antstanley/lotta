use super::*;
use lotta_domain::{RunId, TurnStateKind};

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent").unwrap_or_else(|error| panic!("agent: {error}")),
        ConversationId::default_for_agent(),
        None,
    )
}

fn quiescent() -> RuntimeResidency {
    RuntimeResidency::new(0, 0, false, 0)
}

fn create(registry: &mut ListenerRuntime) -> RuntimeHandle {
    let owner_id = uuid::Uuid::from_u128(u128::from(registry.next_generation));
    registry
        .get_or_create(&scope(), owner_id)
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
    let mut registry = ListenerRuntime::new();
    let handle = create(&mut registry);
    let run = RunId::generate_sequence(1).unwrap_or_else(|error| panic!("run: {error}"));
    registry
        .lifecycle_mut(&handle)
        .unwrap_or_else(|error| panic!("owner: {error}"))
        .begin_turn("turn".into(), run)
        .unwrap_or_else(|error| panic!("turn: {error}"));
    assert_one_hot(registry, &handle, quiescent());
}

#[test]
fn stays_resident_while_queue() {
    assert_auxiliary(RuntimeResidency::new(1, 0, false, 0));
}
#[test]
fn stays_resident_while_approval() {
    assert_auxiliary(RuntimeResidency::new(0, 1, false, 0));
}
#[test]
fn stays_resident_while_interrupted_result() {
    assert_auxiliary(RuntimeResidency::new(0, 0, true, 0));
}
#[test]
fn stays_resident_while_sandbox_subscription() {
    assert_auxiliary(RuntimeResidency::new(0, 0, false, 1));
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

fn assert_auxiliary(snapshot: RuntimeResidency) {
    let mut registry = ListenerRuntime::new();
    let handle = create(&mut registry);
    assert!(snapshot.requires_residency(TurnStateKind::Idle));
    assert_one_hot(registry, &handle, snapshot);
}

fn assert_one_hot(
    mut registry: ListenerRuntime,
    handle: &RuntimeHandle,
    snapshot: RuntimeResidency,
) {
    assert_eq!(
        registry.set_residency(handle, snapshot),
        Ok(ResidencyUpdate::Retained)
    );
    assert_eq!(registry.len(), 1);
}
