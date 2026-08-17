use super::host::{HostFuture, ModHost};
use super::protocol::{RpcMethod, RpcParams, RpcResult};
use super::registrations::*;
use super::registry::{ActiveGenerationTable, ModCommandHandle, ModLifecycleHandle};
use super::safe_mode::*;
use super::types::*;
use lotta_runtime::hooks::HookEvent;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

struct Host(AtomicUsize);
impl ModHost for Host {
    fn call(
        &self,
        _: &ModOwner,
        _: RpcMethod,
        _: RpcParams,
        _: CancellationToken,
    ) -> HostFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(RpcResult::Value { value: json!("ok") }) })
    }
    fn dispose(&self) -> super::host::HostFuture<'_> {
        Box::pin(async { Ok(RpcResult::Disposed) })
    }
    fn abort(&self) -> super::host::HostFuture<'_> {
        Box::pin(async { Ok(RpcResult::Disposed) })
    }
}
fn owner(generation: u64) -> ModOwner {
    ModOwner {
        id: ModId::new("project:life.ts".into()).unwrap(),
        generation: Generation(generation),
    }
}
fn command(generation: u64) -> CommandRegistration {
    CommandRegistration {
        id: RegistrationName::new("life".into()).unwrap(),
        description: "life".into(),
        args: None,
        owner: owner(generation),
    }
}
fn event(generation: u64) -> LifecycleRegistration {
    LifecycleRegistration {
        id: RegistrationName::new("event".into()).unwrap(),
        event: HookEvent::SessionStart,
        owner: owner(generation),
    }
}

#[test]
fn load_publishes_immutable_snapshot() {
    let snapshot = Arc::new(
        ModRegistrationSnapshot::from_batch(
            &owner(1),
            RegistrationBatch {
                commands: vec![command(1)],
                ..RegistrationBatch::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(snapshot.len(), 1);
    assert_eq!(
        snapshot.commands.values().next().unwrap().owner.generation,
        Generation(1)
    );
}
#[test]
fn reload_advances_generation() {
    assert_eq!(Generation(1).next().unwrap(), Generation(2));
}
#[test]
fn dispose_removes_all_six_kinds() {
    let snapshot = ModRegistrationSnapshot::from_batch(
        &owner(1),
        RegistrationBatch {
            commands: vec![command(1)],
            ..RegistrationBatch::default()
        },
    )
    .unwrap();
    assert!(snapshot.without_owner("project:life.ts").is_empty());
}
#[test]
fn diagnostics_are_owner_attributed_and_bounded() {
    let diagnostic = SafeModeDiagnostic {
        owner: owner(1),
        reason: "safe_mode",
    };
    assert_eq!(diagnostic.owner, owner(1));
    assert!(diagnostic.reason.len() <= MOD_DIAGNOSTIC_BYTES_MAX);
}
#[test]
fn safe_mode_removes_third_party_and_retains_trusted() {
    let empty = Arc::new(ModRegistrationSnapshot::default());
    let sources = vec![
        ModSource {
            owner: owner(1),
            trust: ModTrust::ThirdParty,
            registrations: empty.clone(),
        },
        ModSource {
            owner: owner(2),
            trust: ModTrust::Trusted,
            registrations: empty,
        },
    ];
    let (retained, diagnostics) = filter_sources(ModsStartupMode::SafeMode, sources).unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(diagnostics.len(), 1);
}
#[tokio::test]
async fn stale_generation_registration_is_invalid() {
    let host = Arc::new(Host(AtomicUsize::new(0)));
    let active = Arc::new(ActiveGenerationTable::activated([owner(2)]));
    let command = ModCommandHandle::new(command(1), active.clone(), host.clone());
    assert_eq!(
        command.call(json!({}), CancellationToken::new()).await,
        Err(ModError::InvalidScope)
    );
    let lifecycle = ModLifecycleHandle::new(event(1), active, host.clone());
    assert_eq!(
        lifecycle.call(json!({}), CancellationToken::new()).await,
        Err(ModError::InvalidScope)
    );
    assert_eq!(host.0.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn current_generation_registration_is_callable() {
    let host = Arc::new(Host(AtomicUsize::new(0)));
    let command = ModCommandHandle::new(
        command(1),
        Arc::new(ActiveGenerationTable::activated([owner(1)])),
        host.clone(),
    );
    assert_eq!(
        command
            .call(json!({}), CancellationToken::new())
            .await
            .unwrap(),
        json!("ok")
    );
    assert_eq!(host.0.load(Ordering::SeqCst), 1);
}
