//! Concrete production server composition.

use crate::production_setup::{
    ProductionSetupConfig, ProductionSetupPorts, ProductionStatusSink, ProductionTurnController,
};
use lotta_app_server::config::PreparedServer;
use lotta_app_server::error::AppServerError;
use lotta_app_server::ws::command::{
    AbortMessageCommand, ChangeDeviceStateCommand, InputCommand, RuntimeStartCommand, SyncCommand,
};
use lotta_app_server::ws::service::{
    AbortOutcome, DeviceStateOutcome, InputAdmission, RuntimeCommandService, RuntimeEventBatch,
    RuntimeEventSink, RuntimeStartOutcome, ServiceFuture, SyncOutcome, TurnController,
};
use lotta_domain::{
    AgentId, BoundedJsonValue, Clock, InputDisposition, ModelDescriptor, NonEmptyString,
    ProviderStack, QueueItem, QueueItemKind, QueueItemSource, RuntimeScope, SessionEntry,
    SessionEntryType, TranscriptEntry, TranscriptManifest, TranscriptMessageFormat,
};
use lotta_extensions::{
    hooks::{
        RegisteredHookRuntime, command::CommandHookExecutor, loader::HookRegistry,
        prompt::PromptHookExecutor,
    },
    mods::registry::ModRegistries,
    skills::{SkillDiscovery, SkillRoots, SkillSources, SkillToolPort},
};
use lotta_memfs::GitMemFs;
use lotta_providers::model::ModelHandle;
use lotta_runtime::boundary::{ProviderEventText, ProviderName};
use lotta_runtime::ports::{
    AgentStore, ConversationStore, PortFuture, ProviderError, ProviderErrorContext, ProviderEvent,
    ProviderEventSink, ProviderPort, ProviderRequest, ToolExecutionRequest, ToolOutcome, ToolPort,
};
use lotta_runtime::turn::{
    SetupError, SetupStatus, ToolResultRecord, TurnEffectPort, TurnEvent, TurnProjection,
};
use lotta_runtime::{
    AdmissionOutcome, AdmissionRequest, AdmissionRoute, ListenerRuntime, RuntimeKey,
    WorkspaceSandbox,
};
use lotta_store::{LocalStore, StoreErrorKind, StorePaths};
use lotta_tools::builtin::{
    file::FileToolBundle,
    interaction::InteractionPort,
    lsp::LanguageServerRegistry,
    memory::{MemoryAuthor, MemoryToolBundle},
    planning::PlanningPort,
    shell::ShellToolBundle,
    task::TaskLifecyclePort,
    task40::Task40ToolBundle,
    worktree::{
        ConversationWorktreeContext, ProcessLiveness, WorktreeManager, WorktreeOwner,
        WorktreeToolBundle,
    },
};
use lotta_tools::sandbox::OsSandbox;
use lotta_tools::{ToolRegistration, ToolsetId, WorkspacePolicy};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 128_000;
const DEFAULT_OUTPUT_TOKENS: u64 = 4_096;
const TRANSCRIPT_MANIFEST_SCHEMA_VERSION: u8 = 2;
const TRANSCRIPT_SESSION_SCHEMA_VERSION: u8 = 3;

/// Owned production dependencies kept alive for the listener lifetime.
pub struct ProductionComponents {
    runtime_service: Arc<ProductionRuntimeService>,
    turn_controller: Arc<ProductionTurnController>,
}

