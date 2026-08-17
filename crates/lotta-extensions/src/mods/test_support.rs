//! Deterministic in-memory compatibility children speaking the real Task 42 framing.
//!
//! Nothing here reimplements framing or the JSON-RPC contract: the child writes the same envelopes
//! the packaged bridge writes, through the same codec the host reads.

use super::capabilities::{CapabilityFuture, CapabilityPort};
use super::controller::ModStartSpec;
use super::host::{KillSwitch, ModHostChild, ModHostLauncher};
use super::protocol::{
    JSON_RPC_VERSION, RpcError, RpcId, RpcMethod, RpcParams, RpcRequest, RpcResponse, RpcResult,
};
use super::registrations::{
    CommandRegistration, LifecycleRegistration, PermissionRegistration, ProviderRegistration,
    RegistrationBatch, ToolRegistration, UiRegistration,
};
use super::safe_mode::{ModTrust, ModsStartupMode};
use super::types::{
    Capability, ConversationHandle, Generation, ModError, ModId, ModOwner, ModRuntimeScope,
    RegistrationName,
};
use crate::sidecar::framing::{read_frame, write_frame};
use crate::sidecar::{
    SIDECAR_PROTOCOL_VERSION, SidecarCapability, SidecarEnvelope, SidecarEnvelopeKind,
    SidecarFrameLimit, SidecarOwnerIdentity,
};
use lotta_domain::{AgentId, ConversationId, RuntimeScope, Timestamp};
use lotta_runtime::hooks::HookEvent;
use lotta_testkit::clock::FakeClock;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::DuplexStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Local pipe capacity used by every scripted child.
const CHILD_PIPE_BYTES: usize = 256 * 1024;
/// First request identity a scripted child uses for its own inbound calls.
const CHILD_REQUEST_ID_FIRST: u64 = 9_001;

pub(super) fn clock() -> Arc<FakeClock> {
    let start = Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z").expect("fixed instant");
    Arc::new(FakeClock::new(start))
}

pub(super) fn owner(id: &str, generation: u64) -> ModOwner {
    ModOwner {
        id: ModId::new(id.into()).expect("bounded mod identity"),
        generation: Generation(generation),
    }
}

pub(super) fn sidecar_owner(label: &str) -> SidecarOwnerIdentity {
    SidecarOwnerIdentity::new(label, "runtime", "conversation").expect("bounded owner identity")
}

pub(super) fn name(value: &str) -> RegistrationName {
    RegistrationName::new(value.into()).expect("bounded registration name")
}

pub(super) fn scope(agent: &str, conversation: &str) -> ModRuntimeScope {
    ModRuntimeScope::new(
        RuntimeScope::new(
            AgentId::accept(agent).expect("bounded agent"),
            ConversationId::accept(conversation).expect("bounded conversation"),
            None,
        ),
        "/workspace".into(),
    )
}

/// Builds one prefixed six-kind batch so several owners can be merged without name collisions.
pub(super) fn batch(owner: &ModOwner, prefix: &str) -> RegistrationBatch {
    RegistrationBatch {
        tools: vec![ToolRegistration {
            name: name(&format!("{prefix}_tool")),
            description: "tool".into(),
            input_schema: json!({"type":"object"}),
            owner: owner.clone(),
        }],
        commands: vec![CommandRegistration {
            id: name(&format!("{prefix}_command")),
            description: "command".into(),
            args: None,
            owner: owner.clone(),
        }],
        providers: vec![ProviderRegistration {
            name: name(&format!("{prefix}_provider")),
            config: json!({}),
            owner: owner.clone(),
        }],
        permissions: vec![PermissionRegistration {
            id: name(&format!("{prefix}_permission")),
            description: "permission".into(),
            owner: owner.clone(),
        }],
        lifecycle_events: vec![LifecycleRegistration {
            id: name(&format!("{prefix}_event")),
            event: HookEvent::SessionStart,
            owner: owner.clone(),
        }],
        ui_metadata: vec![UiRegistration {
            id: name(&format!("{prefix}_panel")),
            title: "Panel".into(),
            metadata: json!({}),
            owner: owner.clone(),
        }],
    }
}

