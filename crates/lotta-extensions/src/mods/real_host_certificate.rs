use super::host::{FramedModHost, ModHost, ModHostLauncher};
use super::launcher::{ModHostSpec, OsModHostLauncher};
use super::protocol::{RpcMethod, RpcParams, RpcResult};
use super::registrations::RegistrationBatch;
use super::types::{Capability, ConversationHandle, Generation, ModId, ModOwner};
use crate::sidecar::SidecarOwnerIdentity;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn required_bun() -> PathBuf {
    let bun = PathBuf::from("/Users/stan/.bun/bin/bun");
    assert!(bun.is_file(), "required Bun runtime is unavailable");
    bun
}

fn owner() -> ModOwner {
    ModOwner {
        id: ModId::new("real-fixture".into()).unwrap(),
        generation: Generation(1),
    }
}

fn sidecar_owner() -> SidecarOwnerIdentity {
    SidecarOwnerIdentity::new("agent-real", "runtime-real", "conversation-real").unwrap()
}

fn fixture(directory: &Path) -> PathBuf {
    let path = directory.join("fixture.ts");
    std::fs::write(
        &path,
        r#"
import fs from "node:fs";
type RegistrationApi = {
  registerTool(value: object): void;
  registerCommand(value: object): void;
  registerProvider(value: object): void;
  registerPermission(value: object): void;
  registerLifecycleEvent(value: object): void;
  registerUiMetadata(value: object): void;
  capability(name: string, operation: string, params: unknown): Promise<unknown>;
};
export default {
  async register(api: RegistrationApi) {
    fs.writeFileSync("fixture-ran", "yes");
    api.registerTool({name:"real_tool",description:"real",input_schema:{type:"object"}});
    api.registerCommand({id:"real_command",description:"real",args:null});
    api.registerProvider({name:"real_provider",config:{}});
    api.registerPermission({id:"real_permission",description:"real"});
    api.registerLifecycleEvent({id:"real_lifecycle",event:"SessionStart"});
    api.registerUiMetadata({id:"real_ui",title:"Real",metadata:{}});
    api.capability("tools", "fixture.initialize", {typed:true as boolean}).catch(() => undefined);
  },
  toolCall(name: string, input: unknown, callId: string) { return {name,input,callId}; },
  commandCall(name, args) { return {name,args}; },
  lifecycleCall(name, payload) { return {name,payload}; },
};
"#,
    )
    .unwrap();
    path
}

#[tokio::test]
async fn actual_bun_typescript_bridge_registers_six_and_correlates_calls() {
    let bun = required_bun();
    let root = std::env::temp_dir().join(format!("lotta-real-host-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut launcher = OsModHostLauncher::new(ModHostSpec {
        runtime_executable: bun,
        mod_entry: fixture(&root),
        cwd: root.clone(),
        cache_root: root.join("cache"),
        owner: owner(),
        sidecar_owner: sidecar_owner(),
        env: BTreeMap::new(),
    })
    .unwrap();
    let child = launcher.launch().await.unwrap();
    let host = FramedModHost::accept(child, owner(), sidecar_owner(), Duration::from_secs(5))
        .await
        .unwrap();
    host.call(
        &owner(),
        RpcMethod::Initialize,
        RpcParams::Initialize {
            owner: owner(),
            capabilities: vec![Capability::Tools],
            conversation_handle: ConversationHandle::new("handle-real".into()).unwrap(),
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let registration = host
        .call(
            &owner(),
            RpcMethod::Register,
            RpcParams::Register {
                owner: owner(),
                registrations: RegistrationBatch::default(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let RpcResult::RegistrationBatch { registrations } = registration else {
        panic!()
    };
    assert_eq!(registrations.tools.len(), 1);
    assert_eq!(registrations.commands.len(), 1);
    assert_eq!(registrations.providers.len(), 1);
    assert_eq!(registrations.permissions.len(), 1);
    assert_eq!(registrations.lifecycle_events.len(), 1);
    assert_eq!(registrations.ui_metadata.len(), 1);
    let result = host
        .call(
            &owner(),
            RpcMethod::ToolCall,
            RpcParams::ToolCall {
                owner: owner(),
                name: "real_tool".into(),
                tool_call_id: "genuine-call-id".into(),
                input: json!({"answer":45}),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        RpcResult::Value {
            value: json!({
                "name":"real_tool", "input":{"answer":45}, "callId":"genuine-call-id"
            })
        }
    );
    host.dispose().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn packaged_bridge_hash_is_exact() {
    use sha2::{Digest, Sha256};
    let bytes = include_bytes!("../../assets/mod-host-bridge.mjs");
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        "ab56f395b5a845777be7005df9728f8a9f21d4c40d8de2436a239a7005f0e5ab"
    );
}