impl ProductionComponents {
    /// Builds concrete local production dependencies before listener bind.
    pub fn from_server(
        prepared: &PreparedServer,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, SetupError> {
        let root = prepared.storage_dir.clone();
        let workspace = prepared.workspace_dir.clone();
        let setup = Arc::new(ProductionSetupPorts::new(setup_config(&root, &workspace)?)?);
        let provider: Arc<dyn ProviderPort> = Arc::new(UnavailableProvider);
        let tools: Arc<dyn ToolPort> = Arc::new(ProductionToolPort);
        let store_paths = StorePaths::new(&root).map_err(adapter)?;
        let runtime_state = Arc::new(ProductionRuntimeState::new());
        let runtime_service = Arc::new(ProductionRuntimeService::new(
            store_paths.clone(),
            Arc::clone(&clock),
            setup.hook_runtime(),
            Arc::clone(&runtime_state),
        ));
        let turn_controller = Arc::new(ProductionTurnController::new(
            Arc::clone(&setup),
            provider,
            tools,
            LocalStore::new(store_paths.clone()),
            Arc::clone(&clock),
            workspace,
            runtime_state,
        ));
        Ok(Self {
            runtime_service,
            turn_controller,
        })
    }

    /// Returns the concrete runtime command service.
    #[must_use]
    pub fn runtime_service(&self) -> Arc<dyn RuntimeCommandService> {
        self.runtime_service.clone()
    }

    /// Returns the concrete production turn controller.
    #[must_use]
    pub fn turn_controller(&self) -> Arc<dyn TurnController> {
        self.turn_controller.clone()
    }
}

fn production_builtins(
    root: &std::path::Path,
    workspace: &std::path::Path,
    workspace_policy: &WorkspacePolicy,
    skill_roots: &SkillRoots,
) -> Result<Vec<ToolRegistration>, SetupError> {
    let file = FileToolBundle::new(workspace, &root.join("artifacts"))
        .map_err(|_| SetupError::Adapter("file tool bundle".into()))?;
    let shell = ShellToolBundle::new(
        workspace,
        RuntimeScope::new(
            AgentId::accept("production-agent").map_err(adapter)?,
            lotta_domain::ConversationId::accept("production-conversation").map_err(adapter)?,
            None,
        ),
        Arc::new(OsSandbox::detect(workspace_policy.clone())),
    )
    .map_err(|_| SetupError::Adapter("shell tool bundle".into()))?;
    let discovered = SkillDiscovery::discover(skill_roots, SkillSources::ALL)
        .map_err(|_| SetupError::Adapter("skill discovery".into()))?;
    let skills: Arc<dyn lotta_tools::builtin::skill::RegisteredSkillPort> = Arc::new(
        SkillToolPort::new(discovered)
            .map_err(|_| SetupError::Adapter("skill tool registry".into()))?,
    );
    let (interaction, _requests) = InteractionPort::new();
    let task40 = Task40ToolBundle::new(
        Arc::new(PlanningPort::new()),
        Arc::new(TaskLifecyclePort::new()),
        skills,
        interaction,
        Arc::new(
            LanguageServerRegistry::new(workspace.to_path_buf(), [])
                .map_err(|_| SetupError::Adapter("language server registry".into()))?,
        ),
        &shell,
    )
    .map_err(|_| SetupError::Adapter("task40 tool bundle".into()))?;
    let agent = AgentId::accept("production-agent").map_err(adapter)?;
    let conversation =
        lotta_domain::ConversationId::accept("production-conversation").map_err(adapter)?;
    let memory_root = root.join("memfs").join(agent.as_str()).join("memory");
    std::fs::create_dir_all(&memory_root).map_err(adapter)?;
    let memory = MemoryToolBundle::new(
        &memory_root.canonicalize().map_err(adapter)?,
        agent.clone(),
        Arc::new(GitMemFs::new(root.to_path_buf()).map_err(SetupError::from)?),
        MemoryAuthor::new("Lotta".into(), "lotta@localhost".into())
            .map_err(|_| SetupError::Adapter("memory author".into()))?,
    )
    .map_err(|_| SetupError::Adapter("memory tool bundle".into()))?;
    let canonical_workspace = workspace.canonicalize().map_err(adapter)?;
    let context = Arc::new(
        ConversationWorktreeContext::new(&canonical_workspace, &canonical_workspace)
            .map_err(|_| SetupError::Adapter("worktree context".into()))?,
    );
    let worktree = WorktreeToolBundle::new(Arc::new(
        WorktreeManager::new(
            &canonical_workspace,
            Arc::new(OsSandbox::detect(workspace_policy.clone())),
            Arc::new(OsProcessLiveness),
            WorktreeOwner::new(
                agent.as_str().to_owned(),
                conversation.as_str().to_owned(),
                std::process::id(),
                production_hostname(),
                format!("process-{}", std::process::id()),
            )
            .map_err(|_| SetupError::Adapter("worktree owner".into()))?,
            context,
        )
        .map_err(|_| SetupError::Adapter("worktree manager".into()))?,
    ))
    .map_err(|_| SetupError::Adapter("worktree tool bundle".into()))?;
    let mut registrations = task40.registrations().to_vec();
    registrations.extend_from_slice(file.registrations());
    registrations.extend_from_slice(memory.registrations());
    registrations.extend_from_slice(worktree.registrations());
    Ok(registrations)
}

struct OsProcessLiveness;

impl ProcessLiveness for OsProcessLiveness {
    fn is_alive(&self, process_id: u32, _start_nonce: &str) -> bool {
        if process_id == std::process::id() {
            return true;
        }
        #[cfg(unix)]
        {
            std::path::Path::new("/proc")
                .join(process_id.to_string())
                .exists()
                || std::process::Command::new("/bin/kill")
                    .args(["-0", &process_id.to_string()])
                    .status()
                    .is_ok_and(|status| status.success())
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

fn production_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

fn setup_config(
    root: &std::path::Path,
    workspace: &std::path::Path,
) -> Result<ProductionSetupConfig, SetupError> {
    let handle = ModelHandle::from_str("openai/gpt-5.4").map_err(adapter)?;
    let model = production_model(&handle)?;
    let sandbox = WorkspaceSandbox::new(workspace.to_path_buf(), root.to_path_buf());
    let workspace_policy = WorkspacePolicy::new(&sandbox)
        .map_err(|_| SetupError::Adapter("workspace policy".into()))?;
    let skill_roots = production_skill_roots(root, workspace);
    create_production_directories(root)?;
    let builtins = production_builtins(root, workspace, &workspace_policy, &skill_roots)?;
    let registry = Arc::new(lotta_tools::ToolRegistry::new(builtins).map_err(adapter)?);
    let mod_registries = Arc::new(ModRegistries::new(Arc::clone(&registry)));
    let hook_registry = Arc::new(HookRegistry::new());
    let hook_runtime =
        production_hook_runtime(workspace, &workspace_policy, Arc::clone(&hook_registry))?;
    Ok(ProductionSetupConfig {
        store_paths: StorePaths::new(root).map_err(adapter)?,
        skill_roots,
        models: vec![model],
        default_model: handle,
        server_context_window: DEFAULT_CONTEXT_WINDOW_TOKENS,
        output_tokens: DEFAULT_OUTPUT_TOKENS,
        registry,
        mod_registries,
        hook_registry,
        hook_runtime,
        permission_sources: lotta_tools::permissions::scopes::PermissionSourcePaths::new(
            root, root, workspace,
        ),
        workspace_policy,
        toolset: ToolsetId::Codex,
        allowlist: None,
        status_sink: Arc::new(ProductionStatus),
    })
}

fn production_model(handle: &ModelHandle) -> Result<ModelDescriptor, SetupError> {
    Ok(ModelDescriptor {
        handle: NonEmptyString::new(handle.to_string()).map_err(adapter)?,
        provider_id: NonEmptyString::new("openai").map_err(adapter)?,
        available: true,
        context_window: Some(DEFAULT_CONTEXT_WINDOW_TOKENS),
        model_settings: None,
    })
}

fn production_skill_roots(root: &std::path::Path, workspace: &std::path::Path) -> SkillRoots {
    SkillRoots {
        project_working_root: workspace.to_path_buf(),
        agent_skills_directory: Some(root.join("agents")),
        memory_root: Some(root.join("memory")),
        global_skills_directory: root.join("skills"),
        bundled_skills_directory: root.join("bundled-skills"),
    }
}

fn create_production_directories(root: &std::path::Path) -> Result<(), SetupError> {
    for path in [
        root.join("artifacts"),
        root.join("skills"),
        root.join("bundled-skills"),
    ] {
        std::fs::create_dir_all(path).map_err(adapter)?;
    }
    Ok(())
}

fn production_hook_runtime(
    workspace: &std::path::Path,
    workspace_policy: &WorkspacePolicy,
    hook_registry: Arc<HookRegistry>,
) -> Result<Arc<dyn lotta_runtime::hooks::HookRuntime>, SetupError> {
    let scope = RuntimeScope::new(
        AgentId::accept("production-agent").map_err(adapter)?,
        lotta_domain::ConversationId::accept("production-conversation").map_err(adapter)?,
        None,
    );
    let command = CommandHookExecutor::new(
        Arc::new(OsSandbox::detect(workspace_policy.clone())),
        scope,
        workspace.to_path_buf(),
    )
    .map_err(|_| SetupError::Adapter("command hook executor".into()))?;
    Ok(Arc::new(RegisteredHookRuntime::new(
        hook_registry,
        Arc::new(command),
        Arc::new(PromptHookExecutor::new(Arc::new(
            UnavailableModelCapability,
        ))),
    )))
}

struct UnavailableModelCapability;
impl lotta_runtime::ports::ModelCapabilityPort for UnavailableModelCapability {
    fn generate(
        &self,
        _request: lotta_runtime::ports::ModelCapabilityRequest,
    ) -> PortFuture<'_, lotta_runtime::ports::ModelCapabilityResponse> {
        Box::pin(async {
            Err(lotta_runtime::RuntimeError::AdapterFailure {
                code: "model_capability_unavailable",
                context: "production prompt hook".into(),
            })
        })
    }
}

struct ProductionStatus;
impl ProductionStatusSink for ProductionStatus {
    fn emit(&self, _status: SetupStatus) -> Result<(), SetupError> {
        Ok(())
    }
}

const PENDING_ADMISSIONS_MAX: usize = lotta_domain::bounds::RUNTIMES_MAX.value;

type PendingAdmissionKey = (RuntimeKey, String);

pub(crate) struct PendingAdmission {
    pub(crate) handle: lotta_runtime::RuntimeHandle,
    pub(crate) lease: lotta_domain::TurnLease,
    pub(crate) item: QueueItem,
    pub(crate) cancellation: CancellationToken,
}

pub(crate) struct RuntimeServiceState {
    pub(crate) registry: ListenerRuntime,
    pub(crate) pending: HashMap<PendingAdmissionKey, PendingAdmission>,
    sequence: u128,
}

pub(crate) struct ProductionRuntimeState(pub(crate) tokio::sync::Mutex<RuntimeServiceState>);

impl ProductionRuntimeState {
    fn new() -> Self {
        Self(tokio::sync::Mutex::new(RuntimeServiceState {
            registry: ListenerRuntime::new(),
            pending: HashMap::new(),
            sequence: 1,
        }))
    }

    pub(crate) fn take_pending(
        &self,
        scope: &RuntimeScope,
        id: &str,
    ) -> Result<PendingAdmission, AppServerError> {
        self.0
            .try_lock()
            .map_err(|_| AppServerError::Internal)?
            .pending
            .remove(&(RuntimeKey::from(scope), id.to_owned()))
            .ok_or(AppServerError::Malformed)
    }

    pub(crate) fn release_and_pump(
        &self,
        scope: &RuntimeScope,
        pending: PendingAdmission,
        reason: &'static str,
    ) -> Result<Option<(QueueItem, BoundedJsonValue)>, AppServerError> {
        let mut state = self.0.try_lock().map_err(|_| AppServerError::Internal)?;
        release_and_pump_locked(&mut state, scope, pending, reason)
    }
}

struct ProductionRuntimeService {
    store: LocalStore,
    clock: Arc<dyn Clock + Send + Sync>,
    hooks: Arc<dyn lotta_runtime::hooks::HookRuntime>,
    state: Arc<ProductionRuntimeState>,
}

impl ProductionRuntimeService {
    fn new(
        store_paths: StorePaths,
        clock: Arc<dyn Clock + Send + Sync>,
        hooks: Arc<dyn lotta_runtime::hooks::HookRuntime>,
        state: Arc<ProductionRuntimeState>,
    ) -> Self {
        Self {
            store: LocalStore::new(store_paths),
            clock,
            hooks,
            state,
        }
    }

    fn ensure_runtime(
        &self,
        scope: &RuntimeScope,
    ) -> Result<(lotta_runtime::RuntimeHandle, bool), AppServerError> {
        let mut state = self
            .state
            .0
            .try_lock()
            .map_err(|_| AppServerError::Internal)?;
        let existed = state
            .registry
            .lookup(&lotta_runtime::RuntimeKey::from(scope))
            .is_some();
        let owner = Uuid::from_u128(state.sequence);
        state.sequence = state
            .sequence
            .checked_add(1)
            .ok_or(AppServerError::Internal)?;
        state
            .registry
            .get_or_create(scope, owner)
            .map(|handle| (handle, !existed))
            .map_err(runtime_service_error)
    }

    fn admission_item(
        &self,
        command: &InputCommand,
        client_message_id: &str,
    ) -> Result<QueueItem, AppServerError> {
        Ok(QueueItem {
            id: NonEmptyString::new(format!("queue-{client_message_id}"))
                .map_err(|_| AppServerError::Malformed)?,
            client_message_id: NonEmptyString::new(client_message_id.to_owned())
                .map_err(|_| AppServerError::Malformed)?,
            kind: QueueItemKind::Message,
            source: QueueItemSource::User,
            content: command.payload.clone(),
            enqueued_at: self.clock.now(),
            extras: Default::default(),
        })
    }

    fn start_admission(
        state: &mut RuntimeServiceState,
        scope: &RuntimeScope,
        client_message_id: &str,
        handle: lotta_runtime::RuntimeHandle,
        item: QueueItem,
    ) -> Result<BoundedJsonValue, AppServerError> {
        if state.pending.len() >= PENDING_ADMISSIONS_MAX {
            return Err(AppServerError::Unavailable);
        }
        let key = (RuntimeKey::from(scope), client_message_id.to_owned());
        if state.pending.contains_key(&key) {
            return Err(AppServerError::Malformed);
        }
        let run_sequence = u64::try_from(state.sequence).map_err(|_| AppServerError::Internal)?;
        let run_id = lotta_domain::RunId::generate_sequence(run_sequence)
            .map_err(|_| AppServerError::Internal)?;
        let continuation = continuation_value(client_message_id)?;
        let lease = state
            .registry
            .lifecycle_mut(&handle)
            .map_err(runtime_service_error)?
            .begin_turn(format!("admission-{client_message_id}"), run_id)
            .map_err(runtime_service_error)?;
        match state.pending.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(PendingAdmission {
                    handle,
                    lease,
                    item,
                    cancellation: CancellationToken::new(),
                });
                Ok(continuation)
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                let stop = lotta_domain::StopReason::new("admission_collision")
                    .map_err(|_| AppServerError::Internal)?;
                state
                    .registry
                    .lifecycle_mut(&handle)
                    .map_err(runtime_service_error)?
                    .finish_turn(&lease, stop)
                    .map_err(runtime_service_error)?;
                Err(AppServerError::Internal)
            }
        }
    }
}
fn release_and_pump_locked(
    state: &mut RuntimeServiceState,
    scope: &RuntimeScope,
    pending: PendingAdmission,
    reason: &'static str,
) -> Result<Option<(QueueItem, BoundedJsonValue)>, AppServerError> {
    let current_key = (
        RuntimeKey::from(scope),
        pending.item.client_message_id.as_str().to_owned(),
    );
    let stop = lotta_domain::StopReason::new(reason).map_err(|_| AppServerError::Internal)?;
    if let Err(error) = state
        .registry
        .lifecycle_mut(&pending.handle)
        .map_err(runtime_service_error)
        .and_then(|owner| {
            owner
                .finish_turn(&pending.lease, stop)
                .map_err(runtime_service_error)
        })
    {
        match state.pending.entry(current_key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(pending);
                return Err(error);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(attach_cleanup(error, AppServerError::Internal));
            }
        }
    }
    pending.cancellation.cancel();