pub(super) fn start_spec(owner: ModOwner, trust: ModTrust, caps: &[Capability]) -> ModStartSpec {
    ModStartSpec {
        sidecar_owner: sidecar_owner(owner.id.as_str()),
        capabilities: caps.to_vec(),
        scope: scope("agent-a", "conversation-a"),
        trust,
        owner,
    }
}

pub(super) fn enabled() -> ModsStartupMode {
    ModsStartupMode::Enabled
}

/// Builds one Task 32 registration owned by something other than the mod sidecar.
pub(super) fn runtime_tool(
    label: &str,
    execution_owner: lotta_runtime::ports::ToolExecutionOwner,
) -> lotta_tools::registry::ToolRegistration {
    use lotta_runtime::ports::{
        InternalToolName, ModelFacingToolName, PermissionAction, ToolApprovalPolicy,
        ToolDefinition, ToolDescriptionAsset, ToolOutputLimit, ToolTimeout,
    };
    let definition = ToolDefinition::new(
        InternalToolName::new(label.into()).expect("bounded internal name"),
        ModelFacingToolName::new(label.into()).expect("bounded model name"),
        serde_json::from_value(json!({"type":"object"})).expect("object schema"),
        ToolDescriptionAsset::new(String::new()).expect("bounded description"),
        execution_owner,
        ToolApprovalPolicy::Never,
        PermissionAction::new("execute".into()).expect("bounded action"),
        ToolTimeout::new(std::time::Duration::from_secs(5)).expect("bounded timeout"),
        ToolOutputLimit::new(1_048_576, 32_000).expect("bounded output"),
        serde_json::from_value(json!({"fields":[],"policy":"redact"})).expect("redaction spec"),
    );
    lotta_tools::registry::ToolRegistration {
        definition: Arc::new(definition),
        executor: Arc::new(PendingExecutor),
    }
}

struct PendingExecutor;
impl lotta_tools::pipeline::ToolExecutor for PendingExecutor {
    fn execute(
        &self,
        _: lotta_tools::pipeline::RawToolExecutionRequest,
    ) -> lotta_tools::pipeline::ExecutorFuture<'_> {
        Box::pin(std::future::pending::<
            Result<lotta_tools::pipeline::RawToolOutcome, lotta_tools::pipeline::ExecutorError>,
        >())
    }
}

/// Capability port recording every authorized call it received.
pub(super) struct RecordingPort {
    pub calls: Mutex<Vec<(ModOwner, Capability, String)>>,
    pub scopes: Mutex<Vec<String>>,
}
impl RecordingPort {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            scopes: Mutex::new(Vec::new()),
        })
    }
    pub(super) fn observed(&self) -> Vec<(ModOwner, Capability, String)> {
        self.calls.lock().expect("port lock").clone()
    }
    pub(super) fn observed_scopes(&self) -> Vec<String> {
        self.scopes.lock().expect("port lock").clone()
    }
}
impl CapabilityPort for RecordingPort {
    fn call(
        &self,
        owner: &ModOwner,
        scope: &ModRuntimeScope,
        capability: Capability,
        operation: &str,
        params: Value,
        _: CancellationToken,
    ) -> CapabilityFuture<'_> {
        self.calls.lock().expect("port lock").push((
            owner.clone(),
            capability,
            operation.to_owned(),
        ));
        self.scopes
            .lock()
            .expect("port lock")
            .push(scope.runtime().conversation_id.as_str().to_owned());
        Box::pin(async move { Ok(params) })
    }
}

/// Counters observed by lifecycle assertions.
#[derive(Default)]
pub(super) struct ChildCounters {
    pub stops: AtomicUsize,
    pub joins: AtomicUsize,
    pub kills: AtomicUsize,
}

/// Stateful frame-to-frames reaction implementing one child behavior.
pub(super) type Responder = Box<dyn FnMut(&SidecarEnvelope) -> Vec<SidecarEnvelope> + Send>;

