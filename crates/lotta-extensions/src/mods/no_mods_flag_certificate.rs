use super::host::{HostFuture, ModHost};
use super::protocol::{RpcMethod, RpcParams, RpcResult};
use super::registrations::*;
use super::registry::{ModPublication, ModRegistries};
use super::types::*;
use lotta_runtime::ports::*;
use lotta_tools::pipeline::{
    ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
};
use lotta_tools::registry::{ToolRegistration as RuntimeToolRegistration, ToolRegistry};
use lotta_tools::toolset::ToolsetId;
use serde_json::json;
use std::{future, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

struct Stub;
impl ToolExecutor for Stub {
    fn execute(&self, _: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        Box::pin(future::pending::<
            Result<RawToolOutcome, lotta_tools::pipeline::ExecutorError>,
        >())
    }
}
struct Host;
impl ModHost for Host {
    fn call(
        &self,
        _: &ModOwner,
        _: RpcMethod,
        _: RpcParams,
        _: CancellationToken,
    ) -> HostFuture<'_> {
        Box::pin(async { Ok(RpcResult::Registered) })
    }
    fn dispose(&self) -> super::host::HostFuture<'_> {
        Box::pin(async { Ok(RpcResult::Disposed) })
    }
    fn abort(&self) -> super::host::HostFuture<'_> {
        Box::pin(async { Ok(RpcResult::Disposed) })
    }
}
fn runtime_tool(name: &str, owner: ToolExecutionOwner) -> RuntimeToolRegistration {
    let definition = ToolDefinition::new(
        InternalToolName::new(name.into()).unwrap(),
        ModelFacingToolName::new(name.into()).unwrap(),
        serde_json::from_value(json!({"type":"object"})).unwrap(),
        ToolDescriptionAsset::new(String::new()).unwrap(),
        owner,
        ToolApprovalPolicy::Never,
        PermissionAction::new("execute".into()).unwrap(),
        ToolTimeout::new(Duration::from_secs(5)).unwrap(),
        ToolOutputLimit::new(1_048_576, 32_000).unwrap(),
        serde_json::from_value(json!({"fields":[],"policy":"redact"})).unwrap(),
    );
    RuntimeToolRegistration {
        definition: Arc::new(definition),
        executor: Arc::new(Stub),
    }
}

#[test]
fn no_mods_removes_all_mod_owned_and_preserves_other_origins() {
    let tools = Arc::new(ToolRegistry::new([]).unwrap());
    tools
        .update(
            ToolsetId::None,
            &[
                runtime_tool("native", ToolExecutionOwner::Rust),
                runtime_tool("external", ToolExecutionOwner::Controller),
                runtime_tool("mcp", ToolExecutionOwner::Mcp),
            ],
            None,
        )
        .unwrap();
    let registries = ModRegistries::new(tools.clone());
    let owner = ModOwner {
        id: ModId::new("project:no.ts".into()).unwrap(),
        generation: Generation(1),
    };
    let batch = RegistrationBatch {
        tools: vec![ToolRegistration {
            name: RegistrationName::new("mod".into()).unwrap(),
            description: "mod".into(),
            input_schema: json!({"type":"object"}),
            owner: owner.clone(),
        }],
        ..RegistrationBatch::default()
    };
    let snapshot = Arc::new(ModRegistrationSnapshot::from_batch(&owner, batch).unwrap());
    registries
        .commit(&[ModPublication {
            owner: owner.clone(),
            registrations: snapshot,
            host: Arc::new(Host),
        }])
        .unwrap();
    registries.clear_mods().unwrap();
    let snapshot = tools.snapshot().unwrap();
    assert_eq!(snapshot.len(), 3);
    assert!(snapshot.by_internal("native").is_some());
    assert!(snapshot.by_internal("external").is_some());
    assert!(snapshot.by_internal("mcp").is_some());
    assert!(registries.snapshot().unwrap().is_empty());
}