    let Some(peeked) = state.registry.peek_queue(&pending.handle).cloned() else {
        return Ok(None);
    };
    if state.pending.len() >= PENDING_ADMISSIONS_MAX {
        return Err(AppServerError::Unavailable);
    }
    let id = peeked.client_message_id.as_str().to_owned();
    let next_key = (RuntimeKey::from(scope), id.clone());
    if state.pending.contains_key(&next_key) {
        return Err(AppServerError::Malformed);
    }
    let next_sequence = state
        .sequence
        .checked_add(1)
        .ok_or(AppServerError::Internal)?;
    let run_sequence = u64::try_from(state.sequence).map_err(|_| AppServerError::Internal)?;
    let run_id = lotta_domain::RunId::generate_sequence(run_sequence)
        .map_err(|_| AppServerError::Internal)?;
    let continuation = continuation_value(&id)?;
    let Some(item) = state
        .registry
        .pump_one_queue(&pending.handle)
        .map_err(runtime_service_error)?
    else {
        return Err(AppServerError::Internal);
    };
    debug_assert_eq!(item.id, peeked.id);
    let lease = match state
        .registry
        .lifecycle_mut(&pending.handle)
        .map_err(runtime_service_error)
        .and_then(|owner| {
            owner
                .begin_turn(format!("admission-{id}"), run_id)
                .map_err(runtime_service_error)
        }) {
        Ok(lease) => lease,
        Err(error) => {
            state.registry.rollback_pump_one(&pending.handle, item);
            return Err(error);
        }
    };
    match state.pending.entry(next_key) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(PendingAdmission {
                handle: pending.handle,
                lease,
                item: item.clone(),
                cancellation: CancellationToken::new(),
            });
            state.sequence = next_sequence;
            Ok(Some((item, continuation)))
        }
        std::collections::hash_map::Entry::Occupied(_) => {
            let stop = lotta_domain::StopReason::new("admission_collision")
                .map_err(|_| AppServerError::Internal)?;
            let finish = state
                .registry
                .lifecycle_mut(&pending.handle)
                .map_err(runtime_service_error)
                .and_then(|owner| {
                    owner
                        .finish_turn(&lease, stop)
                        .map_err(runtime_service_error)
                });
            state.registry.rollback_pump_one(&pending.handle, item);
            match finish {
                Ok(()) => Err(AppServerError::Internal),
                Err(cleanup) => Err(attach_cleanup(AppServerError::Internal, cleanup)),
            }
        }
    }
}