/// Complete script for one scripted child.
pub(super) struct ChildPlan {
    pub greeting: bool,
    pub stop_blocks: bool,
    pub join_blocks: bool,
    pub responder: Responder,
}
impl ChildPlan {
    pub(super) fn new(responder: Responder) -> Self {
        Self {
            greeting: true,
            stop_blocks: false,
            join_blocks: false,
            responder,
        }
    }
    pub(super) fn blocking_join(mut self) -> Self {
        self.join_blocks = true;
        self
    }
}

/// Owned in-memory child whose pipes carry real Task 42 frames.
pub(super) struct ScriptedChild {
    reader: Option<DuplexStream>,
    writer: Option<DuplexStream>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
    counters: Arc<ChildCounters>,
    stop_blocks: bool,
    join_blocks: bool,
}
impl ScriptedChild {
    pub(super) fn new(plan: ChildPlan, owner: SidecarOwnerIdentity) -> Self {
        let (host_reader, child_writer) = tokio::io::duplex(CHILD_PIPE_BYTES);
        let (host_writer, child_reader) = tokio::io::duplex(CHILD_PIPE_BYTES);
        let stop_blocks = plan.stop_blocks;
        let join_blocks = plan.join_blocks;
        let task = tokio::spawn(drive(child_reader, child_writer, plan, owner));
        Self {
            reader: Some(host_reader),
            writer: Some(host_writer),
            task: Arc::new(Mutex::new(Some(task))),
            counters: Arc::new(ChildCounters::default()),
            stop_blocks,
            join_blocks,
        }
    }
    pub(super) fn counters(&self) -> Arc<ChildCounters> {
        Arc::clone(&self.counters)
    }
}

async fn drive(
    mut reader: DuplexStream,
    mut writer: DuplexStream,
    plan: ChildPlan,
    owner: SidecarOwnerIdentity,
) {
    let limit = SidecarFrameLimit::mod_host();
    if plan.greeting {
        let hello = envelope(&owner, SidecarEnvelopeKind::Hello, "hello", None, json!({}));
        if write_frame(&mut writer, limit, &hello).await.is_err() {
            return;
        }
    }
    let mut responder = plan.responder;
    loop {
        let Ok(inbound) = read_frame::<_, SidecarEnvelope>(&mut reader, limit).await else {
            return;
        };
        for outbound in responder(&inbound) {
            if write_frame(&mut writer, limit, &outbound).await.is_err() {
                return;
            }
        }
    }
}

impl ModHostChild for ScriptedChild {
    type Reader = DuplexStream;
    type Writer = DuplexStream;
    type Stop<'a> = Pin<Box<dyn Future<Output = Result<(), ModError>> + Send + 'a>>;
    type Join<'a> = Pin<Box<dyn Future<Output = Result<(), ModError>> + Send + 'a>>;

    fn take_pipes(&mut self) -> Result<(Self::Reader, Self::Writer), ModError> {
        let reader = self.reader.take().ok_or(ModError::Unavailable)?;
        let writer = self.writer.take().ok_or(ModError::Unavailable)?;
        Ok((reader, writer))
    }

    fn stop(&mut self) -> Self::Stop<'_> {
        Box::pin(async move {
            self.counters.stops.fetch_add(1, Ordering::SeqCst);
            if self.stop_blocks {
                std::future::pending::<()>().await;
            }
            abort_task(&self.task);
            Ok(())
        })
    }

    fn join(&mut self) -> Self::Join<'_> {
        Box::pin(async move {
            self.counters.joins.fetch_add(1, Ordering::SeqCst);
            if self.join_blocks {
                std::future::pending::<()>().await;
            }
            let task = self.task.lock().ok().and_then(|mut guard| guard.take());
            if let Some(task) = task {
                let _ = task.await;
            }
            Ok(())
        })
    }

    fn kill_switch(&self) -> KillSwitch {
        let counters = Arc::clone(&self.counters);
        let task = Arc::clone(&self.task);
        KillSwitch::new(move || {
            counters.kills.fetch_add(1, Ordering::SeqCst);
            abort_task(&task);
        })
    }
}

