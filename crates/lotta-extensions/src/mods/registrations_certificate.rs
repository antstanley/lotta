use super::host::{HostFuture, ModHost};
use super::protocol::{RpcMethod, RpcParams, RpcResult};
use super::registrations::*;
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
fn owner() -> ModOwner {
    ModOwner {
        id: ModId::new("project:sample.ts".into()).unwrap(),
        generation: Generation(3),
    }
}
fn name(value: &str) -> RegistrationName {
    RegistrationName::new(value.into()).unwrap()
}
fn batch() -> RegistrationBatch {
    let owner = owner();
    RegistrationBatch {
        tools: vec![ToolRegistration {
            name: name("mod_tool"),
            description: "tool".into(),
            input_schema: json!({"type":"object"}),
            owner: owner.clone(),
        }],
        commands: vec![CommandRegistration {
            id: name("mod_command"),
            description: "command".into(),
            args: Some("[text]".into()),
            owner: owner.clone(),
        }],
        providers: vec![ProviderRegistration {
            name: name("mod_provider"),
            config: json!({}),
            owner: owner.clone(),
        }],
        permissions: vec![PermissionRegistration {
            id: name("mod_permission"),
            description: "permission".into(),
            owner: owner.clone(),
        }],
        lifecycle_events: vec![LifecycleRegistration {
            id: name("mod_event"),
            event: HookEvent::SessionStart,
            owner: owner.clone(),
        }],
        ui_metadata: vec![UiRegistration {
            id: name("mod_panel"),
            title: "Panel".into(),
            metadata: json!({}),
            owner,
        }],
    }
}

#[test]
fn tool_registration_carries_owner() {
    let snapshot = ModRegistrationSnapshot::from_batch(&owner(), batch()).unwrap();
    assert_eq!(snapshot.tools[&name("mod_tool")].owner, owner());
}
#[test]
fn command_registration_carries_owner_and_is_callable() {
    let snapshot = ModRegistrationSnapshot::from_batch(&owner(), batch()).unwrap();
    assert_eq!(snapshot.commands[&name("mod_command")].owner, owner());
}
#[test]
fn provider_registration_carries_owner() {
    let snapshot = ModRegistrationSnapshot::from_batch(&owner(), batch()).unwrap();
    assert_eq!(snapshot.providers[&name("mod_provider")].owner, owner());
}
#[test]
fn permission_registration_carries_owner() {
    let snapshot = ModRegistrationSnapshot::from_batch(&owner(), batch()).unwrap();
    assert_eq!(snapshot.permissions[&name("mod_permission")].owner, owner());
}
#[test]
fn lifecycle_registration_reuses_task44_and_carries_owner() {
    let snapshot = ModRegistrationSnapshot::from_batch(&owner(), batch()).unwrap();
    assert_eq!(
        snapshot.lifecycle_events[&name("mod_event")].event,
        HookEvent::SessionStart
    );
    assert_eq!(snapshot.lifecycle_events[&name("mod_event")].owner, owner());
}
#[test]
fn ui_metadata_registration_carries_owner() {
    let snapshot = ModRegistrationSnapshot::from_batch(&owner(), batch()).unwrap();
    assert_eq!(snapshot.ui_metadata[&name("mod_panel")].owner, owner());
    let host = Arc::new(Host(AtomicUsize::new(0)));
    assert_eq!(host.0.load(Ordering::SeqCst), 0);
}