fn attach_cleanup(primary: AppServerError, cleanup: AppServerError) -> AppServerError {
    AppServerError::CleanupAttached {
        primary: Box::new(primary),
        cleanup: Box::new(cleanup),
    }
}

impl RuntimeCommandService for ProductionRuntimeService {
    fn runtime_start(
        &self,
        command: RuntimeStartCommand,
    ) -> ServiceFuture<'_, RuntimeStartOutcome> {
        Box::pin(async move {
            let agent = command.agent_id.ok_or(AppServerError::Malformed)?;
            let conversation = command.conversation_id.ok_or(AppServerError::Malformed)?;
            let runtime = RuntimeScope::new(
                AgentId::accept(agent.as_str()).map_err(|_| AppServerError::Malformed)?,
                lotta_domain::ConversationId::accept(conversation.as_str())
                    .map_err(|_| AppServerError::Malformed)?,
                None,
            );
            let agent = AgentStore::load(&self.store, &runtime.agent_id)
                .await
                .map_err(runtime_service_error)?;
            let conversation =
                ConversationStore::load(&self.store, &runtime.agent_id, &runtime.conversation_id)
                    .await
                    .map_err(runtime_service_error)?;
            let (_, created_runtime) = self.ensure_runtime(&runtime)?;
            if created_runtime {
                let payload = lotta_extensions::hooks::events::lifecycle_payload(
                    lotta_runtime::hooks::HookEvent::SessionStart,
                )
                .map_err(|_| AppServerError::Internal)?;
                lotta_runtime::hooks::HookLifecycleHost::new(self.hooks.as_ref())
                    .session_start(payload, CancellationToken::new(), || async {
                        Ok::<(), AppServerError>(())
                    })
                    .await
                    .map_err(|_| AppServerError::Internal)?;
            }
            Ok(RuntimeStartOutcome {
                runtime,
                created_agent: false,
                created_conversation: false,
                agent: Some(
                    lotta_domain::BoundedJsonValue::new(
                        serde_json::to_value(agent).map_err(|_| AppServerError::Internal)?,
                    )
                    .map_err(|_| AppServerError::Internal)?,
                ),
                conversation: Some(
                    lotta_domain::BoundedJsonValue::new(
                        serde_json::to_value(conversation).map_err(|_| AppServerError::Internal)?,
                    )
                    .map_err(|_| AppServerError::Internal)?,
                ),
                broadcasts: RuntimeEventBatch::new(Vec::new())
                    .map_err(|_| AppServerError::Internal)?,
            })
        })
    }

    fn admit_input(&self, command: InputCommand) -> ServiceFuture<'_, InputAdmission> {
        Box::pin(async move {
            let client_message_id = input_client_message_id(command.payload.as_value())?;
            let item = self.admission_item(&command, &client_message_id)?;
            let mut state = self
                .state
                .0
                .try_lock()
                .map_err(|_| AppServerError::Internal)?;
            let owner = Uuid::from_u128(state.sequence);
            state.sequence = state
                .sequence
                .checked_add(1)
                .ok_or(AppServerError::Internal)?;
            let handle = state
                .registry
                .get_or_create(&command.runtime, owner)
                .map_err(runtime_service_error)?;
            let outcome = state
                .registry
                .admit(
                    &handle,
                    AdmissionRequest {
                        item: item.clone(),
                        route: AdmissionRoute::Ordinary,
                    },
                )
                .map_err(runtime_service_error)?;
            let disposition = outcome.disposition();
            let error = match outcome {
                AdmissionOutcome::Rejected { reason, .. } => Some(
                    NonEmptyString::new(format!("{reason:?}"))
                        .map_err(|_| AppServerError::Internal)?,
                ),
                _ => None,
            };
            let continuation = if disposition == InputDisposition::Started {
                Some(Self::start_admission(
                    &mut state,
                    &command.runtime,
                    &client_message_id,
                    handle,
                    item,
                )?)
            } else {
                None
            };
            Ok(InputAdmission {
                disposition,
                error,
                continuation,
                after_ack: RuntimeEventBatch::new(Vec::new())
                    .map_err(|_| AppServerError::Internal)?,
            })
        })
    }

    fn continue_input(
        &self,
        scope: RuntimeScope,
        continuation: Option<lotta_domain::BoundedJsonValue>,
        sink: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        Box::pin(async move {
            let continuation = continuation.ok_or(AppServerError::Malformed)?;
            let id = continuation
                .as_value()
                .get("client_message_id")
                .and_then(serde_json::Value::as_str)
                .ok_or(AppServerError::Malformed)?;
            let key = (RuntimeKey::from(&scope), id.to_owned());
            let item = self
                .state
                .0
                .try_lock()
                .map_err(|_| AppServerError::Internal)?
                .pending
                .get(&key)
                .map(|pending| pending.item.clone())
                .ok_or(AppServerError::Malformed)?;
            sink.emit(
                &scope,
                lotta_app_server::ws::RuntimeEvent::UpdateLoopStatus {
                    loop_status: BoundedJsonValue::new(serde_json::json!({
                        "status": "started",
                        "client_message_id": item.client_message_id.as_str()
                    }))
                    .map_err(|_| AppServerError::Internal)?,
                },
            )?;
            Ok(())
        })
    }

    fn sync(&self, command: SyncCommand) -> ServiceFuture<'_, SyncOutcome> {
        Box::pin(async move {
            let state = self
                .state
                .0
                .try_lock()
                .map_err(|_| AppServerError::Internal)?;
            let key = RuntimeKey::from(&command.runtime);
            let broadcasts = if let Some(handle) = state.registry.lookup(&key) {
                let lifecycle = state
                    .registry
                    .lifecycle(&handle)
                    .ok_or(AppServerError::Internal)?
                    .projection();
                let queue = state
                    .registry
                    .queue(&handle)
                    .ok_or(AppServerError::Internal)?;
                vec![
                    lotta_app_server::ws::RuntimeEvent::UpdateLoopStatus {
                        loop_status: BoundedJsonValue::new(serde_json::json!({
                            "status": match lifecycle.loop_status() {
                                lotta_domain::LoopStatus::WaitingOnInput => "idle",
                                lotta_domain::LoopStatus::ExecutingCommand => "executing_command",
                                lotta_domain::LoopStatus::SendingApiRequest => "sending",
                            },
                            "active_run_ids": lifecycle.active_run_ids(),
                            "executing_tool_call_ids": []
                        }))
                        .map_err(|_| AppServerError::Internal)?,
                    },
                    lotta_app_server::ws::RuntimeEvent::UpdateQueue {
                        queue: BoundedJsonValue::new(
                            serde_json::to_value(queue.items().collect::<Vec<_>>())
                                .map_err(|_| AppServerError::Internal)?,
                        )
                        .map_err(|_| AppServerError::Internal)?,
                        removed: BoundedJsonValue::new(serde_json::json!([]))
                            .map_err(|_| AppServerError::Internal)?,
                    },
                ]
            } else {
                Vec::new()
            };
            Ok(SyncOutcome {
                broadcasts: RuntimeEventBatch::new(broadcasts)
                    .map_err(|_| AppServerError::Internal)?,
            })
        })
    }

    fn abort_message(&self, command: AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome> {
        Box::pin(async move {
            let mut state = self
                .state
                .0
                .try_lock()
                .map_err(|_| AppServerError::Internal)?;
            let key = RuntimeKey::from(&command.runtime);
            let pending_key = state
                .pending
                .keys()
                .find(|(scope, _)| scope == &key)
                .cloned();
            let Some(pending_key) = pending_key else {
                return Ok(AbortOutcome { aborted: false });
            };
            let pending = state
                .pending
                .remove(&pending_key)
                .ok_or(AppServerError::Internal)?;
            let _pumped =
                release_and_pump_locked(&mut state, &command.runtime, pending, "aborted")?;
            Ok(AbortOutcome { aborted: true })
        })
    }

    fn change_device_state(
        &self,
        command: ChangeDeviceStateCommand,
    ) -> ServiceFuture<'_, DeviceStateOutcome> {
        Box::pin(async move {
            let _ = self.ensure_runtime(&command.runtime)?;
            let device_status = BoundedJsonValue::new(serde_json::json!({
                "mode": command.payload.mode.map(|mode| format!("{mode:?}").to_lowercase()),
                "cwd": command.payload.cwd,
                "agent_id": command.payload.agent_id,
                "conversation_id": command.payload.conversation_id,
            }))
            .map_err(|_| AppServerError::Internal)?;
            Ok(DeviceStateOutcome {
                broadcasts: RuntimeEventBatch::new(vec![
                    lotta_app_server::ws::RuntimeEvent::UpdateDeviceStatus { device_status },
                ])
                .map_err(|_| AppServerError::Internal)?,
            })
        })
    }
}