fn abort_task(task: &Arc<Mutex<Option<JoinHandle<()>>>>) {
    if let Ok(guard) = task.lock()
        && let Some(handle) = guard.as_ref()
    {
        handle.abort();
    }
}

/// Launcher handing out pre-scripted children and counting every launch.
pub(super) struct ScriptedLauncher {
    plans: VecDeque<ChildPlan>,
    owner: SidecarOwnerIdentity,
    launched: Arc<AtomicUsize>,
    counters: Arc<Mutex<Vec<Arc<ChildCounters>>>>,
}
impl ScriptedLauncher {
    pub(super) fn new(owner: SidecarOwnerIdentity, plans: Vec<ChildPlan>) -> Self {
        Self {
            plans: plans.into(),
            owner,
            launched: Arc::new(AtomicUsize::new(0)),
            counters: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub(super) fn launched(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.launched)
    }
    /// Shares the growing counter list so a caller can observe children launched later.
    pub(super) fn counters(&self) -> Arc<Mutex<Vec<Arc<ChildCounters>>>> {
        Arc::clone(&self.counters)
    }
}
impl ModHostLauncher for ScriptedLauncher {
    type Child = ScriptedChild;
    type Launch<'a> = Pin<Box<dyn Future<Output = Result<Self::Child, ModError>> + Send + 'a>>;

    fn launch(&mut self) -> Self::Launch<'_> {
        Box::pin(async move {
            let plan = self.plans.pop_front().ok_or(ModError::Unavailable)?;
            let child = ScriptedChild::new(plan, self.owner.clone());
            self.launched.fetch_add(1, Ordering::SeqCst);
            self.counters
                .lock()
                .map_err(|_| ModError::Unavailable)?
                .push(child.counters());
            Ok(child)
        })
    }
}

pub(super) fn envelope(
    owner: &SidecarOwnerIdentity,
    kind: SidecarEnvelopeKind,
    request_id: &str,
    correlation_id: Option<String>,
    payload: Value,
) -> SidecarEnvelope {
    SidecarEnvelope {
        version: SIDECAR_PROTOCOL_VERSION,
        owner: owner.clone(),
        capability: SidecarCapability::Mod,
        timeout_ms: 30_000,
        request_id: request_id.into(),
        correlation_id,
        kind,
        payload,
    }
}

pub(super) fn response(
    owner: &SidecarOwnerIdentity,
    id: RpcId,
    result: RpcResult,
) -> SidecarEnvelope {
    let payload = RpcResponse {
        jsonrpc: JSON_RPC_VERSION.into(),
        id,
        result: Some(result),
        error: None,
    };
    envelope(
        owner,
        SidecarEnvelopeKind::Response,
        &format!("response-{}", id.value()),
        Some(id.value().to_string()),
        serde_json::to_value(payload).expect("encode response"),
    )
}

pub(super) fn error_response(
    owner: &SidecarOwnerIdentity,
    id: RpcId,
    code: i32,
) -> SidecarEnvelope {
    let payload = RpcResponse {
        jsonrpc: JSON_RPC_VERSION.into(),
        id,
        result: None,
        error: Some(RpcError {
            code,
            message: "mod failed".into(),
        }),
    };
    envelope(
        owner,
        SidecarEnvelopeKind::Response,
        &format!("response-{}", id.value()),
        Some(id.value().to_string()),
        serde_json::to_value(payload).expect("encode response"),
    )
}

pub(super) fn decode(inbound: &SidecarEnvelope) -> Option<RpcRequest> {
    serde_json::from_value::<RpcRequest>(inbound.payload.clone()).ok()
}

/// Host double used where only generation checks, not framing, are under test.
pub(super) struct StubHost {
    pub calls: AtomicUsize,
}
impl StubHost {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
        })
    }
}
impl super::host::ModHost for StubHost {
    fn call(
        &self,
        _: &ModOwner,
        _: RpcMethod,
        _: RpcParams,
        _: CancellationToken,
    ) -> super::host::HostFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(RpcResult::Value { value: json!("ok") }) })
    }
    fn dispose(&self) -> super::host::HostFuture<'_> {
        Box::pin(async { Ok(RpcResult::Disposed) })
    }
    fn abort(&self) -> super::host::HostFuture<'_> {
        Box::pin(async { Ok(RpcResult::Disposed) })
    }
}

