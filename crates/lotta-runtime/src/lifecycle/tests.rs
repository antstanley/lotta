use crate::{
    CancellationPolicy, LeaseEffect, LeaseGuard, ListenerRuntime, RuntimeHandle, RuntimeKey,
    RuntimeResidency, SuppressionReason,
};
use lotta_domain::{AgentId, ConversationId};
use std::cell::Cell;
use tokio_util::sync::CancellationToken;

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent-17").unwrap_or_else(|error| panic!("agent: {error}")),
        ConversationId::generate(17).unwrap_or_else(|error| panic!("conversation: {error}")),
        None,
    )
}

fn run(value: u64) -> RunId {
    RunId::generate_sequence(value).unwrap_or_else(|error| panic!("run: {error}"))
}

fn reason(value: &str) -> StopReason {
    StopReason::new(value).unwrap_or_else(|error| panic!("reason: {error}"))
}

fn setup_turn() -> (ListenerRuntime, RuntimeHandle, TurnLease) {
    let mut registry = ListenerRuntime::new();
    let handle = registry
        .get_or_create(&scope(), Uuid::from_u128(17))
        .unwrap_or_else(|error| panic!("runtime: {error}"));
    let lease = registry
        .lifecycle_mut(&handle)
        .unwrap_or_else(|error| panic!("owner: {error}"))
        .begin_turn("turn-1".into(), run(1))
        .unwrap_or_else(|error| panic!("turn: {error}"));
    (registry, handle, lease)
}

#[test]
fn lease_generation_increases() {
    let mut owner = LifecycleOwner::new(scope(), Uuid::from_u128(1));
    let first = owner
        .begin_command()
        .unwrap_or_else(|error| panic!("command: {error}"));
    owner
        .finish_command(&first)
        .unwrap_or_else(|error| panic!("finish: {error}"));
    let second = owner
        .begin_turn("turn".into(), run(1))
        .unwrap_or_else(|error| panic!("turn: {error}"));
    assert_eq!(first.generation(), 1);
    assert_eq!(second.generation(), 2);
}

mod stale_lease_suppression {
    use super::*;

    async fn replacement() -> (ListenerRuntime, RuntimeHandle, LeaseGuard) {
        let (mut registry, handle, predecessor) = setup_turn();
        let guard = LeaseGuard::new(
            handle.clone(),
            predecessor.clone(),
            CancellationToken::new(),
            CancellationPolicy::SuppressWhenCancelled,
        );
        tokio::task::yield_now().await;
        registry
            .lifecycle_mut(&handle)
            .unwrap_or_else(|error| panic!("owner: {error}"))
            .finish_turn(&predecessor, reason("done"))
            .unwrap_or_else(|error| panic!("finish: {error}"));
        registry
            .lifecycle_mut(&handle)
            .unwrap_or_else(|error| panic!("owner: {error}"))
            .begin_turn("turn-2".into(), run(2))
            .unwrap_or_else(|error| panic!("replacement: {error}"));
        (registry, handle, guard)
    }

    #[tokio::test]
    async fn no_persistence_write() {
        let (registry, _, guard) = replacement().await;
        let writes = Cell::new(0);
        assert_eq!(
            guard.apply_after_await(&registry, || writes.set(1)),
            LeaseEffect::Suppressed(SuppressionReason::StaleLease)
        );
        assert_eq!(writes.get(), 0);
    }

    #[tokio::test]
    async fn no_emission() {
        let (registry, _, guard) = replacement().await;
        let tool = Cell::new(0);
        let protocol = Cell::new(0);
        let channel = Cell::new(0);
        let effect = || {
            tool.set(1);
            protocol.set(1);
            channel.set(1);
        };
        assert_eq!(
            guard.apply_after_await(&registry, effect),
            LeaseEffect::Suppressed(SuppressionReason::StaleLease)
        );
        assert_eq!((tool.get(), protocol.get(), channel.get()), (0, 0, 0));
    }

    #[tokio::test]
    async fn does_not_release_newer_turn() {
        let (mut registry, handle, guard) = replacement().await;
        assert_eq!(
            guard.finish_turn_after_await(&mut registry, reason("stale")),
            LeaseEffect::Suppressed(SuppressionReason::StaleLease)
        );
        let projection = registry
            .lifecycle(&handle)
            .unwrap_or_else(|| panic!("owner"))
            .projection();
        assert_eq!(projection.state(), TurnStateKind::Active);
        assert_eq!(projection.active_run_ids(), &[run(2)]);
    }

    #[tokio::test]
    async fn respects_cancellation_policy() {
        let (registry, handle, lease) = setup_turn();
        let token = CancellationToken::new();
        token.cancel();
        let denied = LeaseGuard::new(
            handle.clone(),
            lease.clone(),
            token.clone(),
            CancellationPolicy::SuppressWhenCancelled,
        );
        assert_eq!(
            denied.apply_after_await(&registry, || 1),
            LeaseEffect::Suppressed(SuppressionReason::CancellationDenied)
        );
        let cleanup = LeaseGuard::new(
            handle,
            lease,
            token,
            CancellationPolicy::PermitDuringCancellationCleanup,
        );
        assert_eq!(
            cleanup.apply_after_await(&registry, || 1),
            LeaseEffect::Applied(1)
        );
    }
}