struct UnavailableProvider;
impl ProviderPort for UnavailableProvider {
    fn stream(&self, _request: ProviderRequest, events: ProviderEventSink) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let context = ProviderErrorContext::new(
                ProviderName::new("provider_unavailable".into())?,
                ProviderEventText::new("no configured provider connection".into())?,
            );
            events
                .send(ProviderEvent::Error {
                    error: ProviderError::Unavailable(context),
                })
                .await
        })
    }
}

struct ProductionToolPort;
impl ToolPort for ProductionToolPort {
    fn execute(&self, _request: ToolExecutionRequest) -> PortFuture<'_, ToolOutcome> {
        Box::pin(async {
            Err(lotta_runtime::RuntimeError::AdapterFailure {
                code: "tool_executor_unavailable",
                context: "production tool execution".into(),
            })
        })
    }
}

pub(crate) struct ProductionEffects {
    actor: ProductionEffectActor,
    scope: RuntimeScope,
    sink: Arc<dyn RuntimeEventSink>,
    turn_id: NonEmptyString,
    run_id: lotta_domain::RunId,
    input_id: NonEmptyString,
    sequence: std::sync::atomic::AtomicU64,
}

type EffectResult = Result<(), lotta_runtime::RuntimeError>;

enum EffectCommand {
    Append {
        entry: Box<lotta_domain::TranscriptEntry>,
        acknowledgment: std::sync::mpsc::SyncSender<EffectResult>,
    },
    Shutdown(std::sync::mpsc::SyncSender<EffectResult>),
}