/// Defers every tool call, then optionally settles both the cancel and the original response.
pub(super) fn cancel_aware_responder(
    sidecar: SidecarOwnerIdentity,
    registrations: RegistrationBatch,
    settle: bool,
) -> Responder {
    let mut standard = standard_responder(sidecar.clone(), registrations, Vec::new());
    let mut deferred: Option<RpcId> = None;
    Box::new(move |inbound| {
        let Some(request) = decode(inbound) else {
            return Vec::new();
        };
        match request.method {
            RpcMethod::ToolCall => {
                deferred = Some(request.id);
                Vec::new()
            }
            RpcMethod::Cancel if settle => {
                let mut outbound = vec![response(&sidecar, request.id, RpcResult::Cancelled)];
                if let Some(id) = deferred.take() {
                    let value = RpcResult::Value {
                        value: json!("cancelled"),
                    };
                    outbound.push(response(&sidecar, id, value));
                }
                outbound
            }
            RpcMethod::Cancel => Vec::new(),
            _ => standard(inbound),
        }
    })
}

/// Settles a cancellation but interleaves one inbound capability request first.
pub(super) fn cancel_with_inbound_responder(
    owner: ModOwner,
    sidecar: SidecarOwnerIdentity,
    registrations: RegistrationBatch,
    capability: Capability,
) -> Responder {
    let mut standard = standard_responder(sidecar.clone(), registrations, Vec::new());
    let mut deferred: Option<RpcId> = None;
    let mut handle: Option<ConversationHandle> = None;
    let probe_id = RpcId::new(CHILD_REQUEST_ID_FIRST).expect("positive identity");
    Box::new(move |inbound| {
        let Some(request) = decode(inbound) else {
            return Vec::new();
        };
        if let RpcParams::Initialize {
            conversation_handle,
            ..
        } = &request.params
        {
            handle = Some(conversation_handle.clone());
        }
        match request.method {
            RpcMethod::ToolCall => {
                deferred = Some(request.id);
                Vec::new()
            }
            RpcMethod::Cancel => {
                let mut outbound = Vec::new();
                if let Some(handle) = handle.clone() {
                    outbound.push(capability_request(
                        &sidecar, &owner, probe_id, capability, handle,
                    ));
                }
                outbound.push(response(&sidecar, request.id, RpcResult::Cancelled));
                if let Some(id) = deferred.take() {
                    let value = RpcResult::Value {
                        value: json!("cancelled"),
                    };
                    outbound.push(response(&sidecar, id, value));
                }
                outbound
            }
            _ => standard(inbound),
        }
    })
}

/// Answers a tool call with a frame the strict wire contract must reject.
pub(super) fn malformed_tool_responder(
    sidecar: SidecarOwnerIdentity,
    registrations: RegistrationBatch,
) -> Responder {
    let mut standard = standard_responder(sidecar.clone(), registrations, Vec::new());
    Box::new(move |inbound| {
        let Some(request) = decode(inbound) else {
            return Vec::new();
        };
        if request.method != RpcMethod::ToolCall {
            return standard(inbound);
        }
        vec![envelope(
            &sidecar,
            SidecarEnvelopeKind::Response,
            "response-bogus",
            Some(request.id.value().to_string()),
            json!({"jsonrpc": "2.0", "id": request.id.value(), "surprise": true}),
        )]
    })
}