#[test]
fn four_suppression_reasons_are_independent() {
    let (mut inactive, handle, lease) = setup_turn();
    inactive.deactivate();
    assert_reason(
        &inactive,
        handle.clone(),
        lease.clone(),
        SuppressionReason::ListenerInactive,
    );
    let missing = ListenerRuntime::new();
    assert_reason(
        &missing,
        handle.clone(),
        lease.clone(),
        SuppressionReason::RuntimeMissing,
    );
    let (mut stale, stale_handle, stale_lease) = setup_turn();
    stale
        .lifecycle_mut(&stale_handle)
        .unwrap_or_else(|error| panic!("owner: {error}"))
        .finish_turn(&stale_lease, reason("done"))
        .unwrap_or_else(|error| panic!("finish: {error}"));
    assert_reason(
        &stale,
        stale_handle,
        stale_lease,
        SuppressionReason::StaleLease,
    );
}

fn assert_reason(
    registry: &ListenerRuntime,
    handle: RuntimeHandle,
    lease: TurnLease,
    expected: SuppressionReason,
) {
    let guard = LeaseGuard::new(
        handle,
        lease,
        CancellationToken::new(),
        CancellationPolicy::SuppressWhenCancelled,
    );
    assert_eq!(
        guard.apply_after_await(registry, || ()),
        LeaseEffect::Suppressed(expected)
    );
}

#[test]
fn impossible_state_is_invariant_violation() {
    let mut owner = LifecycleOwner::new(scope(), Uuid::from_u128(1));
    let _lease = owner
        .begin_command()
        .unwrap_or_else(|error| panic!("command: {error}"));
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _replacement = owner.begin_turn("impossible".into(), run(1));
    }))
    .unwrap_err();
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic");
    assert_eq!(
        message,
        concat!(
            "lifecycle invariant violation agent=agent-17 ",
            "conversation=local-conv-17 from=Command to=Active",
        )
    );
    assert_eq!(owner.projection().state(), TurnStateKind::Command);
}

mod projections_derive_from_state {
    use super::*;

    #[test]
    fn is_processing() {
        let mut owner = LifecycleOwner::new(scope(), Uuid::from_u128(1));
        assert!(!owner.projection().is_processing());
        let _lease = owner
            .begin_turn("turn".into(), run(1))
            .unwrap_or_else(|error| panic!("turn: {error}"));
        assert!(owner.projection().is_processing());
    }

    #[test]
    fn loop_status() {
        let mut owner = LifecycleOwner::new(scope(), Uuid::from_u128(1));
        let lease = owner
            .begin_turn("turn".into(), run(1))
            .unwrap_or_else(|error| panic!("turn: {error}"));
        assert_eq!(
            owner.projection().loop_status(),
            LoopStatus::SendingApiRequest
        );
        owner
            .request_cancellation(&lease)
            .unwrap_or_else(|error| panic!("cancel: {error}"));
        assert_eq!(owner.projection().loop_status(), LoopStatus::WaitingOnInput);
    }

    #[test]
    fn active_run_ids() {
        let mut owner = LifecycleOwner::new(scope(), Uuid::from_u128(1));
        let lease = owner
            .begin_turn("turn".into(), run(1))
            .unwrap_or_else(|error| panic!("turn: {error}"));
        owner
            .request_cancellation(&lease)
            .unwrap_or_else(|error| panic!("cancel: {error}"));
        assert_eq!(owner.projection().active_run_ids(), &[run(1)]);
    }

    #[test]
    fn last_stop_reason() {
        let mut owner = LifecycleOwner::new(scope(), Uuid::from_u128(1));
        let lease = owner
            .begin_turn("turn".into(), run(1))
            .unwrap_or_else(|error| panic!("turn: {error}"));
        owner
            .finish_turn(&lease, reason("complete"))
            .unwrap_or_else(|error| panic!("finish: {error}"));
        assert_eq!(
            owner
                .projection()
                .last_stop_reason()
                .map(StopReason::as_str),
            Some("complete")
        );
    }

    #[test]
    fn no_duplicate_projection_fields() {
        let source = include_str!("../lifecycle.rs");
        let owner = source
            .split("pub struct LifecycleOwner")
            .nth(1)
            .and_then(|value| value.split('}').next())
            .unwrap_or_else(|| panic!("owner fields"));
        for forbidden in [
            "is_processing",
            "loop_status",
            "active_run_ids",
            "last_stop_reason",
        ] {
            assert!(!owner.contains(forbidden));
        }
        let registry = include_str!("../registry.rs");
        let entry = registry
            .split("struct RuntimeEntry")
            .nth(1)
            .and_then(|value| value.split('}').next())
            .unwrap_or_else(|| panic!("entry fields"));
        assert!(!entry.contains("TurnStateKind"));
    }
}

#[test]
fn source_contract_uses_live_owner_and_four_checks() {
    let source = include_str!("../lease.rs");
    for required in [
        "is_active()",
        "lifecycle(&self.handle)",
        "is_current(&self.lease)",
        "is_cancelled()",
    ] {
        assert!(source.contains(required));
    }
    let registry = include_str!("../registry.rs");
    assert!(registry.contains("owner: LifecycleOwner"));
    assert!(!registry.contains("lifecycle_snapshot: TurnStateKind,"));
    let _unused_types = (
        RuntimeKey::from(&scope()),
        RuntimeResidency::new(0, false, 0),
    );
}