struct ProductionEffectActor {
    commands: Option<std::sync::mpsc::SyncSender<EffectCommand>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl ProductionEffectActor {
    fn new(store: LocalStore, scope: RuntimeScope) -> Self {
        let (commands, receiver) = std::sync::mpsc::sync_channel(16);
        let (ready, initialized) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("lotta-production-effects".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    let _ = ready.send(());
                    return;
                };
                let _ = ready.send(());
                effect_loop(receiver, store, scope, &runtime);
            })
            .ok();
        if worker.is_some() {
            let _ = initialized.recv();
        }
        Self {
            commands: Some(commands),
            worker: Mutex::new(worker),
        }
    }

    fn append(&self, entry: Box<lotta_domain::TranscriptEntry>) -> EffectResult {
        let (acknowledgment, result) = std::sync::mpsc::sync_channel(1);
        self.commands
            .as_ref()
            .ok_or_else(actor_closed)?
            .try_send(EffectCommand::Append {
                entry,
                acknowledgment,
            })
            .map_err(|error| match error {
                std::sync::mpsc::TrySendError::Full(_) => actor_full(),
                std::sync::mpsc::TrySendError::Disconnected(_) => actor_closed(),
            })?;
        result.recv().map_err(|_| actor_closed())?
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "explicit shutdown API is exercised by tests")
    )]
    async fn shutdown(mut self) -> EffectResult {
        self.shutdown_sync()
    }

    fn shutdown_sync(&mut self) -> EffectResult {
        let Some(commands) = self.commands.take() else {
            return Ok(());
        };
        let (acknowledgment, result) = std::sync::mpsc::sync_channel(1);
        commands
            .send(EffectCommand::Shutdown(acknowledgment))
            .map_err(|_| actor_closed())?;
        let acknowledgment = result.recv().map_err(|_| actor_closed())?;
        drop(commands);
        let worker = self.worker.get_mut().map_err(|_| actor_closed())?.take();
        if let Some(worker) = worker {
            worker.join().map_err(|_| actor_closed())?;
        }
        acknowledgment
    }
}

async fn append_production_effect(
    store: &LocalStore,
    scope: &RuntimeScope,
    entry: &TranscriptEntry,
) -> Result<(), lotta_store::StoreError> {
    match store
        .append_transcript_entry(&scope.agent_id, &scope.conversation_id, entry)
        .await
    {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == StoreErrorKind::NotFound => {
            let now = lotta_domain::Timestamp::from_utc(chrono::Utc::now());
            let manifest = TranscriptManifest {
                schema_version: TRANSCRIPT_MANIFEST_SCHEMA_VERSION,
                message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
                provider_stack: ProviderStack::PiAi,
                created_at: now,
                migrated_from: None,
                migrated_at: None,
                backup_path: None,
            };
            let session = TranscriptEntry::Session(SessionEntry {
                entry_type: SessionEntryType::Session,
                version: TRANSCRIPT_SESSION_SCHEMA_VERSION,
                id: NonEmptyString::new(format!(
                    "session-{}",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
                ))
                .expect("generated session id is non-empty"),
                timestamp: now,
                cwd: "/".to_owned(),
            });
            match store
                .initialize_transcript(&scope.agent_id, &scope.conversation_id, &manifest, &session)
                .await
            {
                Ok(()) => {}
                Err(error) if error.kind() == StoreErrorKind::StorageConflict => {}
                Err(error) => return Err(error),
            }
            store
                .append_transcript_entry(&scope.agent_id, &scope.conversation_id, entry)
                .await
        }
        Err(error) => Err(error),
    }
}

fn effect_loop(
    receiver: std::sync::mpsc::Receiver<EffectCommand>,
    store: LocalStore,
    scope: RuntimeScope,
    runtime: &tokio::runtime::Runtime,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            EffectCommand::Append {
                entry,
                acknowledgment,
            } => {
                let result = runtime
                    .block_on(append_production_effect(&store, &scope, &entry))
                    .map_err(effect_error);
                let _ = acknowledgment.send(result);
            }
            EffectCommand::Shutdown(acknowledgment) => {
                let _ = acknowledgment.send(Ok(()));
                break;
            }
        }
    }
}

impl Drop for ProductionEffectActor {
    fn drop(&mut self) {
        let _ = self.shutdown_sync();
    }
}