/// Answers every method the controller drives, with a configurable per-call value.
pub(super) fn standard_responder(
    owner: SidecarOwnerIdentity,
    registrations: RegistrationBatch,
    diagnostics: Vec<String>,
) -> Responder {
    Box::new(move |inbound| {
        let Some(request) = decode(inbound) else {
            return Vec::new();
        };
        let id = request.id;
        match request.method {
            RpcMethod::Initialize => vec![response(&owner, id, RpcResult::Initialized)],
            RpcMethod::Register => vec![response(
                &owner,
                id,
                RpcResult::RegistrationBatch {
                    registrations: registrations.clone(),
                },
            )],
            RpcMethod::Diagnostics => vec![response(
                &owner,
                id,
                RpcResult::Diagnostics {
                    diagnostics: diagnostics.clone(),
                },
            )],
            RpcMethod::Dispose => vec![response(&owner, id, RpcResult::Disposed)],
            RpcMethod::Cancel => vec![response(&owner, id, RpcResult::Cancelled)],
            RpcMethod::ToolCall | RpcMethod::CommandCall | RpcMethod::LifecycleCall => {
                vec![response(
                    &owner,
                    id,
                    RpcResult::Value { value: json!("ok") },
                )]
            }
            RpcMethod::CapabilityCall | RpcMethod::Reload => {
                vec![error_response(&owner, id, -32601)]
            }
        }
    })
}

/// Records the last capability outcome the child observed, for refusal assertions.
#[derive(Default)]
pub(super) struct CapabilityProbe {
    pub outcomes: Mutex<Vec<Result<Value, i32>>>,
}
impl CapabilityProbe {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub(super) fn observed(&self) -> Vec<Result<Value, i32>> {
        self.outcomes.lock().expect("probe lock").clone()
    }
}

/// Issues one inbound capability call during initialize and settles it before answering.
pub(super) fn capability_probe_responder(
    owner: ModOwner,
    sidecar: SidecarOwnerIdentity,
    registrations: RegistrationBatch,
    capability: Capability,
    probe: Arc<CapabilityProbe>,
) -> Responder {
    let mut standard = standard_responder(sidecar.clone(), registrations, Vec::new());
    let mut deferred: Option<RpcId> = None;
    let probe_id = RpcId::new(CHILD_REQUEST_ID_FIRST).expect("positive identity");
    Box::new(move |inbound| {
        if inbound.kind == SidecarEnvelopeKind::Response {
            record_probe(&probe, inbound);
            return deferred
                .take()
                .map(|id| vec![response(&sidecar, id, RpcResult::Initialized)])
                .unwrap_or_default();
        }
        let Some(request) = decode(inbound) else {
            return Vec::new();
        };
        if request.method != RpcMethod::Initialize {
            return standard(inbound);
        }
        let handle = initialize_handle(&request).unwrap_or_else(|| {
            ConversationHandle::new("forged-handle".into()).expect("bounded handle")
        });
        deferred = Some(request.id);
        vec![capability_request(
            &sidecar, &owner, probe_id, capability, handle,
        )]
    })
}

fn record_probe(probe: &Arc<CapabilityProbe>, inbound: &SidecarEnvelope) {
    let Ok(decoded) = serde_json::from_value::<RpcResponse>(inbound.payload.clone()) else {
        return;
    };
    let outcome = match (decoded.result, decoded.error) {
        (Some(RpcResult::Value { value }), _) => Ok(value),
        (_, Some(error)) => Err(error.code),
        _ => Err(0),
    };
    probe.outcomes.lock().expect("probe lock").push(outcome);
}

fn initialize_handle(request: &RpcRequest) -> Option<ConversationHandle> {
    match &request.params {
        RpcParams::Initialize {
            conversation_handle,
            ..
        } => Some(conversation_handle.clone()),
        _ => None,
    }
}

pub(super) fn capability_request(
    sidecar: &SidecarOwnerIdentity,
    owner: &ModOwner,
    id: RpcId,
    capability: Capability,
    handle: ConversationHandle,
) -> SidecarEnvelope {
    let request = RpcRequest::new(
        id,
        RpcMethod::CapabilityCall,
        RpcParams::CapabilityCall {
            owner: owner.clone(),
            conversation_handle: handle,
            capability,
            operation: "probe".into(),
            params: json!({"probe": true}),
        },
    );
    envelope(
        sidecar,
        SidecarEnvelopeKind::Request,
        &id.value().to_string(),
        None,
        serde_json::to_value(request).expect("encode request"),
    )
}