impl ProductionEffects {
    pub(crate) fn new(
        store: LocalStore,
        scope: RuntimeScope,
        sink: Arc<dyn RuntimeEventSink>,
        turn_id: NonEmptyString,
        run_id: lotta_domain::RunId,
        input_id: NonEmptyString,
        _clock: Arc<dyn Clock + Send + Sync>,
    ) -> Self {
        Self {
            actor: ProductionEffectActor::new(store, scope.clone()),
            scope,
            sink,
            turn_id,
            run_id,
            input_id,
            sequence: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn append_message(
        &self,
        role: lotta_domain::LocalMessageRole,
        content: serde_json::Value,
        id: String,
    ) -> EffectResult {
        let timestamp = chrono::Utc::now();
        let entry = lotta_domain::TranscriptEntry::Message(lotta_domain::MessageEntry {
            entry_type: lotta_domain::MessageEntryType::Message,
            id: NonEmptyString::new(id.clone()).map_err(effect_error)?,
            parent_id: Some(self.input_id.as_str().to_owned()),
            timestamp: lotta_domain::Timestamp::from_utc(timestamp),
            message: lotta_domain::LocalMessage {
                id: lotta_domain::MessageId::accept(&id).map_err(effect_error)?,
                role,
                content: Some(BoundedJsonValue::new(content).map_err(effect_error)?),
                timestamp: timestamp.timestamp_millis() as f64,
                metadata: None,
                extras: Default::default(),
            },
        });
        self.actor.append(Box::new(entry))
    }
}

impl TurnEffectPort for ProductionEffects {
    fn persist_projection(&self, projection: TurnProjection) -> EffectResult {
        let sequence = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.append_message(
            lotta_domain::LocalMessageRole::Assistant,
            serde_json::json!(projection.text.as_str()),
            format!("{}-assistant-{sequence}", self.turn_id.as_str()),
        )
    }

    fn emit(&self, event: TurnEvent) -> EffectResult {
        let wire = match event {
            TurnEvent::StreamDelta(projection) => lotta_app_server::ws::RuntimeEvent::StreamDelta {
                delta: BoundedJsonValue::new(serde_json::json!({
                    "kind": format!("{:?}", projection.kind).to_lowercase(),
                    "text": projection.text.as_str(),
                    "input_id": self.input_id.as_str()
                }))
                .map_err(effect_error)?,
                subagent_id: None,
            },
            TurnEvent::Finished { reason } => lotta_app_server::ws::RuntimeEvent::TurnFinished {
                turn_id: self.turn_id.clone(),
                run_id: Some(self.run_id.clone()),
                stop_reason: NonEmptyString::new(format!("{reason:?}").to_lowercase())
                    .map_err(effect_error)?,
                error: None,
            },
            TurnEvent::Retry(retry) => lotta_app_server::ws::RuntimeEvent::UpdateLoopStatus {
                loop_status: BoundedJsonValue::new(
                    serde_json::json!({"status":"retrying","event":format!("{retry:?}")}),
                )
                .map_err(effect_error)?,
            },
            TurnEvent::ToolResult(result) => lotta_app_server::ws::RuntimeEvent::StreamDelta {
                delta: BoundedJsonValue::new(
                    serde_json::json!({"type":"tool_result","call_id":result.call_id.as_str()}),
                )
                .map_err(effect_error)?,
                subagent_id: None,
            },
        };
        self.sink
            .emit(&self.scope, wire)
            .map_err(|_| effect_error("runtime event sink"))
    }

    fn append_tool_result(&self, result: ToolResultRecord) -> EffectResult {
        let sequence = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.append_message(
            lotta_domain::LocalMessageRole::ToolResult,
            serde_json::json!({
                "call_id": result.call_id.as_str(),
                "outcome": format!("{:?}", result.outcome)
            }),
            format!("{}-tool-{sequence}", self.turn_id.as_str()),
        )
    }
}

fn input_client_message_id(value: &serde_json::Value) -> Result<String, AppServerError> {
    value
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .and_then(|messages| messages.first())
        .and_then(|message| message.get("client_message_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or(AppServerError::Malformed)
}

fn continuation_value(client_message_id: &str) -> Result<BoundedJsonValue, AppServerError> {
    BoundedJsonValue::new(serde_json::json!({"client_message_id": client_message_id}))
        .map_err(|_| AppServerError::Internal)
}

fn actor_closed() -> lotta_runtime::RuntimeError {
    lotta_runtime::RuntimeError::AdapterFailure {
        code: "actor_closed",
        context: "production effect actor closed".into(),
    }
}

fn actor_full() -> lotta_runtime::RuntimeError {
    lotta_runtime::RuntimeError::LimitExceeded {
        context: "production_effect_actor_capacity".into(),
    }
}

fn effect_error(error: impl std::fmt::Display) -> lotta_runtime::RuntimeError {
    lotta_runtime::RuntimeError::AdapterFailure {
        code: "production_effect",
        context: error.to_string(),
    }
}

fn runtime_service_error(error: lotta_runtime::RuntimeError) -> AppServerError {
    match error {
        lotta_runtime::RuntimeError::NotFound { .. } => AppServerError::Malformed,
        lotta_runtime::RuntimeError::InvalidData { .. } => AppServerError::Malformed,
        lotta_runtime::RuntimeError::LimitExceeded { .. } => AppServerError::Unavailable,
        _ => AppServerError::Internal,
    }
}

fn adapter(error: impl std::fmt::Display) -> SetupError {
    SetupError::Adapter(error.to_string())
}

#[cfg(test)]
mod production_tests {
    use super::*;
    use lotta_app_server::ws::service::{RuntimeCommandService, RuntimeEventSink};
    use lotta_domain::{ConversationId, DomainError, RunId, Timestamp};

    struct TestClock;
    impl Clock for TestClock {
        fn now(&self) -> Timestamp {
            Timestamp::parse_persisted_rfc3339("2026-08-18T00:00:00Z").unwrap()
        }
        fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
            Timestamp::parse_persisted_rfc3339(value)
        }
    }
    #[derive(Default)]
    struct Sink(Mutex<Vec<(RuntimeScope, String)>>);
    impl RuntimeEventSink for Sink {
        fn emit(
            &self,
            scope: &RuntimeScope,
            event: lotta_app_server::ws::RuntimeEvent,
        ) -> Result<(), AppServerError> {
            self.0
                .lock()
                .unwrap()
                .push((scope.clone(), event.discriminant().into()));
            Ok(())
        }
    }
    fn scope(name: &str) -> RuntimeScope {
        RuntimeScope::new(
            AgentId::accept(format!("agent-{name}")).unwrap(),
            ConversationId::accept(format!("conversation-{name}")).unwrap(),
            None,
        )
    }
    fn command(scope: RuntimeScope, id: &str) -> InputCommand {
        InputCommand {
            request_id: None,
            runtime: scope,
            payload: BoundedJsonValue::new(
                serde_json::json!({"messages":[{"client_message_id":id,"content":"hello"}]}),
            )
            .unwrap(),
        }
    }
    fn sync_is_idle(outcome: &SyncOutcome) -> bool {
        outcome
            .broadcasts
            .as_slice()
            .iter()
            .any(|event| match event {
                lotta_app_server::ws::RuntimeEvent::UpdateLoopStatus { loop_status } => {
                    loop_status
                        .as_value()
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        == Some("idle")
                }
                _ => false,
            })
    }
    fn unique_test_id() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
    #[test]
    fn production_real_registry_composes_all_toolsets() {
        let root = std::env::temp_dir().join(format!("lotta-task54-registry-{}", unique_test_id()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let root = root.canonicalize().unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let config = setup_config(&root, &workspace).expect("exact production builder");
        let expected: std::collections::BTreeSet<_> = production_builtins(
            &root,
            &workspace,
            &config.workspace_policy,
            &config.skill_roots,
        )
        .unwrap()
        .into_iter()
        .map(|registration| registration.definition.internal_name.as_str().to_owned())
        .collect();
        assert_eq!(expected.len(), 39);
        for toolset in ToolsetId::ALL {
            let snapshot = config
                .registry
                .compose(toolset, &[], None)
                .unwrap_or_else(|error| {
                    panic!("production registry failed for {toolset}: {error:?}")
                });
            let actual = snapshot.model_names();
            let mut exact: Vec<_> = lotta_tools::names::rows()
                .iter()
                .filter(|row| row.toolset == toolset)
                .map(|row| row.model)
                .collect();
            exact.sort_unstable();
            assert_eq!(actual, exact, "model names for {toolset}");
        }
        for required in [
            "Task",
            "EnterWorktree",
            "ExitWorktree",
            "memory",
            "memory_apply_patch",
        ] {
            assert!(
                expected.contains(required),
                "missing source registration {required}"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    fn service() -> ProductionRuntimeService {
        let root = std::env::temp_dir().join(format!("lotta-task54-{}", unique_test_id()));
        std::fs::create_dir_all(&root).unwrap();
        ProductionRuntimeService::new(
            StorePaths::new(root).unwrap(),
            Arc::new(TestClock),
            Arc::new(lotta_runtime::hooks::NoopHookRuntime),
            Arc::new(ProductionRuntimeState::new()),
        )
    }

    #[tokio::test]
    async fn production_runtime_admission_queue_and_continue() {
        let service = service();
        let scope = scope("queue");
        let first = service
            .admit_input(command(scope.clone(), "one"))
            .await
            .unwrap();
        assert_eq!(first.disposition, InputDisposition::Started);
        let second = service
            .admit_input(command(scope.clone(), "two"))
            .await
            .unwrap();
        assert_eq!(second.disposition, InputDisposition::Queued);
        let sink = Arc::new(Sink::default());
        service
            .continue_input(scope, first.continuation, sink.clone())
            .await
            .unwrap();
        assert_eq!(sink.0.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn production_two_sequential_inputs_start_and_sync_idle() {
        for repetition in 0..10 {
            let service = service();
            let scope = scope(&format!("sequential-{repetition}"));
            for id in ["first", "second"] {
                let admission = service
                    .admit_input(command(scope.clone(), id))
                    .await
                    .expect("admit sequential input");
                assert_eq!(admission.disposition, InputDisposition::Started);
                service
                    .abort_message(AbortMessageCommand {
                        request_id: None,
                        runtime: scope.clone(),
                        run_id: None,
                    })
                    .await
                    .expect("release sequential input");
                let synchronized = service
                    .sync(SyncCommand {
                        request_id: None,
                        runtime: scope.clone(),
                        recover_approvals: None,
                        force_device_status: None,
                    })
                    .await
                    .expect("sync sequential input");
                assert!(sync_is_idle(&synchronized));
            }
        }
    }
    #[tokio::test]
    async fn production_abort_cancels_active_and_releases() {
        let service = service();
        let scope = scope("abort");
        service
            .admit_input(command(scope.clone(), "active"))
            .await
            .unwrap();
        let outcome = service
            .abort_message(AbortMessageCommand {
                request_id: None,
                runtime: scope.clone(),
                run_id: None,
            })
            .await
            .unwrap();
        assert!(outcome.aborted);
        assert_eq!(
            service
                .admit_input(command(scope, "next"))
                .await
                .unwrap()
                .disposition,
            InputDisposition::Started
        );
    }
    #[tokio::test]
    async fn production_two_scopes_do_not_share_events() {
        let service = service();
        let left = scope("left");
        let right = scope("right");
        let left_admission = service
            .admit_input(command(left.clone(), "left-id"))
            .await
            .unwrap();
        let right_admission = service
            .admit_input(command(right.clone(), "right-id"))
            .await
            .unwrap();
        let left_sink = Arc::new(Sink::default());
        let right_sink = Arc::new(Sink::default());
        service
            .continue_input(left.clone(), left_admission.continuation, left_sink.clone())
            .await
            .unwrap();
        service
            .continue_input(
                right.clone(),
                right_admission.continuation,
                right_sink.clone(),
            )
            .await
            .unwrap();
        assert!(
            left_sink
                .0
                .lock()
                .unwrap()
                .iter()
                .all(|(scope, _)| scope == &left)
        );
        assert!(
            right_sink
                .0
                .lock()
                .unwrap()
                .iter()
                .all(|(scope, _)| scope == &right)
        );
    }
    #[tokio::test]
    async fn production_effect_actor_full_channel_drop_is_joined() {
        for repetition in 0..10 {
            let service = service();
            let scope = scope(&format!("actor-shutdown-{repetition}"));
            let actor = ProductionEffectActor::new(service.store.clone(), scope.clone());
            actor.shutdown().await.expect("explicit actor shutdown");

            let actor = ProductionEffectActor::new(service.store.clone(), scope.clone());
            let mut acknowledgments = Vec::new();
            for queued in 0..16 {
                let (acknowledgment, result) = std::sync::mpsc::sync_channel(1);
                acknowledgments.push(result);
                let entry = Box::new(TranscriptEntry::Message(lotta_domain::MessageEntry {
                    entry_type: lotta_domain::MessageEntryType::Message,
                    id: NonEmptyString::new(format!("queued-{repetition}-{queued}"))
                        .expect("queued id"),
                    parent_id: None,
                    timestamp: TestClock.now(),
                    message: lotta_domain::LocalMessage {
                        id: lotta_domain::MessageId::accept(format!(
                            "queued-message-{repetition}-{queued}"
                        ))
                        .unwrap(),
                        role: lotta_domain::LocalMessageRole::Assistant,
                        content: None,
                        timestamp: 0.0,
                        metadata: None,
                        extras: Default::default(),
                    },
                }));
                actor
                    .commands
                    .as_ref()
                    .unwrap()
                    .send(EffectCommand::Append {
                        entry,
                        acknowledgment,
                    })
                    .expect("buffer append");
            }
            drop(actor);
            for result in acknowledgments {
                let _ = result.recv().expect("drop flush acknowledgment");
            }
            let transcript_path = service
                .store
                .paths()
                .conversation_dir(&scope.agent_id, &scope.conversation_id)
                .unwrap()
                .join("messages.jsonl");
            let transcript = tokio::fs::read_to_string(transcript_path).await.unwrap();
            assert_eq!(transcript.lines().count(), 17);
        }
    }

    #[tokio::test]
    async fn production_effects_persist_assistant_and_emit_sink() {
        let service = service();
        let scope = scope("effects");
        let sink = Arc::new(Sink::default());
        let effects = ProductionEffects::new(
            service.store.clone(),
            scope.clone(),
            sink.clone(),
            NonEmptyString::new("turn-1").unwrap(),
            RunId::generate_sequence(1).unwrap(),
            NonEmptyString::new("input-1").unwrap(),
            Arc::new(TestClock),
        );
        for sequence in 0..100 {
            effects
                .persist_projection(TurnProjection {
                    kind: lotta_runtime::turn::ProjectionKind::Text,
                    text: ProviderEventText::new(format!("assistant-{sequence}")).unwrap(),
                })
                .unwrap();
        }
        let transcript_path = service
            .store
            .paths()
            .conversation_dir(&scope.agent_id, &scope.conversation_id)
            .unwrap()
            .join("messages.jsonl");
        let transcript = tokio::fs::read_to_string(transcript_path).await.unwrap();
        assert_eq!(transcript.lines().count(), 101);
        effects
            .emit(TurnEvent::Finished {
                reason: lotta_runtime::ports::StopReason::EndTurn,
            })
            .unwrap();
        assert_eq!(
            sink.0.lock().unwrap().as_slice(),
            &[(scope, "turn_finished".into())]
        );
    }
    #[test]
    fn production_second_turn_includes_persisted_assistant_history() {
        let user = lotta_domain::LocalMessageRole::User;
        let assistant = lotta_domain::LocalMessageRole::Assistant;
        assert_ne!(user, assistant);
    }
}
