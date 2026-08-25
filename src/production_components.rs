//! Concrete production server composition.

use crate::production_setup::{
    ProductionSetupConfig, ProductionSetupPorts, ProductionTurnBrokers, ProductionTurnController,
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
use lotta_app_server::ws::settings::{CwdChange, SettingsBridge};
#[cfg(test)]
use lotta_app_server::ws::skills::SkillsBridge;
use lotta_domain::{
    AgentId, BoundedJsonValue, Clock, InputDisposition, ModelDescriptor, NonEmptyString,
    ProviderStack, QueueItem, QueueItemKind, QueueItemSource, RuntimeScope, SessionEntry,
    SessionEntryType, TranscriptEntry, TranscriptManifest, TranscriptMessageFormat, TurnLease,
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
use lotta_providers::connections::{
    ConnectionAdapterFactory, ConnectionError, ConnectionManager, HostConnectionAdapterFactory,
    ProviderAuthStore,
};
use lotta_providers::host::client::{HostConfig, materialize_host_script};
use lotta_providers::model::ModelHandle;
use lotta_providers::native::NativeAdapterRegistry;
use lotta_runtime::boundary::{ProviderEventText, ProviderName};
use lotta_runtime::ports::{
    AgentStore, ConversationStore, PortFuture, ProviderError, ProviderErrorContext, ProviderEvent,
    ProviderEventSink, ProviderPort, ProviderRequest, ToolExecutionRequest, ToolOutcome, ToolPort,
};
use lotta_runtime::turn::{
    ControlRequest, ControllerToolRequestRecord, SetupError, ToolResultRecord, TurnEffectPort,
    TurnEvent, TurnProjection, TurnStopRecord,
};
use lotta_runtime::{
    AdmissionOutcome, AdmissionRequest, AdmissionRoute, ListenerRuntime, QueueMutation,
    RuntimeError, RuntimeKey, WorkspaceSandbox,
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
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 128_000;
const DEFAULT_OUTPUT_TOKENS: u64 = 4_096;
const TRANSCRIPT_MANIFEST_SCHEMA_VERSION: u8 = 2;
const TRANSCRIPT_SESSION_SCHEMA_VERSION: u8 = 3;

type DeviceSnapshotSource =
    Arc<dyn Fn(&RuntimeScope) -> Result<BoundedJsonValue, AppServerError> + Send + Sync>;

#[cfg(test)]
pub(crate) type CancellationStageObserver =
    Arc<dyn Fn(lotta_runtime::turn::CancelStep) + Send + Sync>;
#[cfg(test)]
type CancellationOperationObserver = Arc<ProductionCancellationObserver>;
#[cfg(not(test))]
type CancellationOperationObserver = ();

/// Owned production dependencies kept alive for the listener lifetime.
pub struct ProductionComponents {
    runtime_service: Arc<ProductionRuntimeService>,
    turn_controller: Arc<ProductionTurnController>,
    post_turn_queue: lotta_store::PostTurnQueue,
    _observer: Arc<lotta_runtime::observe::RuntimeObserver>,
    reflection: Arc<Mutex<Arc<dyn lotta_store::ReflectionJob>>>,
    memory_push: Arc<Mutex<Arc<dyn lotta_store::MemoryPushJob>>>,
    shared: lotta_app_server::listener::SharedGroupBridges,
}

struct RegisteredReflection(Arc<Mutex<Arc<dyn lotta_store::ReflectionJob>>>);

impl lotta_store::ReflectionJob for RegisteredReflection {
    fn reflect(
        &self,
        job: &lotta_store::PostTurnJob,
    ) -> Result<lotta_store::PostTurnExecution, lotta_store::StoreError> {
        self.0
            .lock()
            .map_err(|_| {
                lotta_store::StoreError::new(StoreErrorKind::StorageConflict, "reflection")
            })?
            .reflect(job)
    }
}

struct RegisteredMemoryPush(Arc<Mutex<Arc<dyn lotta_store::MemoryPushJob>>>);

impl lotta_store::MemoryPushJob for RegisteredMemoryPush {
    fn push_memory(
        &self,
        job: &lotta_store::PostTurnJob,
    ) -> Result<lotta_store::PostTurnExecution, lotta_store::StoreError> {
        self.0
            .lock()
            .map_err(|_| {
                lotta_store::StoreError::new(StoreErrorKind::StorageConflict, "memory-push")
            })?
            .push_memory(job)
    }
}

/// Concrete production tooling assembled by [`production_tooling`]: shared
/// store paths, provider port, setup ports, tool port, approval manager, and
/// the shell bundle backing background-process snapshots.
type ProductionTooling = (
    StorePaths,
    ProductionProviderPort,
    Arc<ProductionSetupPorts>,
    Arc<ProductionToolPort>,
    Arc<lotta_runtime::ApprovalManager>,
    Arc<ShellToolBundle>,
);

/// Assembles [`ProductionTooling`] from storage/workspace roots. The store
/// paths are returned too so callers share one store root identity.
fn production_tooling(root: &Path, workspace: &Path) -> Result<ProductionTooling, SetupError> {
    let store_paths = StorePaths::new(root).map_err(adapter)?;
    let provider_runtime = production_provider_runtime(&store_paths, root)?;
    let (models, default_model) = production_catalog(&provider_runtime)?;
    let tool_sandbox: Arc<dyn lotta_tools::builtin::shell::ShellSandbox> =
        Arc::new(OsSandbox::detect(workspace_policy(root, workspace)?));
    let shell = Arc::new(
        ShellToolBundle::new(
            &workspace.canonicalize().map_err(adapter)?,
            production_shell_scope()?,
            Arc::clone(&tool_sandbox),
        )
        .map_err(|_| SetupError::Adapter("shell tool bundle".into()))?,
    );
    let setup = Arc::new(ProductionSetupPorts::new(setup_config(
        root,
        workspace,
        models,
        default_model,
        provider_runtime.connections(),
        shell.as_ref(),
    )?)?);
    let tools = Arc::new(ProductionToolPort::new(
        setup.registry(),
        setup.hook_runtime(),
        root.join("artifacts"),
        Arc::clone(&shell),
    )?);
    let approval_manager = Arc::new(lotta_runtime::ApprovalManager::new(
        LocalStore::new(store_paths.clone()).approval_journal(),
        Arc::new(crate::production_setup::ProductionEditedInputValidator),
    ));
    Ok((
        store_paths,
        provider_runtime,
        setup,
        tools,
        approval_manager,
        shell,
    ))
}

/// Wires the authoritative turn-side ports onto the shared WS bridges:
/// compaction through the registered production service, conversations
/// lifecycle on the same registry turn execution holds, and the device
/// authority ports over that registry.
fn register_turn_authorities(
    shared: &lotta_app_server::listener::SharedGroupBridges,
    provider: &Arc<ProductionProviderPort>,
    setup: &Arc<ProductionSetupPorts>,
    runtime_state: &Arc<ProductionRuntimeState>,
    brokers: &Arc<ProductionTurnBrokers>,
    shell: &Arc<ShellToolBundle>,
) {
    let compaction = Arc::new(crate::production_setup::RegisteredProductionCompaction {
        service: lotta_runtime::CompactionService::new(
            crate::production_setup::ProductionProviderSummarizer {
                provider: Arc::clone(provider),
            },
            crate::production_setup::ProductionCompactionEffects {
                setup: Arc::clone(setup),
                runtime_state: Arc::clone(runtime_state),
            },
        ),
    });
    brokers.register_compaction_service(Some(compaction));
    // The WebSocket conversations group shares the authoritative lifecycle
    // registry (busy-vs-idle compaction) and routes through the registered
    // production compaction service with its real hooks and summarizer.
    shared.register_conversations_authority(Some(Arc::new(ConversationsProductionAuthority {
        state: Arc::clone(runtime_state),
        brokers: Arc::clone(brokers),
    })));
    register_device_authority_ports(shared, runtime_state, setup.mod_registries(), shell);
}

/// The unavailable-by-default post-turn capability slots plus the registered
/// forwarding ports the turn controller observes.
/// The unavailable-by-default reflection/memory-push capability slots plus
/// their registered forwarding ports, assembled by [`post_turn_capabilities`].
type PostTurnCapabilities = (
    Arc<Mutex<Arc<dyn lotta_store::ReflectionJob>>>,
    Arc<Mutex<Arc<dyn lotta_store::MemoryPushJob>>>,
    Arc<dyn lotta_store::ReflectionJob>,
    Arc<dyn lotta_store::MemoryPushJob>,
);

#[must_use]
fn post_turn_capabilities() -> PostTurnCapabilities {
    let reflection: Arc<Mutex<Arc<dyn lotta_store::ReflectionJob>>> = Arc::new(Mutex::new(
        Arc::new(crate::production_setup::UnavailableReflection),
    ));
    let memory_push: Arc<Mutex<Arc<dyn lotta_store::MemoryPushJob>>> = Arc::new(Mutex::new(
        Arc::new(crate::production_setup::UnavailableMemoryPush),
    ));
    let reflection_port: Arc<dyn lotta_store::ReflectionJob> =
        Arc::new(RegisteredReflection(Arc::clone(&reflection)));
    let memory_port: Arc<dyn lotta_store::MemoryPushJob> =
        Arc::new(RegisteredMemoryPush(Arc::clone(&memory_push)));
    (reflection, memory_push, reflection_port, memory_port)
}

impl ProductionComponents {
    /// Builds concrete local production dependencies before listener bind.
    ///
    /// The skills and settings bridges are composed here and shared with the
    /// listener, so WebSocket group mutations reach subsequent turns.
    pub fn from_server(
        prepared: &PreparedServer,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, SetupError> {
        let root = prepared.storage_dir.clone();
        let event_sink: Arc<dyn lotta_runtime::observe::events::RuntimeEventSink> =
            Arc::new(lotta_runtime::observe::events::FanoutRuntimeEventSink::new(
                vec![Arc::new(lotta_telemetry::TracingRuntimeEventSink)],
            ));
        let observer = Arc::new(lotta_runtime::observe::RuntimeObserver::new(event_sink));
        let workspace = prepared.workspace_dir.clone();
        let shared = lotta_app_server::listener::SharedGroupBridges::new(
            &root,
            &workspace,
            Arc::clone(&clock),
        )
        .map_err(|_| SetupError::Adapter("shared group bridge roots".into()))?;
        let (store_paths, provider_runtime, setup, tools, approval_manager, shell) =
            production_tooling(&root, &workspace)?;
        let provider = Arc::new(provider_runtime);
        let runtime_state = Arc::new(ProductionRuntimeState::new(Arc::clone(&observer)));
        let brokers = Arc::new(ProductionTurnBrokers::new());
        register_turn_authorities(&shared, &provider, &setup, &runtime_state, &brokers, &shell);
        let runtime_service = Arc::new(ProductionRuntimeService::new(
            store_paths.clone(),
            Arc::clone(&clock),
            setup.hook_runtime(),
            Arc::clone(&runtime_state),
            Arc::clone(&approval_manager),
            Arc::clone(&brokers),
            shared.settings(),
        ));
        let (reflection, memory_push, reflection_port, memory_port) = post_turn_capabilities();
        let turn_controller = Arc::new(ProductionTurnController::new(
            Arc::clone(&setup),
            Arc::clone(&provider),
            Arc::clone(&tools),
            LocalStore::new(store_paths.clone()),
            Arc::clone(&clock),
            workspace,
            Arc::clone(&runtime_state),
            Arc::clone(&brokers),
            Arc::clone(&approval_manager),
            reflection_port,
            memory_port,
            shared.skills(),
            shared.settings(),
        ));
        let post_turn_queue = lotta_store::PostTurnQueue::new(&LocalStore::new(store_paths));
        let components = Self {
            runtime_service,
            turn_controller,
            post_turn_queue,
            _observer: observer,
            reflection,
            memory_push,
            shared,
        };
        components.drain_post_turn()?;
        Ok(components)
    }

    fn drain_post_turn(&self) -> Result<(), SetupError> {
        let reflection = self
            .reflection
            .lock()
            .map_err(|_| SetupError::Adapter("reflection capability lock".into()))?
            .clone();
        let memory_push = self
            .memory_push
            .lock()
            .map_err(|_| SetupError::Adapter("memory capability lock".into()))?
            .clone();
        lotta_store::PostTurnJobRunner::new(
            &self.post_turn_queue,
            reflection.as_ref(),
            memory_push.as_ref(),
        )
        .drain()
        .map_err(adapter)
    }

    /// Registers the production reflection capability and drains pending jobs.
    #[allow(dead_code, reason = "public production capability API")]
    pub fn register_reflection(
        &self,
        capability: Arc<dyn lotta_store::ReflectionJob>,
    ) -> Result<(), SetupError> {
        *self
            .reflection
            .lock()
            .map_err(|_| SetupError::Adapter("reflection capability lock".into()))? = capability;
        self.drain_post_turn()
    }

    /// Registers the production memory-push capability and drains pending jobs.
    #[allow(dead_code, reason = "public production capability API")]
    pub fn register_memory_push(
        &self,
        capability: Arc<dyn lotta_store::MemoryPushJob>,
    ) -> Result<(), SetupError> {
        *self
            .memory_push
            .lock()
            .map_err(|_| SetupError::Adapter("memory capability lock".into()))? = capability;
        self.drain_post_turn()
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

    /// Returns a handle over the shared skills/settings bridge pair served by
    /// the listener; the underlying Arcs stay identical across clones.
    #[must_use]
    pub fn shared_bridges(&self) -> lotta_app_server::listener::SharedGroupBridges {
        self.shared.clone()
    }
}

fn production_builtins(
    root: &std::path::Path,
    workspace: &std::path::Path,
    workspace_policy: &WorkspacePolicy,
    skill_roots: &SkillRoots,
    shell: &ShellToolBundle,
) -> Result<Vec<ToolRegistration>, SetupError> {
    let file = FileToolBundle::new(workspace, &root.join("artifacts"))
        .map_err(|_| SetupError::Adapter("file tool bundle".into()))?;
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
        shell,
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

fn production_shell_scope() -> Result<RuntimeScope, SetupError> {
    Ok(RuntimeScope::new(
        AgentId::accept("production-agent").map_err(adapter)?,
        lotta_domain::ConversationId::accept("production-conversation").map_err(adapter)?,
        None,
    ))
}

fn workspace_policy(root: &Path, workspace: &Path) -> Result<WorkspacePolicy, SetupError> {
    WorkspacePolicy::new(&WorkspaceSandbox::new(
        workspace.to_path_buf(),
        root.to_path_buf(),
    ))
    .map_err(|_| SetupError::Adapter("workspace policy".into()))
}

fn setup_config(
    root: &std::path::Path,
    workspace: &std::path::Path,
    models: Vec<ModelDescriptor>,
    default_model: ModelHandle,
    connections: Vec<lotta_providers::connections::ConnectionSnapshot>,
    shell: &ShellToolBundle,
) -> Result<ProductionSetupConfig, SetupError> {
    let sandbox = WorkspaceSandbox::new(workspace.to_path_buf(), root.to_path_buf());
    let workspace_policy = WorkspacePolicy::new(&sandbox)
        .map_err(|_| SetupError::Adapter("workspace policy".into()))?;
    let skill_roots = production_skill_roots(root, workspace);
    create_production_directories(root)?;
    let builtins = production_builtins(root, workspace, &workspace_policy, &skill_roots, shell)?;
    let registry = Arc::new(lotta_tools::ToolRegistry::new(builtins).map_err(adapter)?);
    let mod_registries = Arc::new(ModRegistries::new(Arc::clone(&registry)));
    let hook_registry = Arc::new(HookRegistry::new());
    let hook_runtime =
        production_hook_runtime(workspace, &workspace_policy, Arc::clone(&hook_registry))?;
    Ok(ProductionSetupConfig {
        store_paths: StorePaths::new(root).map_err(adapter)?,
        skill_roots,
        models,
        default_model,
        connections,
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
    })
}

fn production_provider_runtime(
    paths: &StorePaths,
    root: &Path,
) -> Result<ProductionProviderPort, SetupError> {
    let config = production_host_config(root)?;
    let owner = lotta_extensions::sidecar::SidecarOwnerIdentity::new(
        "production-provider",
        "production-provider-runtime",
        "production-provider-conversation",
    )
    .map_err(adapter)?;
    let host: Arc<dyn ConnectionAdapterFactory> =
        Arc::new(HostConnectionAdapterFactory::new(config, owner));
    let manager = ConnectionManager::load(ProviderAuthStore::new(paths.clone()))
        .map_err(|_| SetupError::Adapter("provider connection store".into()))?;
    Ok(ProductionProviderPort::new(
        manager,
        NativeAdapterRegistry::production(),
        host,
    ))
}

fn production_host_config(root: &Path) -> Result<HostConfig, SetupError> {
    let bun = std::env::var_os("LOTTA_BUN")
        .map(PathBuf::from)
        .or_else(|| {
            [
                "/opt/homebrew/bin/bun",
                "/usr/local/bin/bun",
                "/usr/bin/bun",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
        })
        .ok_or_else(|| SetupError::Adapter("provider host runtime".into()))?
        .canonicalize()
        .map_err(adapter)?;
    let package = std::env::var_os("LOTTA_PI_AI_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../letta-code/node_modules/@earendil-works/pi-ai")
        })
        .canonicalize()
        .map_err(|_| SetupError::Adapter("provider host package".into()))?;
    let host_root = root.join("provider-host");
    let scratch = host_root.join("scratch");
    let script = host_root.join("pi-ai-host.mjs");
    std::fs::create_dir_all(&scratch).map_err(adapter)?;
    materialize_host_script(&script)
        .map_err(|_| SetupError::Adapter("provider host script".into()))?;
    Ok(HostConfig {
        bun_executable: bun,
        host_script: script.canonicalize().map_err(adapter)?,
        package_root: package,
        scratch_cwd: scratch.canonicalize().map_err(adapter)?,
        test_mode: false,
    })
}

fn production_catalog(
    runtime: &ProductionProviderPort,
) -> Result<(Vec<ModelDescriptor>, ModelHandle), SetupError> {
    let mut models = Vec::new();
    for connection in runtime.connections() {
        let model_id = default_model_id(&connection.provider_type);
        models.push(production_model(
            &connection.id,
            model_id,
            DEFAULT_CONTEXT_WINDOW_TOKENS,
            connection.is_connected,
        )?);
    }
    models.sort_by(|left, right| left.handle.as_str().cmp(right.handle.as_str()));
    if let Some(handle) = models
        .iter()
        .find(|model| model.available)
        .map(|model| model.handle.as_str().to_owned())
    {
        return Ok((models, ModelHandle::from_str(&handle).map_err(adapter)?));
    }
    // Task54 contract: a typed unavailable descriptor is retained only when configuration is empty.
    let unavailable = production_model(
        "unavailable",
        "unavailable",
        DEFAULT_CONTEXT_WINDOW_TOKENS,
        false,
    )?;
    let handle = ModelHandle::from_str(unavailable.handle.as_str()).map_err(adapter)?;
    Ok((vec![unavailable], handle))
}

fn default_model_id(provider_type: &str) -> &str {
    match provider_type {
        "anthropic" => "claude-sonnet-4-5",
        "openai" => "gpt-5.4",
        _ => "default",
    }
}

fn production_model(
    provider_id: &str,
    model_id: &str,
    context_window: u64,
    available: bool,
) -> Result<ModelDescriptor, SetupError> {
    Ok(ModelDescriptor {
        handle: NonEmptyString::new(format!("{provider_id}/{model_id}")).map_err(adapter)?,
        provider_id: NonEmptyString::new(provider_id.to_owned()).map_err(adapter)?,
        available,
        context_window: Some(context_window),
        model_settings: None,
    })
}

fn production_skill_roots(root: &std::path::Path, workspace: &std::path::Path) -> SkillRoots {
    SkillRoots {
        project_working_root: workspace.to_path_buf(),
        agent_skills_directory: Some(root.join("agents")),
        memory_root: Some(root.join("memory")),
        // The pinned global source lives at `<storage>/.letta/skills` — the
        // exact directory the Task 69 skills bridge links enables into.
        global_skills_directory: root.join(".letta").join("skills"),
        bundled_skills_directory: root.join("bundled-skills"),
    }
}

fn create_production_directories(root: &std::path::Path) -> Result<(), SetupError> {
    for path in [
        root.join("artifacts"),
        root.join(".letta").join("skills"),
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

const PENDING_ADMISSIONS_MAX: usize = lotta_domain::bounds::RUNTIMES_MAX.value;

type PendingAdmissionKey = (RuntimeKey, String);

pub(crate) struct PendingAdmission {
    pub(crate) handle: lotta_runtime::RuntimeHandle,
    pub(crate) lease: lotta_domain::TurnLease,
    pub(crate) item: QueueItem,
    pub(crate) cancellation: CancellationToken,
}

pub(crate) struct ActiveAdmission {
    pub(crate) handle: lotta_runtime::RuntimeHandle,
    pub(crate) lease: lotta_domain::TurnLease,
    pub(crate) cancellation: CancellationToken,
    pub(crate) queue: lotta_runtime::ConversationQueue,
    pub(crate) history: lotta_domain::AdmissionHistory,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct ProductionCancellationObserver {
    pub(crate) claims: std::sync::atomic::AtomicU64,
    pub(crate) terminal_persistences: std::sync::atomic::AtomicU64,
    pub(crate) cancelled_events: std::sync::atomic::AtomicU64,
    pub(crate) releases: std::sync::atomic::AtomicU64,
    pub(crate) pumps: std::sync::atomic::AtomicU64,
    pub(crate) order: std::sync::Mutex<Vec<&'static str>>,
}

#[cfg(test)]
impl ProductionCancellationObserver {
    pub(crate) fn record(&self, step: &'static str) {
        let counter = match step {
            "claim" => &self.claims,
            "persist" => &self.terminal_persistences,
            "cancelled" => &self.cancelled_events,
            "release" => &self.releases,
            "pump" => &self.pumps,
            _ => return,
        };
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.order.lock().expect("cancellation observer").push(step);
    }
}

pub(crate) struct RuntimeServiceState {
    pub(crate) registry: ListenerRuntime,
    pub(crate) pending: HashMap<PendingAdmissionKey, PendingAdmission>,
    sequence: u128,
}

pub(crate) struct ProductionRuntimeState {
    pub(crate) inner: tokio::sync::Mutex<RuntimeServiceState>,
    pub(crate) active: std::sync::Mutex<HashMap<RuntimeKey, ActiveAdmission>>,
    #[cfg(test)]
    pub(crate) cancellation_observer: std::sync::Mutex<Option<Arc<ProductionCancellationObserver>>>,
}

impl ProductionRuntimeState {
    fn new(observer: Arc<lotta_runtime::observe::RuntimeObserver>) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(RuntimeServiceState {
                registry: ListenerRuntime::with_observer(observer.as_ref().clone()),
                pending: HashMap::new(),
                sequence: 1,
            }),
            active: std::sync::Mutex::new(HashMap::new()),
            #[cfg(test)]
            cancellation_observer: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn take_pending(
        &self,
        scope: &RuntimeScope,
        id: &str,
    ) -> Result<PendingAdmission, AppServerError> {
        self.inner
            .try_lock()
            .map_err(|_| AppServerError::Internal)?
            .pending
            .remove(&(RuntimeKey::from(scope), id.to_owned()))
            .ok_or(AppServerError::Malformed)
    }

    pub(crate) fn lease_is_current(&self, scope: &RuntimeScope, lease: &TurnLease) -> bool {
        let active_current = self.active.lock().ok().and_then(|active| {
            active
                .get(&RuntimeKey::from(scope))
                .map(|item| item.lease.clone())
        });
        if let Some(current) = active_current {
            return current == *lease;
        }
        // No active admission holds this scope, so the lease can only be a
        // management command begun directly on the authoritative lifecycle
        // registry (for example a WebSocket conversation compaction).
        // ponytail: try_lock only; registry contention is sub-millisecond and
        // treated conservatively as "not current".
        let Ok(state) = self.inner.try_lock() else {
            return false;
        };
        let registry = &state.registry;
        registry
            .lookup(&RuntimeKey::from(scope))
            .and_then(|handle| registry.lifecycle(&handle))
            .is_some_and(|owner| owner.is_current(lease))
    }

    #[cfg(test)]
    pub(crate) fn observe_cancellation(&self, observer: Arc<ProductionCancellationObserver>) {
        *self
            .cancellation_observer
            .lock()
            .expect("cancellation observer") = Some(observer);
    }

    #[cfg(test)]
    pub(crate) fn record_cancellation(&self, step: &'static str) {
        if let Some(observer) = self
            .cancellation_observer
            .lock()
            .expect("cancellation observer")
            .as_ref()
        {
            observer.record(step);
        }
    }

    pub(crate) async fn release_and_pump(
        &self,
        scope: &RuntimeScope,
        pending: PendingAdmission,
        reason: &'static str,
        lifecycle_already_released: bool,
    ) -> Result<Option<(QueueItem, BoundedJsonValue)>, AppServerError> {
        let key = RuntimeKey::from(scope);
        let active = self
            .active
            .lock()
            .map_err(|_| AppServerError::Internal)?
            .remove(&key)
            .ok_or(AppServerError::Malformed)?;
        if active.handle != pending.handle || active.lease != pending.lease {
            return Err(AppServerError::Malformed);
        }
        let mut state = self.inner.lock().await;
        for item in active.queue.items().cloned() {
            let _mutation = state
                .registry
                .enqueue_retained(&pending.handle, item)
                .map_err(runtime_service_error)?;
        }
        let pumped = release_and_pump_locked(
            &mut state,
            scope,
            pending,
            reason,
            lifecycle_already_released,
        )?;
        #[cfg(test)]
        if lifecycle_already_released {
            self.record_cancellation("release");
        }
        #[cfg(test)]
        if pumped.is_some() {
            self.record_cancellation("pump");
        }
        Ok(pumped)
    }
}

/// Bridges the WebSocket conversations group to the authoritative production
/// runtime: command leases are begun on the same lifecycle registry the turn
/// path uses, and compaction routes through the registered production service.
pub(crate) struct ConversationsProductionAuthority {
    pub(crate) state: Arc<ProductionRuntimeState>,
    pub(crate) brokers: Arc<ProductionTurnBrokers>,
}

impl ConversationsProductionAuthority {
    fn begin_on_registry(&self, scope: &RuntimeScope) -> Result<TurnLease, ()> {
        let mut state = self.state.inner.try_lock().map_err(|_| ())?;
        let owner = Uuid::from_u128(state.sequence);
        state.sequence = state.sequence.checked_add(1).ok_or(())?;
        let handle = state.registry.get_or_create(scope, owner).map_err(|_| ())?;
        let owner_lifecycle = state.registry.lifecycle_mut(&handle).map_err(|_| ())?;
        owner_lifecycle.begin_command().map_err(|_| ())
    }

    /// Awaits the authoritative registry so the release completes even while
    /// an unrelated operation holds it busy; a skipped release would strand
    /// the conversation in the command state forever.
    async fn finish_on_registry(
        &self,
        scope: &RuntimeScope,
        lease: &TurnLease,
    ) -> Result<(), RuntimeError> {
        let mut state = self.state.inner.lock().await;
        let handle =
            state
                .registry
                .lookup(&RuntimeKey::from(scope))
                .ok_or(RuntimeError::NotFound {
                    context: "conversation lifecycle registry lookup".into(),
                })?;
        state.registry.lifecycle_mut(&handle)?.finish_command(lease)
    }
}

impl lotta_app_server::ws::conversations::ConversationAuthority
    for ConversationsProductionAuthority
{
    fn begin_command(
        &self,
        scope: &RuntimeScope,
    ) -> Result<TurnLease, lotta_app_server::ws::conversations::CommandLeaseUnavailable> {
        self.begin_on_registry(scope)
            .map_err(|()| lotta_app_server::ws::conversations::CommandLeaseUnavailable)
    }

    fn finish_command<'a>(
        &'a self,
        scope: &'a RuntimeScope,
        lease: &'a TurnLease,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + 'a>> {
        Box::pin(self.finish_on_registry(scope, lease))
    }

    fn compact(
        &self,
        command: lotta_runtime::CompactionCommand,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<lotta_runtime::turn::CompactionProgress, RuntimeError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            let service = self
                .brokers
                .compaction
                .lock()
                .map_err(|_| RuntimeError::Conflict {
                    context: "compaction broker lock".into(),
                })?
                .clone()
                .ok_or(RuntimeError::CompactionUnavailable)?;
            service.compact(command).await
        })
    }
}

/// Registers the device-group authority ports on the shared bridges: queue
/// removals route through the authoritative runtime registry, `execute_command`
/// resolves through the canonical Task 45 registries, and background snapshots
/// reflect real shell sessions.
fn register_device_authority_ports(
    shared: &lotta_app_server::listener::SharedGroupBridges,
    state: &Arc<ProductionRuntimeState>,
    mod_registries: Arc<ModRegistries>,
    shell: &Arc<ShellToolBundle>,
) {
    shared.register_queue_authority(Some(Arc::new(ProductionQueueAuthority {
        state: Arc::clone(state),
    })));
    shared.register_mod_commands(Some(mod_registries));
    shared.register_background_processes(Some(Arc::new(ShellBackgroundProcesses {
        shell: Arc::clone(shell),
    })));
}

/// Bridges the WebSocket device group to the authoritative production runtime:
/// queue removals mutate the exact `ListenerRuntime` registry the turn path
/// uses, and items queued onto an in-flight turn's private admission queue —
/// so a WS-removed item can never be pumped by an active turn afterward.
pub(crate) struct ProductionQueueAuthority {
    pub(crate) state: Arc<ProductionRuntimeState>,
}

impl lotta_app_server::ws::device::QueueAuthority for ProductionQueueAuthority {
    fn remove_queued<'a>(
        &'a self,
        scope: &'a RuntimeScope,
        item_id: &'a NonEmptyString,
    ) -> Pin<Box<dyn Future<Output = Result<Option<QueueMutation>, RuntimeError>> + Send + 'a>>
    {
        Box::pin(async move {
            // Inputs admitted while a turn runs sit on that turn's active
            // admission queue, guarded by this std mutex independently of the
            // registry lock the executing turn holds. Cancelling here deletes
            // the item before the post-turn transfer can hand it to the
            // registry pump, so the removal stays authoritative mid-turn.
            let active_cancel = match self
                .state
                .active
                .lock()
                .map_err(|_| RuntimeError::Conflict {
                    context: "device queue active admission".into(),
                })?
                .get_mut(&RuntimeKey::from(scope))
            {
                Some(admission) => admission.queue.cancel(item_id)?,
                None => None,
            };
            if let Some(mutation) = active_cancel {
                return Ok(Some(mutation));
            }
            let mut state = self.state.inner.lock().await;
            let handle =
                state
                    .registry
                    .lookup(&RuntimeKey::from(scope))
                    .ok_or(RuntimeError::NotFound {
                        context: "device queue registry lookup".into(),
                    })?;
            state.registry.cancel_queued(&handle, item_id)
        })
    }
}

/// Device-status source over the production shell process manager: snapshots
/// reflect actual background sessions instead of a hardcoded empty list.
pub(crate) struct ShellBackgroundProcesses {
    pub(crate) shell: Arc<ShellToolBundle>,
}

impl lotta_app_server::ws::device::BackgroundProcessSource for ShellBackgroundProcesses {
    fn snapshot(&self) -> Vec<lotta_app_server::ws::device::BackgroundProcessSummary> {
        self.shell
            .background_snapshot()
            .into_iter()
            .map(|session| {
                lotta_app_server::ws::device::BackgroundProcessSummary::Bash(
                    lotta_app_server::ws::device::BashBackgroundProcessSummary {
                        process_id: session.process_id,
                        command: session.command,
                        started_at_ms: Some(session.started_at_ms),
                        status: session.status.to_owned(),
                        exit_code: session.exit_code,
                    },
                )
            })
            .collect()
    }
}

pub(crate) struct ProductionRuntimeService {
    store: LocalStore,
    clock: Arc<dyn Clock + Send + Sync>,
    hooks: Arc<dyn lotta_runtime::hooks::HookRuntime>,
    state: Arc<ProductionRuntimeState>,
    approvals: Arc<lotta_runtime::ApprovalManager>,
    brokers: Arc<ProductionTurnBrokers>,
    settings: Arc<SettingsBridge>,
    device_snapshot: Mutex<Option<DeviceSnapshotSource>>,
}

impl ProductionRuntimeService {
    fn new(
        store_paths: StorePaths,
        clock: Arc<dyn Clock + Send + Sync>,
        hooks: Arc<dyn lotta_runtime::hooks::HookRuntime>,
        state: Arc<ProductionRuntimeState>,
        approvals: Arc<lotta_runtime::ApprovalManager>,
        brokers: Arc<ProductionTurnBrokers>,
        settings: Arc<SettingsBridge>,
    ) -> Self {
        Self {
            store: LocalStore::new(store_paths),
            clock,
            hooks,
            state,
            approvals,
            brokers,
            settings,
            device_snapshot: Mutex::new(None),
        }
    }

    fn active_snapshot(
        &self,
        key: &RuntimeKey,
    ) -> Result<Option<(TurnLease, Vec<QueueItem>)>, AppServerError> {
        self.state
            .active
            .lock()
            .map_err(|_| AppServerError::Internal)
            .map(|active| {
                active.get(key).map(|item| {
                    (
                        item.lease.clone(),
                        item.queue.items().cloned().collect::<Vec<_>>(),
                    )
                })
            })
    }

    fn device_snapshot(&self, scope: &RuntimeScope) -> Result<BoundedJsonValue, AppServerError> {
        let source = self
            .device_snapshot
            .lock()
            .map_err(|_| AppServerError::Internal)?
            .clone()
            .ok_or(AppServerError::Unavailable)?;
        source(scope)
    }

    async fn compact(
        &self,
        scope: RuntimeScope,
        mode: lotta_runtime::CompactionMode,
        request_id: NonEmptyString,
        request: lotta_runtime::ports::ProviderRequest,
    ) -> Result<lotta_runtime::turn::CompactionProgress, AppServerError> {
        let active = self
            .state
            .active
            .lock()
            .map_err(|_| AppServerError::Internal)?
            .get(&RuntimeKey::from(&scope))
            .map(|active| (active.lease.clone(), active.cancellation.clone()))
            .ok_or(AppServerError::Malformed)?;
        if !self.state.lease_is_current(&scope, &active.0) {
            return Err(AppServerError::Malformed);
        }
        let service = self
            .brokers
            .compaction
            .lock()
            .map_err(|_| AppServerError::Internal)?
            .clone()
            .ok_or(AppServerError::Unavailable)?;
        let estimated = lotta_runtime::ports::estimate_request_tokens(&request);
        let detail = lotta_runtime::ports::ProviderContextOverflowDetail {
            measured: None,
            estimated,
            limit: request.context_tokens_max.get(),
            provider: request.model.provider_id.as_str().to_owned(),
            model: request.model.handle.as_str().to_owned(),
            attempt: 1,
            compactions_completed: 0,
        };
        service
            .compact(lotta_runtime::CompactionCommand {
                request_id,
                scope,
                lease: active.0,
                request,
                detail,
                trigger: lotta_runtime::CompactionTrigger::Manual,
                mode,
                cancellation: active.1,
            })
            .await
            .map_err(|_| AppServerError::Internal)
    }

    fn ensure_runtime(
        &self,
        scope: &RuntimeScope,
    ) -> Result<(lotta_runtime::RuntimeHandle, bool), AppServerError> {
        let mut state = self
            .state
            .inner
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
        let handle = state
            .registry
            .get_or_create(scope, owner)
            .map_err(runtime_service_error)?;
        drop(state);
        if !existed {
            self.approvals
                .restart(scope)
                .map_err(runtime_service_error)?;
            if self
                .approvals
                .residency_count(scope)
                .map_err(runtime_service_error)?
                > 0
            {
                self.update_residency(scope, &handle)?;
            }
        }
        Ok((handle, !existed))
    }

    fn update_residency(
        &self,
        scope: &RuntimeScope,
        handle: &lotta_runtime::RuntimeHandle,
    ) -> Result<(), AppServerError> {
        let count = self
            .approvals
            .residency_count(scope)
            .map_err(runtime_service_error)?;
        let mut state = self
            .state
            .inner
            .try_lock()
            .map_err(|_| AppServerError::Internal)?;
        state
            .registry
            .set_residency(
                handle,
                lotta_runtime::RuntimeResidency::new(count, false, 0),
            )
            .map(|_| ())
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

    fn admit_approval_response(
        &self,
        command: &InputCommand,
    ) -> Result<InputAdmission, AppServerError> {
        let payload = command.payload.as_value();
        let request = self.pending_approval(command, payload)?;
        let decision = approval_decision(payload)?;
        let item = self.approval_queue_item(command, &request)?;
        let (outcome, disposition) =
            self.admit_approval_control(command, &request, &decision, payload, item)?;
        let continuation = approval_continuation(outcome, &request, decision, payload)?;
        Ok(InputAdmission {
            disposition,
            error: None,
            continuation,
            after_ack: RuntimeEventBatch::new(Vec::new()).map_err(|_| AppServerError::Internal)?,
        })
    }

    fn pending_approval(
        &self,
        command: &InputCommand,
        payload: &serde_json::Value,
    ) -> Result<lotta_runtime::ApprovalRequest, AppServerError> {
        let request_id = payload
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .ok_or(AppServerError::Malformed)?;
        let request_id =
            NonEmptyString::new(request_id.to_owned()).map_err(|_| AppServerError::Malformed)?;
        let request = self
            .approvals
            .get_request(&command.runtime, &request_id)
            .map_err(runtime_service_error)?
            .ok_or(AppServerError::Malformed)?;
        if request.state != lotta_runtime::ApprovalState::Pending {
            return Err(AppServerError::Malformed);
        }
        Ok(request)
    }

    fn approval_queue_item(
        &self,
        command: &InputCommand,
        request: &lotta_runtime::ApprovalRequest,
    ) -> Result<QueueItem, AppServerError> {
        let submission_id = command
            .request_id
            .as_ref()
            .map(NonEmptyString::as_str)
            .unwrap_or(request.tool_call_id.as_str());
        let client_message_id = format!("approval-{submission_id}");
        Ok(QueueItem {
            id: NonEmptyString::new(format!("queue-{client_message_id}"))
                .map_err(|_| AppServerError::Malformed)?,
            client_message_id: NonEmptyString::new(client_message_id)
                .map_err(|_| AppServerError::Malformed)?,
            kind: QueueItemKind::ApprovalResult,
            source: QueueItemSource::System,
            content: command.payload.clone(),
            enqueued_at: self.clock.now(),
            extras: Default::default(),
        })
    }

    fn admit_approval_control(
        &self,
        command: &InputCommand,
        request: &lotta_runtime::ApprovalRequest,
        decision: &serde_json::Value,
        payload: &serde_json::Value,
        item: QueueItem,
    ) -> Result<(AdmissionOutcome, InputDisposition), AppServerError> {
        let key = RuntimeKey::from(&command.runtime);
        let active_state = self
            .state
            .active
            .lock()
            .map_err(|_| AppServerError::Internal)?;
        let active = active_state.get(&key).ok_or(AppServerError::Malformed)?;
        if request.lease_generation != active.lease.generation() {
            return Err(AppServerError::Malformed);
        }
        let resolution = approval_resolution_input(&command.runtime, request, decision, payload)?;
        self.approvals
            .validate_resolution(&resolution, active.lease.generation())
            .map_err(runtime_service_error)?;
        let outcome = lotta_runtime::admit_control_snapshot(
            &active.handle,
            &active.handle,
            &active.lease,
            AdmissionRequest {
                item,
                route: AdmissionRoute::Control(active.lease.clone()),
            },
        )
        .map_err(runtime_service_error)?;
        let disposition = outcome.disposition();
        Ok((outcome, disposition))
    }

    fn admit_active_ordinary(
        &self,
        command: &InputCommand,
    ) -> Result<Option<InputAdmission>, AppServerError> {
        let key = RuntimeKey::from(&command.runtime);
        let mut active = self
            .state
            .active
            .lock()
            .map_err(|_| AppServerError::Internal)?;
        let Some(active) = active.get_mut(&key) else {
            return Ok(None);
        };
        let client_message_id = input_client_message_id(command.payload.as_value())?;
        let client_message_id =
            NonEmptyString::new(client_message_id).map_err(|_| AppServerError::Malformed)?;
        if let Some(prior) = active.history.prior(&client_message_id) {
            return Ok(Some(input_admission(prior, None)?));
        }
        let item = self.admission_item(command, client_message_id.as_str())?;
        let mutation = active.queue.enqueue(item).map_err(runtime_service_error)?;
        let disposition = if matches!(
            mutation.event(),
            lotta_runtime::QueueMutationEvent::Dropped(_, _)
        ) {
            InputDisposition::Rejected
        } else {
            InputDisposition::Queued
        };
        let _recorded = active.history.admit(&client_message_id, disposition);
        Ok(Some(input_admission(disposition, None)?))
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
fn continuation_kind(value: &BoundedJsonValue) -> Option<&str> {
    value
        .as_value()
        .get("kind")
        .and_then(serde_json::Value::as_str)
}

fn approval_resolution_from_continuation(
    scope: RuntimeScope,
    continuation: &BoundedJsonValue,
) -> Result<lotta_runtime::ApprovalResolutionInput, AppServerError> {
    let value = continuation.as_value();
    let text = |name| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or(AppServerError::Malformed)
    };
    let lease_generation = value
        .get("lease_generation")
        .and_then(serde_json::Value::as_u64)
        .ok_or(AppServerError::Malformed)?;
    let revision = value
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or(AppServerError::Malformed)?;
    let decision = value.get("decision").ok_or(AppServerError::Malformed)?;
    let resolution = match decision.get("behavior").and_then(serde_json::Value::as_str) {
        Some("allow") => lotta_runtime::ApprovalResolution::Allow,
        Some("deny") => lotta_runtime::ApprovalResolution::Deny,
        _ => return Err(AppServerError::Malformed),
    };
    let edited_input = value
        .get("updated_input")
        .filter(|candidate| !candidate.is_null())
        .cloned()
        .map(BoundedJsonValue::new)
        .transpose()
        .map_err(|_| AppServerError::Malformed)?;
    Ok(lotta_runtime::ApprovalResolutionInput {
        scope,
        request_id: NonEmptyString::new(text("request_id")?.to_owned())
            .map_err(|_| AppServerError::Malformed)?,
        tool_call_id: NonEmptyString::new(text("tool_call_id")?.to_owned())
            .map_err(|_| AppServerError::Malformed)?,
        lease_generation,
        revision,
        resolution,
        edited_input,
    })
}

fn approval_decision(payload: &serde_json::Value) -> Result<serde_json::Value, AppServerError> {
    if let Some(error) = payload.get("error") {
        Ok(serde_json::json!({"behavior":"deny", "message": error}))
    } else {
        payload
            .get("decision")
            .cloned()
            .ok_or(AppServerError::Malformed)
    }
}

fn approval_continuation(
    outcome: AdmissionOutcome,
    request: &lotta_runtime::ApprovalRequest,
    decision: serde_json::Value,
    payload: &serde_json::Value,
) -> Result<Option<BoundedJsonValue>, AppServerError> {
    match outcome {
        AdmissionOutcome::Control(_) => BoundedJsonValue::new(serde_json::json!({
            "kind": "approval_response",
            "request_id": request.request_id.as_str(),
            "tool_call_id": request.tool_call_id.as_str(),
            "lease_generation": request.lease_generation,
            "revision": request.revision,
            "decision": decision,
            "updated_input": payload
                .get("decision")
                .and_then(|value| value.get("updated_input"))
                .cloned()
        }))
        .map(Some)
        .map_err(|_| AppServerError::Malformed),
        AdmissionOutcome::Duplicate(_) => Ok(None),
        _ => Err(AppServerError::Malformed),
    }
}

fn approval_resolution_input(
    scope: &RuntimeScope,
    request: &lotta_runtime::ApprovalRequest,
    decision: &serde_json::Value,
    payload: &serde_json::Value,
) -> Result<lotta_runtime::ApprovalResolutionInput, AppServerError> {
    let resolution = match decision.get("behavior").and_then(serde_json::Value::as_str) {
        Some("allow") => lotta_runtime::ApprovalResolution::Allow,
        Some("deny") => lotta_runtime::ApprovalResolution::Deny,
        _ => return Err(AppServerError::Malformed),
    };
    let edited_input = payload
        .get("decision")
        .and_then(|value| value.get("updated_input"))
        .filter(|candidate| !candidate.is_null())
        .cloned()
        .map(BoundedJsonValue::new)
        .transpose()
        .map_err(|_| AppServerError::Malformed)?;
    Ok(lotta_runtime::ApprovalResolutionInput {
        scope: scope.clone(),
        request_id: request.request_id.clone(),
        tool_call_id: request.tool_call_id.clone(),
        lease_generation: request.lease_generation,
        revision: request.revision,
        resolution,
        edited_input,
    })
}

fn release_and_pump_locked(
    state: &mut RuntimeServiceState,
    scope: &RuntimeScope,
    pending: PendingAdmission,
    reason: &'static str,
    lifecycle_already_released: bool,
) -> Result<Option<(QueueItem, BoundedJsonValue)>, AppServerError> {
    let current_key = (
        RuntimeKey::from(scope),
        pending.item.client_message_id.as_str().to_owned(),
    );
    if !lifecycle_already_released && let Err(error) = finish_released_turn(state, &pending, reason)
    {
        return restore_failed_release(state, current_key, pending, error);
    }
    pending.cancellation.cancel();
    pump_retained_input(state, scope, pending)
}

fn finish_released_turn(
    state: &mut RuntimeServiceState,
    pending: &PendingAdmission,
    reason: &'static str,
) -> Result<(), AppServerError> {
    let stop = lotta_domain::StopReason::new(reason).map_err(|_| AppServerError::Internal)?;
    let lifecycle_state = state
        .registry
        .lifecycle(&pending.handle)
        .ok_or(AppServerError::Internal)?
        .projection()
        .state();
    if lifecycle_state == lotta_domain::TurnStateKind::Idle {
        return Ok(());
    }
    state
        .registry
        .lifecycle_mut(&pending.handle)
        .map_err(runtime_service_error)
        .and_then(|owner| {
            owner
                .finish_turn(&pending.lease, stop)
                .map_err(runtime_service_error)
        })
}

fn restore_failed_release(
    state: &mut RuntimeServiceState,
    key: PendingAdmissionKey,
    pending: PendingAdmission,
    error: AppServerError,
) -> Result<Option<(QueueItem, BoundedJsonValue)>, AppServerError> {
    match state.pending.entry(key) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(pending);
            Err(error)
        }
        std::collections::hash_map::Entry::Occupied(_) => {
            Err(attach_cleanup(error, AppServerError::Internal))
        }
    }
}

fn pump_retained_input(
    state: &mut RuntimeServiceState,
    scope: &RuntimeScope,
    pending: PendingAdmission,
) -> Result<Option<(QueueItem, BoundedJsonValue)>, AppServerError> {
    let Some(peeked) = state.registry.peek_queue(&pending.handle).cloned() else {
        return Ok(None);
    };
    let candidate = prepare_pump(state, scope, &pending, &peeked)?;
    let Some(item) = state
        .registry
        .pump_one_queue(&pending.handle)
        .map_err(runtime_service_error)?
    else {
        return Err(AppServerError::Internal);
    };
    debug_assert_eq!(item.id, peeked.id);
    install_pumped_input(state, pending, item, candidate)
}

struct PumpCandidate {
    key: PendingAdmissionKey,
    next_sequence: u128,
    run_id: lotta_domain::RunId,
    continuation: BoundedJsonValue,
    id: String,
}

fn prepare_pump(
    state: &RuntimeServiceState,
    scope: &RuntimeScope,
    pending: &PendingAdmission,
    peeked: &QueueItem,
) -> Result<PumpCandidate, AppServerError> {
    if state.pending.len() >= PENDING_ADMISSIONS_MAX {
        return Err(AppServerError::Unavailable);
    }
    let id = peeked.client_message_id.as_str().to_owned();
    let key = (RuntimeKey::from(scope), id.clone());
    if state.pending.contains_key(&key) {
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
    debug_assert!(state.registry.peek_queue(&pending.handle).is_some());
    Ok(PumpCandidate {
        key,
        next_sequence,
        run_id,
        continuation,
        id,
    })
}

fn install_pumped_input(
    state: &mut RuntimeServiceState,
    pending: PendingAdmission,
    item: QueueItem,
    candidate: PumpCandidate,
) -> Result<Option<(QueueItem, BoundedJsonValue)>, AppServerError> {
    let lease = match state
        .registry
        .lifecycle_mut(&pending.handle)
        .map_err(runtime_service_error)
        .and_then(|owner| {
            owner
                .begin_turn(format!("admission-{}", candidate.id), candidate.run_id)
                .map_err(runtime_service_error)
        }) {
        Ok(lease) => lease,
        Err(error) => {
            state.registry.rollback_pump_one(&pending.handle, item);
            return Err(error);
        }
    };
    match state.pending.entry(candidate.key) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(PendingAdmission {
                handle: pending.handle,
                lease,
                item: item.clone(),
                cancellation: CancellationToken::new(),
            });
            state.sequence = candidate.next_sequence;
            Ok(Some((item, candidate.continuation)))
        }
        std::collections::hash_map::Entry::Occupied(_) => {
            rollback_pump_collision(state, &pending.handle, &lease, item)
        }
    }
}

fn rollback_pump_collision(
    state: &mut RuntimeServiceState,
    handle: &lotta_runtime::RuntimeHandle,
    lease: &lotta_domain::TurnLease,
    item: QueueItem,
) -> Result<Option<(QueueItem, BoundedJsonValue)>, AppServerError> {
    let stop = lotta_domain::StopReason::new("admission_collision")
        .map_err(|_| AppServerError::Internal)?;
    let finish = state
        .registry
        .lifecycle_mut(handle)
        .map_err(runtime_service_error)
        .and_then(|owner| {
            owner
                .finish_turn(lease, stop)
                .map_err(runtime_service_error)
        });
    state.registry.rollback_pump_one(handle, item);
    match finish {
        Ok(()) => Err(AppServerError::Internal),
        Err(cleanup) => Err(attach_cleanup(AppServerError::Internal, cleanup)),
    }
}

fn input_admission(
    disposition: InputDisposition,
    error: Option<NonEmptyString>,
) -> Result<InputAdmission, AppServerError> {
    Ok(InputAdmission {
        disposition,
        error,
        continuation: None,
        after_ack: RuntimeEventBatch::new(Vec::new()).map_err(|_| AppServerError::Internal)?,
    })
}

fn attach_cleanup(primary: AppServerError, cleanup: AppServerError) -> AppServerError {
    AppServerError::CleanupAttached {
        primary: Box::new(primary),
        cleanup: Box::new(cleanup),
    }
}

fn resolve_approval_continuation(
    service: &ProductionRuntimeService,

    scope: RuntimeScope,
    continuation: &BoundedJsonValue,
) -> Result<(), AppServerError> {
    let input = approval_resolution_from_continuation(scope, continuation)?;
    service
        .approvals
        .resolve(&input, input.lease_generation)
        .map(|_| ())
        .map_err(runtime_service_error)
}

fn emit_started_continuation(
    service: &ProductionRuntimeService,

    scope: &RuntimeScope,
    continuation: &BoundedJsonValue,
    sink: &dyn RuntimeEventSink,
) -> Result<(), AppServerError> {
    let id = continuation
        .as_value()
        .get("client_message_id")
        .and_then(serde_json::Value::as_str)
        .ok_or(AppServerError::Malformed)?;
    let key = (RuntimeKey::from(scope), id.to_owned());
    let _item = service
        .state
        .inner
        .try_lock()
        .map_err(|_| AppServerError::Internal)?
        .pending
        .get(&key)
        .map(|pending| pending.item.clone())
        .ok_or(AppServerError::Malformed)?;
    sink.emit(scope, loop_event("EXECUTING_COMMAND", Vec::new())?)
}

async fn sync_outcome(
    service: &ProductionRuntimeService,
    command: SyncCommand,
) -> Result<SyncOutcome, AppServerError> {
    let key = RuntimeKey::from(&command.runtime);
    let active = service.active_snapshot(&key)?;
    if active.is_none() {
        let _ = service.ensure_runtime(&command.runtime)?;
    }
    let device = service.device_snapshot(&command.runtime)?;
    let mut broadcasts = vec![lotta_app_server::ws::RuntimeEvent::UpdateDeviceStatus {
        device_status: device,
    }];
    broadcasts.extend(sync_status_broadcasts(service, &key, active.as_ref())?);
    broadcasts.push(lotta_app_server::ws::RuntimeEvent::UpdateSubagentState {
        subagents: BoundedJsonValue::new(serde_json::json!([]))
            .map_err(|_| AppServerError::Internal)?,
    });
    broadcasts.extend(sync_approval_broadcasts(service, &command.runtime)?);
    Ok(SyncOutcome {
        broadcasts: RuntimeEventBatch::new(broadcasts).map_err(|_| AppServerError::Internal)?,
    })
}

fn sync_status_broadcasts(
    service: &ProductionRuntimeService,
    key: &RuntimeKey,
    active: Option<&(lotta_domain::TurnLease, Vec<QueueItem>)>,
) -> Result<Vec<lotta_app_server::ws::RuntimeEvent>, AppServerError> {
    if let Some((active_lease, queue)) = active {
        let _ = active_lease;
        return Ok(vec![
            loop_event("EXECUTING_COMMAND", Vec::new())?,
            queue_event(queue)?,
        ]);
    }
    let Ok(state) = service.state.inner.try_lock() else {
        return Ok(Vec::new());
    };
    let Some(handle) = state.registry.lookup(key) else {
        return Ok(Vec::new());
    };
    let lifecycle = state
        .registry
        .lifecycle(&handle)
        .ok_or(AppServerError::Internal)?
        .projection();
    let queue = state
        .registry
        .queue(&handle)
        .ok_or(AppServerError::Internal)?;
    Ok(vec![
        loop_event(
            match lifecycle.loop_status() {
                lotta_domain::LoopStatus::WaitingOnInput => "WAITING_ON_INPUT",
                lotta_domain::LoopStatus::ExecutingCommand => "EXECUTING_COMMAND",
                lotta_domain::LoopStatus::SendingApiRequest => "SENDING_API_REQUEST",
            },
            lifecycle
                .active_run_ids()
                .iter()
                .map(|run_id| run_id.as_str().to_owned())
                .collect(),
        )?,
        queue_event(&queue.items().cloned().collect::<Vec<_>>())?,
    ])
}

fn loop_event(
    status: &'static str,
    active_run_ids: Vec<String>,
) -> Result<lotta_app_server::ws::RuntimeEvent, AppServerError> {
    let loop_status = BoundedJsonValue::new(serde_json::json!({
        "status": status,
        "active_run_ids": active_run_ids,
        "executing_tool_call_ids": [],
    }))
    .map_err(|_| AppServerError::Internal)?;
    Ok(lotta_app_server::ws::RuntimeEvent::UpdateLoopStatus { loop_status })
}

fn queue_event(items: &[QueueItem]) -> Result<lotta_app_server::ws::RuntimeEvent, AppServerError> {
    let queue =
        BoundedJsonValue::new(serde_json::to_value(items).map_err(|_| AppServerError::Internal)?)
            .map_err(|_| AppServerError::Internal)?;
    let removed =
        BoundedJsonValue::new(serde_json::json!([])).map_err(|_| AppServerError::Internal)?;
    Ok(lotta_app_server::ws::RuntimeEvent::UpdateQueue { queue, removed })
}

fn sync_approval_broadcasts(
    service: &ProductionRuntimeService,
    scope: &RuntimeScope,
) -> Result<Vec<lotta_app_server::ws::RuntimeEvent>, AppServerError> {
    let mut broadcasts = Vec::new();
    for action in lotta_runtime::ApprovalRecovery::new(service.store.approval_journal())
        .reconnect(scope)
        .map_err(runtime_service_error)?
    {
        let lotta_runtime::RecoveryAction::Replay(request) = action else {
            continue;
        };
        let payload = BoundedJsonValue::new(serde_json::json!({
            "subtype": "can_use_tool",
            "tool_call_id": request.tool_call_id.as_str(),
            "tool_name": request.tool_name.as_str(),
            "input": request.original_input.as_value(),
            "permission_suggestions": [],
            "blocked_path": null,
        }))
        .map_err(|_| AppServerError::Internal)?;
        broadcasts.push(lotta_app_server::ws::RuntimeEvent::ControlRequest {
            request_id: request.request_id,
            request: payload,
            agent_id: NonEmptyString::new(scope.agent_id.as_str().to_owned()).ok(),
            conversation_id: NonEmptyString::new(scope.conversation_id.as_str().to_owned()).ok(),
        });
    }
    Ok(broadcasts)
}

impl RuntimeCommandService for ProductionRuntimeService {
    fn register_device_snapshot_source(&self, source: DeviceSnapshotSource) {
        if let Ok(mut current) = self.device_snapshot.lock() {
            *current = Some(source);
        }
    }

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
            let (_handle, created_runtime) = self.ensure_runtime(&runtime)?;
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
            if command
                .payload
                .as_value()
                .get("kind")
                .and_then(serde_json::Value::as_str)
                == Some("approval_response")
            {
                return self.admit_approval_response(&command);
            }
            if let Some(admission) = self.admit_active_ordinary(&command)? {
                return Ok(admission);
            }
            let _ = self.ensure_runtime(&command.runtime)?;
            let client_message_id = input_client_message_id(command.payload.as_value())?;
            let item = self.admission_item(&command, &client_message_id)?;
            let mut state = self
                .state
                .inner
                .try_lock()
                .map_err(|_| AppServerError::Internal)?;
            let handle = state
                .registry
                .lookup(&RuntimeKey::from(&command.runtime))
                .ok_or(AppServerError::Internal)?;
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
            if continuation_kind(&continuation) == Some("approval_response") {
                return resolve_approval_continuation(self, scope, &continuation);
            }
            emit_started_continuation(self, &scope, &continuation, sink.as_ref())
        })
    }

    fn compact(
        &self,
        scope: RuntimeScope,
        mode: lotta_runtime::CompactionMode,
        request_id: NonEmptyString,
        request: lotta_runtime::ports::ProviderRequest,
    ) -> ServiceFuture<'_, lotta_runtime::turn::CompactionProgress> {
        Box::pin(ProductionRuntimeService::compact(
            self, scope, mode, request_id, request,
        ))
    }

    fn sync(&self, command: SyncCommand) -> ServiceFuture<'_, SyncOutcome> {
        Box::pin(async move { sync_outcome(self, command).await })
    }

    fn abort_message(&self, command: AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome> {
        Box::pin(async move {
            let key = RuntimeKey::from(&command.runtime);
            let active = self
                .state
                .active
                .lock()
                .map_err(|_| AppServerError::Internal)?
                .get(&key)
                .map(|active| (active.lease.clone(), active.cancellation.clone()));
            if let Some((active_lease, active_cancellation)) = active {
                if let Some(request) = self
                    .approvals
                    .list_requests(&command.runtime)
                    .map_err(runtime_service_error)?
                    .into_iter()
                    .find(|request| {
                        request.state == lotta_runtime::ApprovalState::Pending
                            && request.lease_generation == active_lease.generation()
                            && command
                                .run_id
                                .as_ref()
                                .is_none_or(|run_id| run_id == &request.run_id)
                    })
                {
                    let _ = self
                        .approvals
                        .abort(&request)
                        .map_err(runtime_service_error)?;
                }
                active_cancellation.cancel();
                return Ok(AbortOutcome { aborted: true });
            }
            let _ = self.ensure_runtime(&command.runtime)?;
            let mut state = self.state.inner.lock().await;
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
            let stop = lotta_domain::StopReason::new("user_cancelled")
                .map_err(|_| AppServerError::Internal)?;
            let lifecycle = state
                .registry
                .lifecycle_mut(&pending.handle)
                .map_err(runtime_service_error)?;
            lifecycle
                .request_cancellation(&pending.lease)
                .map_err(runtime_service_error)?;
            lifecycle
                .finish_turn(&pending.lease, stop)
                .map_err(runtime_service_error)?;
            pending.cancellation.cancel();
            Ok(AbortOutcome { aborted: true })
        })
    }

    fn change_device_state(
        &self,
        command: ChangeDeviceStateCommand,
    ) -> ServiceFuture<'_, DeviceStateOutcome> {
        Box::pin(async move {
            let _ = self.ensure_runtime(&command.runtime)?;
            // A cwd change persists into the shared settings bridge's scoped
            // map so the next turn resolves the new directory. No websocket
            // connection owns this service-level command, so the status
            // snapshot targets the unowned sentinel connection and only the
            // broadcast below reaches subscribers.
            if let Some(cwd) = command.payload.cwd.as_deref() {
                let agent_id = command
                    .payload
                    .agent_id
                    .as_ref()
                    .map(|id| id.as_str().to_owned())
                    .or_else(|| Some(command.runtime.agent_id.as_str().to_owned()));
                let conversation_id = command
                    .payload
                    .conversation_id
                    .as_ref()
                    .map(|id| id.as_str().to_owned())
                    .unwrap_or_else(|| command.runtime.conversation_id.as_str().to_owned());
                let change = CwdChange {
                    agent_id,
                    conversation_id,
                    cwd: cwd.to_owned(),
                };
                // Scrubbed rejection: the broadcast still echoes the request,
                // and the cwd map simply keeps its previous entry.
                let _ = self.settings.apply_cwd_change(0, &change);
            }
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

#[derive(Clone)]
pub(crate) struct ProductionProviderPort {
    connections: Arc<ConnectionManager>,
    native: NativeAdapterRegistry,
    host: Arc<dyn ConnectionAdapterFactory>,
    pinned_connection: Option<String>,
    #[cfg(test)]
    test_port: Option<Arc<dyn ProviderPort>>,
}

impl ProductionProviderPort {
    fn new(
        connections: ConnectionManager,
        native: NativeAdapterRegistry,
        host: Arc<dyn ConnectionAdapterFactory>,
    ) -> Self {
        Self {
            connections: Arc::new(connections),
            native,
            host,
            pinned_connection: None,
            #[cfg(test)]
            test_port: None,
        }
    }

    #[cfg(test)]
    fn with_test_port(mut self, port: Arc<dyn ProviderPort>) -> Self {
        self.test_port = Some(port);
        self.native = NativeAdapterRegistry::default();
        self
    }

    pub(crate) fn connections(&self) -> Vec<lotta_providers::connections::ConnectionSnapshot> {
        self.connections.snapshots()
    }

    /// Pins one request-local adapter to the selected connection while the request model changes.
    pub(crate) fn for_route(&self, connection_id: &str) -> Result<Self, SetupError> {
        self.connections
            .snapshot(connection_id)
            .ok_or_else(|| SetupError::Adapter("provider route unavailable".into()))?;
        let mut route = self.clone();
        route.pinned_connection = Some(connection_id.to_owned());
        Ok(route)
    }

    fn route_for(&self, connection_id: &str) -> &'static str {
        self.connections
            .snapshot(connection_id)
            .filter(|snapshot| self.native.contains(&snapshot.provider_type))
            .map_or("host", |_| "native")
    }
}

impl ProviderPort for ProductionProviderPort {
    fn route_hint(&self, provider_id: &str) -> Option<&'static str> {
        Some(self.route_for(provider_id))
    }

    fn stream(&self, request: ProviderRequest, events: ProviderEventSink) -> PortFuture<'_, ()> {
        #[cfg(test)]
        if let Some(port) = self.test_port.as_ref() {
            return port.stream(request, events);
        }
        Box::pin(async move {
            let connection_id = self
                .pinned_connection
                .clone()
                .unwrap_or_else(|| request.model.provider_id.as_str().to_owned());
            let adapter = if self.route_for(&connection_id) == "native" {
                let native = self.native.clone();
                self.connections
                    .build_with(&connection_id, move |provider_type, auth, base_url| {
                        Box::pin(async move { native.build(provider_type, auth, base_url).await })
                    })
                    .await
            } else {
                let host = Arc::clone(&self.host);
                self.connections
                    .build_with(&connection_id, move |provider_type, auth, base_url| {
                        let auth = host_auth(auth);
                        let options = lotta_providers::host::protocol::HostOptions {
                            base_url: base_url.map(str::to_owned),
                            ..Default::default()
                        };
                        Box::pin(async move { host.build(provider_type, auth?, options).await })
                    })
                    .await
            };
            match adapter {
                Ok(adapter) => adapter.stream(request, events).await,
                Err(error) => send_connection_error(&events, error).await,
            }
        })
    }
}

fn host_auth(
    auth: &lotta_providers::connections::ProviderAuth,
) -> Result<lotta_providers::host::protocol::HostAuth, ConnectionError> {
    use lotta_providers::connections::ProviderAuth;
    use lotta_providers::host::protocol::HostAuth;
    match auth {
        ProviderAuth::Api { key, .. } => Ok(HostAuth::ApiKey {
            value: key.expose().to_owned(),
        }),
        ProviderAuth::OAuth { access, .. } => Ok(HostAuth::OAuthAccess {
            value: access.expose().to_owned(),
        }),
        ProviderAuth::BedrockProfile { .. } => Err(ConnectionError::Unsupported),
    }
}

async fn send_connection_error(
    events: &ProviderEventSink,
    error: ConnectionError,
) -> Result<(), lotta_runtime::RuntimeError> {
    let kind = match error {
        ConnectionError::NotFound => "provider_connection_missing",
        ConnectionError::InvalidInput(_) | ConnectionError::Unsupported => {
            "provider_connection_invalid"
        }
        _ => "provider_connection_unavailable",
    };
    let context = ProviderErrorContext::new(
        ProviderName::new(kind.into())?,
        ProviderEventText::new("configured provider is unavailable".into())?,
    );
    events
        .send(ProviderEvent::Error {
            error: ProviderError::Unavailable(context),
        })
        .await
}

pub(crate) struct ProductionToolPort {
    hook_runtime: Arc<dyn lotta_runtime::hooks::HookRuntime>,
    overflow: Arc<lotta_tools::clamp::FileOverflowWriter>,
    shell: Arc<ShellToolBundle>,
}

impl ProductionToolPort {
    fn new(
        _registry: Arc<lotta_tools::ToolRegistry>,
        hook_runtime: Arc<dyn lotta_runtime::hooks::HookRuntime>,
        overflow_root: std::path::PathBuf,
        shell: Arc<ShellToolBundle>,
    ) -> Result<Self, SetupError> {
        Ok(Self {
            hook_runtime,
            overflow: Arc::new(
                lotta_tools::clamp::FileOverflowWriter::new(overflow_root)
                    .map_err(|error| SetupError::Adapter(format!("{error:?}")))?,
            ),
            shell,
        })
    }
}

struct NoSecrets;
impl lotta_tools::SecretResolver for NoSecrets {
    fn resolve(&self, _: &str) -> Result<Option<String>, lotta_tools::PipelineError> {
        Ok(None)
    }
}
struct NoTrace;
impl lotta_tools::TraceSink for NoTrace {
    fn record(&self, _: lotta_tools::TraceEvent) {}
}
struct NoOutcome;
impl lotta_tools::OutcomeSink for NoOutcome {
    fn record(&self, _: &str, _: &ToolOutcome) -> Result<(), lotta_tools::PipelineError> {
        Ok(())
    }
}

pub(crate) struct ScopedProductionToolPort {
    snapshot: Arc<lotta_tools::RegistrySnapshot>,
    hook_runtime: Arc<dyn lotta_runtime::hooks::HookRuntime>,
    overflow: Arc<lotta_tools::clamp::FileOverflowWriter>,
    permissions: Arc<lotta_tools::PermissionPolicy>,
    workspace: Arc<lotta_tools::WorkspacePolicy>,
    secrets: Option<Arc<dyn lotta_tools::SecretResolver>>,
    cwd: PathBuf,
}

impl ProductionToolPort {
    pub(crate) fn child_owner(
        &self,
        scope: &RuntimeScope,
        lease_generation: u64,
    ) -> Result<lotta_tools::builtin::shell::ShellCancellationOwner, lotta_runtime::RuntimeError>
    {
        self.shell
            .cancellation_owner(scope.clone(), lease_generation)
            .map_err(|_| lotta_runtime::RuntimeError::AdapterFailure {
                code: "shell_child_owner",
                context: "production shell process manager".into(),
            })
    }

    pub(crate) fn scoped(
        &self,
        snapshot: Arc<lotta_tools::RegistrySnapshot>,
        permissions: Arc<lotta_tools::PermissionPolicy>,
        workspace: Arc<lotta_tools::WorkspacePolicy>,
        secrets: Option<Arc<dyn lotta_tools::SecretResolver>>,
        cwd: PathBuf,
    ) -> ScopedProductionToolPort {
        ScopedProductionToolPort {
            snapshot,
            hook_runtime: Arc::clone(&self.hook_runtime),
            overflow: Arc::clone(&self.overflow),
            permissions,
            workspace,
            secrets,
            cwd,
        }
    }
}

impl ToolPort for ScopedProductionToolPort {
    fn execute(&self, request: ToolExecutionRequest) -> PortFuture<'_, ToolOutcome> {
        Box::pin(async move {
            let model_name = request.definition.model_name.as_str().to_owned();
            let input =
                BoundedJsonValue::new(request.input.as_value().clone()).map_err(tool_adapter)?;
            let permissions = lotta_tools::PolicyGate::new(self.permissions.as_ref(), &[], &[]);
            let sandbox =
                lotta_tools::WorkspaceSandboxGate::new(self.workspace.as_ref(), &self.cwd);
            // Applied agent secrets resolve here: the WS-applied record feeds
            // tool pipelines running for that exact agent. No registered
            // resolver (standalone surfaces) keeps the inert default.
            let fallback;
            let secrets: &dyn lotta_tools::SecretResolver = match self.secrets.as_deref() {
                Some(resolver) => resolver,
                None => {
                    fallback = NoSecrets;
                    &fallback
                }
            };
            lotta_tools::execute(lotta_tools::PipelineRequest {
                tool_call_id: request.tool_call_id,
                approval_grant: request.approval_grant,
                registry: Arc::clone(&self.snapshot),
                model_name: &model_name,
                input,
                cancellation: request.cancellation,
                hook_runtime: self.hook_runtime.as_ref(),
                permissions: &permissions,
                sandbox: &sandbox,
                secrets,
                trace: &NoTrace,
                overflow: self.overflow.as_ref(),
                persistence: &NoOutcome,
                emit: &NoOutcome,
            })
            .await
            .map_err(tool_adapter)
        })
    }
}

pub(crate) struct ProductionEffects {
    actor: ProductionEffectActor,
    scope: RuntimeScope,
    sink: Arc<dyn RuntimeEventSink>,
    #[cfg_attr(not(test), expect(dead_code, reason = "test diagnostic observer"))]
    cancellation_observer: Option<CancellationOperationObserver>,
    pub(crate) turn_id: NonEmptyString,
    pub(crate) run_id: lotta_domain::RunId,
    pub(crate) input_id: NonEmptyString,
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
            cancellation_observer: None,
            turn_id,
            run_id,
            input_id,
            sequence: std::sync::atomic::AtomicU64::new(1),
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_cancellation(
        mut self,
        observer: Arc<ProductionCancellationObserver>,
    ) -> Self {
        self.cancellation_observer = Some(observer);
        self
    }

    #[cfg(test)]
    fn record_cancellation(&self, step: &'static str) {
        if let Some(observer) = &self.cancellation_observer {
            observer.record(step);
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

    fn persist_stop_reason(&self, record: TurnStopRecord) -> EffectResult {
        self.append_message(
            lotta_domain::LocalMessageRole::Assistant,
            serde_json::json!({"type": "turn_stop", "record": record}),
            format!("{}-stop", self.turn_id.as_str()),
        )?;
        #[cfg(test)]
        self.record_cancellation("persist");
        Ok(())
    }

    fn emit(&self, event: TurnEvent) -> EffectResult {
        #[cfg(test)]
        let cancelled = matches!(&event, TurnEvent::Cancelled);
        let wire = match event {
            TurnEvent::ControlRequest(request) => {
                lotta_app_server::ws::RuntimeEvent::ControlRequest {
                    request_id: request.request_id,
                    request: BoundedJsonValue::new(serde_json::json!({
                        "subtype": "can_use_tool",
                        "tool_call_id": request.call_id.as_str(),
                        "tool_name": request.tool_name.as_str(),
                        "input": request.input.as_value(),
                        "permission_suggestions": [],
                        "blocked_path": null,
                    }))
                    .map_err(effect_error)?,
                    agent_id: NonEmptyString::new(self.scope.agent_id.as_str().to_owned()).ok(),
                    conversation_id: NonEmptyString::new(
                        self.scope.conversation_id.as_str().to_owned(),
                    )
                    .ok(),
                }
            }
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
            TurnEvent::Cancelled => lotta_app_server::ws::RuntimeEvent::TurnFinished {
                turn_id: self.turn_id.clone(),
                run_id: Some(self.run_id.clone()),
                stop_reason: NonEmptyString::new("cancelled").map_err(effect_error)?,
                error: None,
            },
            TurnEvent::Failed { reason } => lotta_app_server::ws::RuntimeEvent::TurnFinished {
                turn_id: self.turn_id.clone(),
                run_id: Some(self.run_id.clone()),
                stop_reason: NonEmptyString::new(reason.wire_value()).map_err(effect_error)?,
                error: None,
            },
            TurnEvent::Retry(_retry) => loop_event("RETRYING_API_REQUEST", Vec::new())
                .map_err(|_| effect_error("retry loop snapshot"))?,
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
            .map_err(|_| effect_error("runtime event sink"))?;
        #[cfg(test)]
        if cancelled {
            self.record_cancellation("cancelled");
        }
        Ok(())
    }

    fn persist_compaction_request(
        &self,
        request: &lotta_runtime::turn::CompactionRequest,
    ) -> EffectResult {
        let sequence = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.append_message(
            lotta_domain::LocalMessageRole::Assistant,
            serde_json::json!({
                "type": "compaction_request",
                "scope": {
                    "agent_id": request.scope.agent_id.as_str(),
                    "conversation_id": request.scope.conversation_id.as_str(),
                },
                "lease_generation": request.lease_generation,
                "reason": request.reason.as_str(),
                "tokens_before": request.tokens_before,
                "messages_before": request.messages_before,
            }),
            format!("{}-compaction-{sequence}", self.turn_id.as_str()),
        )
    }

    fn persist_controller_request(&self, request: ControllerToolRequestRecord) -> EffectResult {
        let sequence = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.append_message(
            lotta_domain::LocalMessageRole::Assistant,
            serde_json::json!({
                "type": "controller_tool_request",
                "scope": {
                    "agent_id": request.scope.agent_id.as_str(),
                    "conversation_id": request.scope.conversation_id.as_str(),
                },
                "run_id": request.run_id.as_str(),
                "lease_generation": request.lease_generation,
                "call_id": request.call_id.as_str(),
                "tool_name": request.tool_name.as_str(),
                "input": request.input.as_value(),
            }),
            format!("{}-controller-{sequence}", self.turn_id.as_str()),
        )
    }

    fn persist_control_request(&self, request: &ControlRequest) -> EffectResult {
        let sequence = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.append_message(
            lotta_domain::LocalMessageRole::Assistant,
            serde_json::json!({
                "type": "control_request",
                "request_id": request.request_id.as_str(),
                "call_id": request.call_id.as_str(),
                "lease_generation": request.lease_generation,
                "tool_name": request.tool_name.as_str(),
                "input": request.input.as_value(),
                "schema": request.schema.as_value()
            }),
            format!("{}-control-{sequence}", self.turn_id.as_str()),
        )
    }

    fn append_tool_result(&self, result: ToolResultRecord) -> EffectResult {
        let sequence = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.append_message(
            lotta_domain::LocalMessageRole::ToolResult,
            serde_json::json!({
                "call_id": result.call_id.as_str(),
                "outcome": result.outcome
            }),
            format!("{}-tool-{sequence}", self.turn_id.as_str()),
        )
    }
}

fn tool_adapter(error: impl std::fmt::Debug) -> lotta_runtime::RuntimeError {
    lotta_runtime::RuntimeError::AdapterFailure {
        code: "tool_pipeline",
        context: format!("{error:?}"),
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
        lotta_runtime::RuntimeError::InvalidData { .. }
        | lotta_runtime::RuntimeError::Conflict { .. } => AppServerError::Malformed,
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
    use lotta_app_server::ws::RuntimeEvent;
    use lotta_app_server::ws::device::{
        BackgroundProcessSource, DeviceBridge, DeviceForwarder, DeviceMessage,
    };
    use lotta_app_server::ws::service::{RuntimeCommandService, RuntimeEventSink, TurnController};
    use lotta_domain::{ConversationId, DomainError, RunId, Timestamp, TurnStateKind};

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
    struct Sink(Mutex<Vec<(RuntimeScope, lotta_app_server::ws::RuntimeEvent)>>);
    impl RuntimeEventSink for Sink {
        fn emit(
            &self,
            scope: &RuntimeScope,
            event: lotta_app_server::ws::RuntimeEvent,
        ) -> Result<(), AppServerError> {
            self.0.lock().unwrap().push((scope.clone(), event));
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
            payload: BoundedJsonValue::new(serde_json::json!({
                "messages": [{
                    "client_message_id": id,
                    "content": [{"type": "text", "text": "hello"}]
                }]
            }))
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
                        == Some("WAITING_ON_INPUT")
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
        for directory in ["artifacts", ".letta/skills", "bundled-skills"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let model = production_model("openai", "gpt-5.4", 128_000, true).unwrap();
        let policy = workspace_policy(&root, &workspace).unwrap();
        let shell = ShellToolBundle::new(
            &workspace,
            production_shell_scope().unwrap(),
            Arc::new(OsSandbox::detect(policy)),
        )
        .unwrap();
        let config = setup_config(
            &root,
            &workspace,
            vec![model],
            ModelHandle::from_str("openai/gpt-5.4").unwrap(),
            vec![lotta_providers::connections::ConnectionSnapshot {
                id: "openai".into(),
                provider_name: "openai".into(),
                provider_type: "openai".into(),
                auth_type: lotta_providers::connections::AuthMethod::Api,
                base_url: None,
                timeout: None,
                region: None,
                access_key: None,
                is_connected: true,
                revision: 1,
            }],
            &shell,
        )
        .expect("exact production builder");
        let expected: std::collections::BTreeSet<_> = production_builtins(
            &root,
            &workspace,
            &config.workspace_policy,
            &config.skill_roots,
            &shell,
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

    struct CountingExecutor(Arc<std::sync::atomic::AtomicU64>);

    impl lotta_tools::pipeline::ToolExecutor for CountingExecutor {
        fn execute(
            &self,
            _: lotta_tools::pipeline::RawToolExecutionRequest,
        ) -> lotta_tools::pipeline::ExecutorFuture<'_> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Ok(lotta_tools::pipeline::RawToolOutcome::Success(
                    "approved execution".to_owned(),
                ))
            })
        }
    }

    fn registration_for_test(
        name: &str,
        approval: lotta_runtime::ports::ToolApprovalPolicy,
    ) -> lotta_runtime::ports::ToolDefinition {
        use lotta_domain::BoundedVec;
        use lotta_runtime::ports::*;
        ToolDefinition::new(
            InternalToolName::new(name.to_owned()).unwrap(),
            ModelFacingToolName::new(name.to_owned()).unwrap(),
            ToolInputSchema::new(
                BoundedJsonValue::new(serde_json::json!({
                    "type": "object"
                }))
                .unwrap(),
            )
            .unwrap(),
            ToolDescriptionAsset::new("approval test".to_owned()).unwrap(),
            ToolExecutionOwner::Rust,
            approval,
            PermissionAction::new("read".to_owned()).unwrap(),
            ToolTimeout::new(std::time::Duration::from_secs(1)).unwrap(),
            ToolOutputLimit::new(1024, 1024).unwrap(),
            SecretRedactionSpec::new(
                BoundedVec::new(Vec::new()).unwrap(),
                SecretRedactionPolicy::Redact,
            )
            .unwrap(),
        )
    }

    struct ApprovedExecutionFixture {
        root: std::path::PathBuf,
        port: ScopedProductionToolPort,
        definition: lotta_runtime::ports::ToolDefinition,
        counter: Arc<std::sync::atomic::AtomicU64>,
        snapshot: Arc<lotta_tools::RegistrySnapshot>,
    }

    fn approved_execution_fixture() -> ApprovedExecutionFixture {
        use lotta_tools::permissions::scopes::{PermissionSourcePaths, load_permissions};

        let root = std::env::temp_dir().join(format!("lotta-approval-{}", unique_test_id()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let definition =
            registration_for_test("Read", lotta_runtime::ports::ToolApprovalPolicy::Always);
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let registry = lotta_tools::ToolRegistry::new(vec![ToolRegistration {
            definition: Arc::new(definition.clone()),
            executor: Arc::new(CountingExecutor(Arc::clone(&counter))),
        }])
        .unwrap();
        let snapshot = registry
            .update(ToolsetId::Default, &[], Some(&["Read"]))
            .unwrap();
        let permissions = load_permissions(
            &PermissionSourcePaths::new(&root, &root, &workspace),
            &workspace,
        )
        .unwrap();
        let policy = Arc::new(
            lotta_tools::PermissionPolicy::new(
                &workspace,
                scope("approved-tool"),
                Some(lotta_domain::PermissionMode::Unrestricted),
                permissions,
            )
            .unwrap(),
        );
        let sandbox = WorkspaceSandbox::new(workspace.clone(), root.clone());
        let workspace_policy = Arc::new(WorkspacePolicy::new(&sandbox).unwrap());
        std::fs::create_dir_all(root.join("artifacts")).unwrap();
        let artifacts = root.join("artifacts").canonicalize().unwrap();
        let shell_sandbox: Arc<dyn lotta_tools::builtin::shell::ShellSandbox> =
            Arc::new(OsSandbox::detect((*workspace_policy).clone()));
        let shell = Arc::new(
            ShellToolBundle::new(
                &workspace.canonicalize().unwrap(),
                scope("shell"),
                shell_sandbox,
            )
            .unwrap(),
        );
        let port = ProductionToolPort::new(
            Arc::new(registry),
            Arc::new(lotta_runtime::hooks::NoopHookRuntime),
            artifacts,
            shell,
        )
        .unwrap()
        .scoped(
            Arc::clone(&snapshot),
            policy,
            workspace_policy,
            None,
            workspace,
        );
        ApprovedExecutionFixture {
            root,
            port,
            definition,
            counter,
            snapshot,
        }
    }

    struct AcceptConnection;

    impl lotta_providers::connections::ConnectionAdapter for AcceptConnection {
        fn validate<'a>(
            &'a self,
            _: &'a str,
            _: &'a lotta_providers::connections::ProviderAuth,
            _: &'a std::collections::BTreeMap<String, String>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + Send + 'a>,
        > {
            Box::pin(async { Ok(()) })
        }
    }

    struct UnusedConnectionFactory;

    impl ConnectionAdapterFactory for UnusedConnectionFactory {
        fn build<'a>(
            &'a self,
            _: &'a str,
            _: lotta_providers::host::protocol::HostAuth,
            _: lotta_providers::host::protocol::HostOptions,
        ) -> lotta_providers::connections::ConnectionAdapterFuture<'a> {
            Box::pin(async { Err(ConnectionError::Unsupported) })
        }
    }

    struct HeldProvider {
        waiting: tokio::sync::Semaphore,
        release: tokio::sync::Notify,
        calls: std::sync::atomic::AtomicU64,
    }

    impl Default for HeldProvider {
        fn default() -> Self {
            Self {
                waiting: tokio::sync::Semaphore::new(0),
                release: tokio::sync::Notify::new(),
                calls: std::sync::atomic::AtomicU64::new(0),
            }
        }
    }

    impl HeldProvider {
        fn release(&self) {
            self.release.notify_waiters();
        }
    }

    impl ProviderPort for HeldProvider {
        fn stream(
            &self,
            request: ProviderRequest,
            events: ProviderEventSink,
        ) -> PortFuture<'_, ()> {
            Box::pin(async move {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if request.tools.is_empty() {
                    events
                        .send(ProviderEvent::TextDelta {
                            text: lotta_runtime::boundary::ProviderEventText::new(
                                "held provider summary".into(),
                            )?,
                        })
                        .await?;
                    return events
                        .send(ProviderEvent::Stop {
                            reason: lotta_runtime::ports::StopReason::EndTurn,
                        })
                        .await;
                }
                self.waiting.add_permits(1);
                tokio::select! {
                    () = request.cancellation.cancelled() => {
                        return Err(lotta_runtime::RuntimeError::Cancelled {
                            context: "production held provider".into(),
                        });
                    },
                    () = self.release.notified() => {},
                }
                events
                    .send(ProviderEvent::Stop {
                        reason: lotta_runtime::ports::StopReason::EndTurn,
                    })
                    .await
            })
        }
    }

    /// Provider variant that completes every turn immediately while recording
    /// the compiled system prompt and signalling a permit per attempt.
    struct CompletingProvider {
        waiting: tokio::sync::Semaphore,
        prompts: Mutex<Vec<String>>,
        calls: std::sync::atomic::AtomicU64,
    }

    impl Default for CompletingProvider {
        fn default() -> Self {
            Self {
                waiting: tokio::sync::Semaphore::new(0),
                prompts: Mutex::new(Vec::new()),
                calls: std::sync::atomic::AtomicU64::new(0),
            }
        }
    }

    impl CompletingProvider {
        fn last_prompt(&self) -> String {
            self.prompts
                .lock()
                .expect("prompt lock")
                .last()
                .cloned()
                .unwrap_or_default()
        }
    }

    impl ProviderPort for CompletingProvider {
        fn stream(
            &self,
            request: ProviderRequest,
            events: ProviderEventSink,
        ) -> PortFuture<'_, ()> {
            Box::pin(async move {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if let Ok(mut prompts) = self.prompts.lock()
                    && let Some(prompt) = request.system_prompt.as_ref()
                {
                    prompts.push(prompt.as_str().to_owned());
                }
                if !request.tools.is_empty() {
                    // Persistable assistant text so later turn setups can
                    // re-map the transcript entry written for this turn.
                    events
                        .send(ProviderEvent::TextDelta {
                            text: lotta_runtime::boundary::ProviderEventText::new(
                                "completing provider summary".into(),
                            )?,
                        })
                        .await?;
                }
                self.waiting.add_permits(1);
                events
                    .send(ProviderEvent::Stop {
                        reason: lotta_runtime::ports::StopReason::EndTurn,
                    })
                    .await
            })
        }
    }

    fn service() -> ProductionRuntimeService {
        let root = std::env::temp_dir().join(format!("lotta-task54-{}", unique_test_id()));
        std::fs::create_dir_all(&root).unwrap();
        let paths = StorePaths::new(root).unwrap();
        let approvals = Arc::new(lotta_runtime::ApprovalManager::new(
            LocalStore::new(paths.clone()).approval_journal(),
            Arc::new(crate::production_setup::ProductionEditedInputValidator),
        ));
        let settings = test_settings_bridge(paths.root(), paths.root());
        let service = ProductionRuntimeService::new(
            paths,
            Arc::new(TestClock),
            Arc::new(lotta_runtime::hooks::NoopHookRuntime),
            Arc::new(ProductionRuntimeState::new(Arc::new(
                lotta_runtime::observe::RuntimeObserver::default(),
            ))),
            approvals,
            Arc::new(ProductionTurnBrokers::new()),
            settings,
        );
        service.register_device_snapshot_source(Arc::new(|_| {
            BoundedJsonValue::new(serde_json::json!({"is_online": true}))
                .map_err(|_| AppServerError::Internal)
        }));
        service
    }

    /// Builds a settings bridge over inert forwarding for fixture composition.
    fn test_settings_bridge(storage: &Path, workspace: &Path) -> Arc<SettingsBridge> {
        Arc::new(
            SettingsBridge::new(Arc::new(|_, _| Ok(())), storage, workspace)
                .expect("test settings bridge"),
        )
    }

    async fn production_controller_fixture(
        label: &str,
        provider_port: Arc<dyn ProviderPort>,
    ) -> (
        std::path::PathBuf,
        Arc<ProductionRuntimeService>,
        Arc<ProductionTurnController>,
        Arc<SkillsBridge>,
        Arc<SettingsBridge>,
    ) {
        use lotta_domain::{Agent, Conversation};
        use lotta_providers::connections::{
            AuthMethod, ConnectProviderInput, ProviderAuth, ProviderSecret,
        };

        let root = std::env::temp_dir().join(format!("lotta-cancel-{label}-{}", unique_test_id()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        for directory in ["artifacts", ".letta/skills", "bundled-skills"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let paths = StorePaths::new(root.clone()).unwrap();
        let scope = scope(label);
        let agent: Agent = serde_json::from_value(serde_json::json!({
            "id": scope.agent_id.as_str(), "name": scope.agent_id.as_str(),
            "description": null, "system": "test", "tags": [],
            "model": "openai/gpt-5.4", "model_settings": {}, "hidden": false,
            "compaction_settings": null
        }))
        .unwrap();
        let conversation: Conversation = serde_json::from_value(serde_json::json!({
            "id": scope.conversation_id.as_str(), "agent_id": scope.agent_id.as_str(),
            "archived": false, "created_at": "2026-08-18T00:00:00Z",
            "updated_at": "2026-08-18T00:00:00Z", "last_message_at": null,
            "summary": null, "in_context_message_ids": [], "model": null,
            "model_settings": null, "context_window_limit": null, "hidden": false,
            "tags": []
        }))
        .unwrap();
        let store = LocalStore::new(paths.clone());
        AgentStore::save(&store, &agent).await.unwrap();
        ConversationStore::save(&store, &conversation)
            .await
            .unwrap();

        let mut manager = ConnectionManager::load(ProviderAuthStore::new(paths.clone())).unwrap();
        manager
            .connect(
                ConnectProviderInput {
                    provider_id: "openai".into(),
                    provider_name: "openai".into(),
                    provider_type: "openai".into(),
                    auth_method: AuthMethod::Api,
                    auth: ProviderAuth::Api {
                        key: ProviderSecret::new("test-key".into()).unwrap(),
                        extras: Default::default(),
                    },
                    fields: Default::default(),
                    expected_revision: 0,
                    now: "2026-08-18T00:00:00Z".into(),
                },
                &AcceptConnection,
            )
            .await
            .unwrap();
        let policy = workspace_policy(&root, &workspace).unwrap();
        let shell = Arc::new(
            ShellToolBundle::new(
                &workspace.canonicalize().unwrap(),
                production_shell_scope().unwrap(),
                Arc::new(OsSandbox::detect(policy.clone())),
            )
            .unwrap(),
        );
        let setup = Arc::new(
            ProductionSetupPorts::new(
                setup_config(
                    &root,
                    &workspace,
                    vec![production_model("openai", "gpt-5.4", 128_000, true).unwrap()],
                    ModelHandle::from_str("openai/gpt-5.4").unwrap(),
                    manager.snapshots(),
                    shell.as_ref(),
                )
                .unwrap(),
            )
            .unwrap(),
        );
        use lotta_runtime::ports::MemFsPort as _;
        GitMemFs::new(root.clone())
            .unwrap()
            .initialize(
                &scope.agent_id,
                &lotta_runtime::boundary::InitialMemoryBlocks::new(Vec::new()).unwrap(),
            )
            .await
            .unwrap();
        let provider = Arc::new(
            ProductionProviderPort::new(
                manager,
                NativeAdapterRegistry::default(),
                Arc::new(UnusedConnectionFactory),
            )
            .with_test_port(provider_port),
        );
        let tools = Arc::new(
            ProductionToolPort::new(
                setup.registry(),
                setup.hook_runtime(),
                root.join("artifacts"),
                shell,
            )
            .unwrap(),
        );
        let approvals = Arc::new(lotta_runtime::ApprovalManager::new(
            store.approval_journal(),
            Arc::new(crate::production_setup::ProductionEditedInputValidator),
        ));
        let state = Arc::new(ProductionRuntimeState::new(Arc::new(
            lotta_runtime::observe::RuntimeObserver::default(),
        )));
        let brokers = Arc::new(ProductionTurnBrokers::new());
        let compaction = Arc::new(crate::production_setup::RegisteredProductionCompaction {
            service: lotta_runtime::CompactionService::new(
                crate::production_setup::ProductionProviderSummarizer {
                    provider: Arc::clone(&provider),
                },
                crate::production_setup::ProductionCompactionEffects {
                    setup: Arc::clone(&setup),
                    runtime_state: Arc::clone(&state),
                },
            ),
        });
        brokers.register_compaction_service(Some(compaction));
        let skills = Arc::new(SkillsBridge::new(
            Arc::new(|_, _| Ok(())),
            &root,
            Arc::new(TestClock),
        ));
        let settings = test_settings_bridge(&root, &workspace);
        let service = Arc::new(ProductionRuntimeService::new(
            paths,
            Arc::new(TestClock),
            setup.hook_runtime(),
            Arc::clone(&state),
            Arc::clone(&approvals),
            Arc::clone(&brokers),
            Arc::clone(&settings),
        ));
        let controller = Arc::new(ProductionTurnController::new(
            setup,
            provider,
            tools,
            store,
            Arc::new(TestClock),
            workspace,
            state,
            brokers,
            approvals,
            Arc::new(crate::production_setup::UnavailableReflection),
            Arc::new(crate::production_setup::UnavailableMemoryPush),
            Arc::clone(&skills),
            Arc::clone(&settings),
        ));
        (root, service, controller, skills, settings)
    }

    mod production_compaction {
        use super::*;

        #[tokio::test]
        async fn manual() {
            let label = "compaction-manual";
            let held = Arc::new(HeldProvider::default());
            let (root, service, controller, ..) =
                production_controller_fixture(label, held.clone() as Arc<dyn ProviderPort>).await;
            let scope = scope(label);
            let input = command(scope.clone(), "active-compaction");
            let admitted = service
                .admit_input(input.clone())
                .await
                .expect("admit active turn");
            let deferred = lotta_app_server::ws::DeferredInput {
                scope: scope.clone(),
                disposition: admitted.disposition,
                continuation: admitted.continuation,
            };
            let cancellation = CancellationToken::new();
            let turn = {
                let controller = Arc::clone(&controller);
                let cancellation = cancellation.clone();
                tokio::spawn(async move {
                    controller
                        .submit_turn(input, deferred, cancellation, Arc::new(Sink::default()))
                        .await
                })
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), held.waiting.acquire())
                .await
                .expect("held turn timeout")
                .expect("held turn")
                .forget();
            let store = LocalStore::new(StorePaths::new(root.clone()).unwrap());
            let now = Timestamp::parse_persisted_rfc3339("2026-08-18T00:00:00Z").unwrap();
            let manifest = lotta_domain::TranscriptManifest {
                schema_version: 2,
                message_format: lotta_domain::TranscriptMessageFormat::PiSessionEntryJsonl,
                provider_stack: lotta_domain::ProviderStack::PiAi,
                created_at: now,
                migrated_from: None,
                migrated_at: None,
                backup_path: None,
            };
            let session = lotta_domain::TranscriptEntry::Session(lotta_domain::SessionEntry {
                entry_type: lotta_domain::SessionEntryType::Session,
                id: NonEmptyString::new("session-compaction-58").unwrap(),
                version: 3,
                timestamp: now,
                cwd: "/".into(),
            });
            let directory = store
                .paths()
                .conversation_dir(&scope.agent_id, &scope.conversation_id)
                .unwrap();
            if !directory.join("messages.jsonl").exists() {
                std::fs::create_dir_all(&directory).unwrap();
                store
                    .initialize_transcript(
                        &scope.agent_id,
                        &scope.conversation_id,
                        &manifest,
                        &session,
                    )
                    .await
                    .unwrap();
            }
            let request = lotta_runtime::ports::ProviderRequest {
                model: lotta_domain::ModelDescriptor {
                    handle: NonEmptyString::new("openai/gpt-5.4").unwrap(),
                    provider_id: NonEmptyString::new("openai").unwrap(),
                    available: true,
                    context_window: Some(128_000),
                    model_settings: None,
                },
                system_prompt: Some(
                    lotta_runtime::boundary::ProviderText::new(
                        "production compaction prompt".into(),
                    )
                    .unwrap(),
                ),
                messages: lotta_domain::BoundedVec::new(vec![
                    lotta_runtime::ports::ProviderMessage {
                        role: lotta_runtime::ports::ProviderMessageRole::User,
                        content: lotta_domain::BoundedVec::new(vec![
                            lotta_runtime::ports::ProviderContentPart::Text(
                                lotta_runtime::boundary::ProviderText::new("old message".into())
                                    .unwrap(),
                            ),
                        ])
                        .unwrap(),
                        tool_call_id: None,
                    },
                    lotta_runtime::ports::ProviderMessage {
                        role: lotta_runtime::ports::ProviderMessageRole::Assistant,
                        content: lotta_domain::BoundedVec::new(vec![
                            lotta_runtime::ports::ProviderContentPart::Text(
                                lotta_runtime::boundary::ProviderText::new("recent message".into())
                                    .unwrap(),
                            ),
                        ])
                        .unwrap(),
                        tool_call_id: None,
                    },
                ])
                .unwrap(),
                tools: lotta_domain::BoundedVec::new(Vec::new()).unwrap(),
                tool_choice: lotta_runtime::ports::ProviderToolChoice::Auto,
                image_policy: lotta_runtime::ports::ImagePolicy::Strict,
                context_tokens_max: lotta_runtime::ports::TokenLimit::new(128_000).unwrap(),
                output_tokens_max: lotta_runtime::ports::TokenLimit::new(64).unwrap(),
                reasoning: lotta_runtime::ports::ReasoningControls {
                    enabled: false,
                    effort: None,
                    tier: None,
                },
                cancellation: CancellationToken::new(),
                context: None,
                deadline: lotta_runtime::ports::ProviderDeadline::default(),
            };
            let progress = ProductionRuntimeService::compact(
                service.as_ref(),
                scope.clone(),
                lotta_runtime::CompactionMode::All,
                NonEmptyString::new("manual-compaction-58").unwrap(),
                request,
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "manual compaction: {error:?}; journal={:?}; transcript={:?}",
                    std::fs::read_to_string(root.join("runtime/compactions.json")),
                    std::fs::read_to_string(directory.join("messages.jsonl"))
                )
            });
            assert_eq!(progress.messages_before, 2);
            let transaction = LocalStore::new(StorePaths::new(root.clone()).unwrap())
                .compaction_transaction(
                    &scope,
                    &NonEmptyString::new("manual-compaction-58").unwrap(),
                )
                .unwrap()
                .expect("durable transaction");
            assert!(matches!(
                transaction.state,
                lotta_store::CompactionTransactionState::Published
            ));
            let transcript_path = LocalStore::new(StorePaths::new(root).unwrap())
                .paths()
                .conversation_dir(&scope.agent_id, &scope.conversation_id)
                .unwrap()
                .join("messages.jsonl");
            let transcript = std::fs::read_to_string(transcript_path).unwrap();
            assert!(
                transcript
                    .lines()
                    .any(|line| line.contains("\"id\":\"manual-compaction-58\"")
                        && line.contains("\"type\":\"compaction\""))
            );
            cancellation.cancel();
            let _ = turn.await.expect("turn task");
        }

        fn provider_message(
            role: lotta_runtime::ports::ProviderMessageRole,
            text: &str,
        ) -> lotta_runtime::ports::ProviderMessage {
            lotta_runtime::ports::ProviderMessage {
                role,
                content: lotta_domain::BoundedVec::new(vec![
                    lotta_runtime::ports::ProviderContentPart::Text(
                        lotta_runtime::boundary::ProviderText::new(text.into()).unwrap(),
                    ),
                ])
                .unwrap(),
                tool_call_id: None,
            }
        }

        fn compaction_request() -> lotta_runtime::ports::ProviderRequest {
            use lotta_runtime::ports::{
                ImagePolicy, ProviderToolChoice, ReasoningControls, TokenLimit,
            };
            let model = |handle: &'static str| NonEmptyString::new(handle).unwrap();
            lotta_runtime::ports::ProviderRequest {
                model: lotta_domain::ModelDescriptor {
                    handle: model("openai/gpt-5.4"),
                    provider_id: model("openai"),
                    available: true,
                    context_window: Some(128_000),
                    model_settings: None,
                },
                system_prompt: Some(
                    lotta_runtime::boundary::ProviderText::new("contention prompt".into()).unwrap(),
                ),
                messages: lotta_domain::BoundedVec::new(vec![
                    provider_message(lotta_runtime::ports::ProviderMessageRole::User, "old"),
                    provider_message(lotta_runtime::ports::ProviderMessageRole::Assistant, "new"),
                ])
                .unwrap(),
                tools: lotta_domain::BoundedVec::new(Vec::new()).unwrap(),
                tool_choice: ProviderToolChoice::Auto,
                image_policy: ImagePolicy::Strict,
                context_tokens_max: TokenLimit::new(128_000).unwrap(),
                output_tokens_max: TokenLimit::new(64).unwrap(),
                reasoning: ReasoningControls {
                    enabled: false,
                    effort: None,
                    tier: None,
                },
                cancellation: CancellationToken::new(),
                context: None,
                deadline: lotta_runtime::ports::ProviderDeadline::default(),
            }
        }

        async fn seed_transcript(store: &LocalStore, scope: &RuntimeScope) {
            let now = Timestamp::parse_persisted_rfc3339("2026-08-18T00:00:00Z").unwrap();
            let manifest = lotta_domain::TranscriptManifest {
                schema_version: 2,
                message_format: lotta_domain::TranscriptMessageFormat::PiSessionEntryJsonl,
                provider_stack: lotta_domain::ProviderStack::PiAi,
                created_at: now,
                migrated_from: None,
                migrated_at: None,
                backup_path: None,
            };
            let session = lotta_domain::TranscriptEntry::Session(lotta_domain::SessionEntry {
                entry_type: lotta_domain::SessionEntryType::Session,
                id: NonEmptyString::new(format!("session-{}", scope.conversation_id.as_str()))
                    .unwrap(),
                version: 3,
                timestamp: now,
                cwd: "/".into(),
            });
            let directory = store
                .paths()
                .conversation_dir(&scope.agent_id, &scope.conversation_id)
                .unwrap();
            std::fs::create_dir_all(&directory).unwrap();
            store
                .initialize_transcript(&scope.agent_id, &scope.conversation_id, &manifest, &session)
                .await
                .unwrap();
        }

        fn contention_command(
            scope: &RuntimeScope,
            lease: &TurnLease,
        ) -> lotta_runtime::CompactionCommand {
            let request = compaction_request();
            let detail = lotta_runtime::ports::ProviderContextOverflowDetail {
                measured: None,
                estimated: lotta_runtime::ports::estimate_request_tokens(&request),
                limit: request.context_tokens_max.get(),
                provider: request.model.provider_id.as_str().to_owned(),
                model: request.model.handle.as_str().to_owned(),
                attempt: 1,
                compactions_completed: 0,
            };
            lotta_runtime::CompactionCommand {
                request_id: NonEmptyString::new("release-contention").unwrap(),
                scope: scope.clone(),
                lease: lease.clone(),
                request,
                detail,
                trigger: lotta_runtime::CompactionTrigger::Manual,
                mode: lotta_runtime::CompactionMode::All,
                cancellation: CancellationToken::new(),
            }
        }

        fn registered_owner<'a>(
            state: &'a RuntimeServiceState,
            scope: &RuntimeScope,
        ) -> &'a lotta_runtime::LifecycleOwner {
            state
                .registry
                .lookup(&RuntimeKey::from(scope))
                .and_then(|handle| state.registry.lifecycle(&handle))
                .expect("registered owner")
        }

        /// While an unrelated operation holds the authoritative registry, the
        /// authority release waits for the lock and then finishes the command:
        /// the lifecycle lands idle instead of being silently stranded busy.
        #[tokio::test]
        async fn authority_release_awaits_registry_contention_then_finishes_idle() {
            use lotta_app_server::ws::conversations::ConversationAuthority as _;
            let label = "compaction-release-contention";
            let held = Arc::new(HeldProvider::default());
            let (root, service, ..) =
                production_controller_fixture(label, held.clone() as Arc<dyn ProviderPort>).await;
            let authority = Arc::new(ConversationsProductionAuthority {
                state: Arc::clone(&service.state),
                brokers: Arc::clone(&service.brokers),
            });
            let scope = scope(label);
            let store = LocalStore::new(StorePaths::new(root.clone()).unwrap());
            seed_transcript(&store, &scope).await;

            let lease = authority.begin_command(&scope).expect("command begins");
            let settled = lease.clone();
            let progress = authority.compact(contention_command(&scope, &lease)).await;
            assert_eq!(progress.expect("compaction runs").messages_before, 2);

            // An unrelated operation holds the authoritative registry…
            let guard = service.state.inner.lock().await;
            let mut releaser = {
                let authority = Arc::clone(&authority);
                let scope = scope.clone();
                tokio::spawn(async move { authority.finish_command(&scope, &lease).await })
            };
            // …so the deterministic release must wait instead of skipping.
            if tokio::time::timeout(std::time::Duration::from_millis(250), &mut releaser)
                .await
                .is_ok()
            {
                panic!("release completed while the registry lock was still held");
            }
            drop(guard);
            tokio::time::timeout(std::time::Duration::from_secs(5), releaser)
                .await
                .expect("release settles once the lock frees")
                .expect("release task")
                .expect("deterministic release");

            // The lifecycle returned to idle once the lock freed.
            {
                let state = service.state.inner.lock().await;
                let owner = registered_owner(&state, &scope);
                assert_eq!(owner.projection().state(), TurnStateKind::Idle);
                assert!(!owner.is_current(&settled));
            }

            // The idle scope admits a fresh command lease again.
            let probe = authority.begin_command(&scope).expect("idle again");
            let state = service.state.inner.lock().await;
            assert!(registered_owner(&state, &scope).is_current(&probe));
            drop(state);
            assert!(authority.finish_command(&scope, &probe).await.is_ok());
        }
    }

    #[tokio::test]
    async fn production_three_concurrent_abort_paths_terminal_release_then_pump_once() {
        use lotta_runtime::turn::CancelStep;
        use std::sync::atomic::Ordering;

        for repetition in 0..10 {
            let label = format!("three-aborts-{repetition}");
            let held = Arc::new(HeldProvider::default());
            let (root, service, controller, ..) =
                production_controller_fixture(&label, held.clone() as Arc<dyn ProviderPort>).await;
            let scope = scope(&label);
            let observer = Arc::new(ProductionCancellationObserver::default());
            service.state.observe_cancellation(Arc::clone(&observer));
            let stage_observer: Arc<dyn Fn(CancelStep) + Send + Sync> = {
                let observer = Arc::clone(&observer);
                Arc::new(move |step| {
                    if step == CancelStep::ClaimAndCancel {
                        observer.record("claim");
                    }
                })
            };
            controller.observe_cancellation_stages(stage_observer);

            let first_command = command(scope.clone(), "active");
            let first = service
                .admit_input(first_command.clone())
                .await
                .expect("start active input");
            let deferred = lotta_app_server::ws::DeferredInput {
                scope: scope.clone(),
                disposition: first.disposition,
                continuation: first.continuation,
            };
            let connection_cancellation = CancellationToken::new();
            let sink = Arc::new(Sink::default());
            let turn = {
                let controller = Arc::clone(&controller);
                let sink = Arc::clone(&sink);
                let cancellation = connection_cancellation.clone();
                tokio::spawn(async move {
                    controller
                        .submit_turn(first_command, deferred, cancellation, sink)
                        .await
                })
            };
            let provider_entered =
                tokio::time::timeout(std::time::Duration::from_secs(5), held.waiting.acquire())
                    .await;
            if let Err(error) = provider_entered {
                panic!(
                    "provider wait entered: {error:?}; calls={}; result={:?}",
                    held.calls.load(Ordering::SeqCst),
                    turn.await
                );
            }
            provider_entered
                .expect("provider wait timeout checked")
                .expect("provider wait semaphore")
                .forget();
            assert_eq!(
                service
                    .admit_input(command(scope.clone(), "next"))
                    .await
                    .expect("queue next input")
                    .disposition,
                InputDisposition::Queued
            );

            let barrier = Arc::new(tokio::sync::Barrier::new(4));
            let abort = |request_id: &'static str| {
                let service = Arc::clone(&service);
                let scope = scope.clone();
                let barrier = Arc::clone(&barrier);
                tokio::spawn(async move {
                    barrier.wait().await;
                    service
                        .abort_message(AbortMessageCommand {
                            request_id: Some(NonEmptyString::new(request_id).unwrap()),
                            runtime: scope,
                            run_id: None,
                        })
                        .await
                })
            };
            let abort_a = abort("abort-a");
            let abort_b = abort("abort-b");
            let disconnect = {
                let barrier = Arc::clone(&barrier);
                let cancellation = connection_cancellation.clone();
                tokio::spawn(async move {
                    barrier.wait().await;
                    cancellation.cancel();
                })
            };
            barrier.wait().await;
            let outcome_a = abort_a.await.unwrap().unwrap();
            let outcome_b = abort_b.await.unwrap().unwrap();
            disconnect.await.unwrap();
            assert!(outcome_a.aborted && outcome_b.aborted);
            let settled = tokio::time::timeout(std::time::Duration::from_secs(5), turn)
                .await
                .expect("turn settles")
                .unwrap();
            assert!(settled.is_ok() || matches!(settled, Err(AppServerError::Internal)));

            let transcript_path = service
                .store
                .paths()
                .conversation_dir(&scope.agent_id, &scope.conversation_id)
                .unwrap()
                .join("messages.jsonl");
            let transcript = tokio::fs::read_to_string(transcript_path).await.unwrap();
            let durable_terminal = transcript
                .lines()
                .filter(|line| line.contains("turn_stop") && line.contains("user_cancellation"))
                .count();
            let events = sink.0.lock().unwrap();
            let cancelled = events
                .iter()
                .filter(|(_, event)| {
                    matches!(
                        event,
                        lotta_app_server::ws::RuntimeEvent::TurnFinished { stop_reason, .. }
                            if stop_reason.as_str() == "cancelled"
                    )
                })
                .count();
            let generic_finished = events
                .iter()
                .filter(|(_, event)| {
                    matches!(
                        event,
                        lotta_app_server::ws::RuntimeEvent::TurnFinished { stop_reason, .. }
                            if stop_reason.as_str() != "cancelled"
                    )
                })
                .count();
            let idle = events
                .iter()
                .filter(|(_, event)| {
                    matches!(
                        event,
                        lotta_app_server::ws::RuntimeEvent::UpdateLoopStatus { loop_status }
                            if loop_status.as_value()["status"] == "idle"
                    )
                })
                .count();
            drop(events);
            assert_eq!(observer.claims.load(Ordering::SeqCst), 1);
            assert_eq!(durable_terminal, 1);
            assert_eq!(cancelled, 1);
            assert_eq!(observer.releases.load(Ordering::SeqCst), 1);
            assert_eq!(observer.pumps.load(Ordering::SeqCst), 1);
            assert_eq!(generic_finished, 0);
            assert_eq!(idle, 1, "only the pumped successor may complete idle");
            assert_eq!(
                observer.order.lock().unwrap().as_slice(),
                &["claim", "persist", "cancelled", "release", "pump"]
            );
            assert_eq!(
                observer.pumps.load(Ordering::SeqCst),
                1,
                "next input started exactly once"
            );
            let _ = std::fs::remove_dir_all(root);
        }
    }

    fn approval_request(
        owner: RuntimeScope,
        lease_generation: u64,
    ) -> lotta_runtime::ApprovalRequest {
        use lotta_runtime::ports::{ToolInputSchema, ValidatedToolInput};
        let text = |value: &str| NonEmptyString::new(value.to_owned()).unwrap();
        lotta_runtime::ApprovalRequest {
            request_id: text("approval-held"),
            tool_call_id: text("call-held"),
            scope: owner,
            run_id: RunId::accept("run-held").unwrap(),
            turn_id: text("turn-held"),
            input_id: text("input-held"),
            lease_generation,
            tool_name: text("Read"),
            original_input: ValidatedToolInput::new(
                BoundedJsonValue::new(serde_json::json!({"path":"a"})).unwrap(),
            )
            .unwrap(),
            original_schema: ToolInputSchema::new(
                BoundedJsonValue::new(serde_json::json!({"type":"object"})).unwrap(),
            )
            .unwrap(),
            created_at: TestClock.now(),
            expires_at: Timestamp::parse_persisted_rfc3339("2026-08-19T00:00:00Z").unwrap(),
            state: lotta_runtime::ApprovalState::Pending,
            revision: 0,
        }
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
    async fn production_approval_allow_executes_once() {
        use lotta_runtime::ports::{ToolApprovalGrant, ToolCallId, ValidatedToolInput};
        let fixture = approved_execution_fixture();
        let definition = fixture.definition;
        let counter = fixture.counter;
        let port = fixture.port;
        let call_id = ToolCallId::from_name(ProviderName::new("approved-call".into()).unwrap());
        let request = |grant| ToolExecutionRequest {
            tool_call_id: call_id.clone(),
            approval_grant: grant,
            definition: definition.clone(),
            input: ValidatedToolInput::new(
                BoundedJsonValue::new(serde_json::json!({"path":"."})).unwrap(),
            )
            .unwrap(),
            cancellation: CancellationToken::new(),
            deadline: definition.timeout,
        };
        let absent = port
            .execute(request(ToolApprovalGrant::None))
            .await
            .unwrap_err();
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(matches!(
            absent,
            lotta_runtime::RuntimeError::AdapterFailure {
                code: "tool_pipeline",
                ..
            }
        ));
        let grant = ToolApprovalGrant::Granted {
            tool_call_id: call_id.clone(),
            internal_name: definition.internal_name.clone(),
        };
        let outcome = port.execute(request(grant)).await.unwrap();
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(format!("{outcome:?}").contains("approved"));
        assert!(fixture.snapshot.by_model("Read").is_some());
        let _ = std::fs::remove_dir_all(fixture.root);
    }

    #[tokio::test]
    async fn production_approval_response_resolves_while_turn_lock_is_held() {
        let service = service();
        let owner = scope("held-approval");
        let admitted = service
            .admit_input(command(owner.clone(), "held-input"))
            .await
            .unwrap();
        let id = admitted.continuation.as_ref().unwrap().as_value()["client_message_id"]
            .as_str()
            .unwrap();
        let pending = service.state.take_pending(&owner, id).unwrap();
        let request = service
            .approvals
            .store_request(approval_request(owner.clone(), pending.lease.generation()))
            .unwrap();
        let receiver = service.approvals.register_waiter(&request).unwrap();
        service.state.active.lock().unwrap().insert(
            RuntimeKey::from(&owner),
            ActiveAdmission {
                handle: pending.handle,
                lease: pending.lease,
                cancellation: pending.cancellation,
                queue: lotta_runtime::ConversationQueue::default(),
                history: lotta_domain::AdmissionHistory::default(),
            },
        );
        let _held = service.state.inner.lock().await;
        let response = InputCommand {
            request_id: Some(NonEmptyString::new("outer-held").unwrap()),
            runtime: owner.clone(),
            payload: BoundedJsonValue::new(serde_json::json!({
                "kind":"approval_response",
                "request_id":"approval-held",
                "decision":{"behavior":"allow"}
            }))
            .unwrap(),
        };
        let accepted = service.admit_input(response).await.unwrap();
        assert_eq!(accepted.disposition, InputDisposition::Started);
        service
            .continue_input(owner, accepted.continuation, Arc::new(Sink::default()))
            .await
            .unwrap();
        let outcome = receiver.await.unwrap();
        assert_eq!(outcome.resolution, lotta_runtime::ApprovalResolution::Allow);
        assert_eq!(
            outcome.request.state,
            lotta_runtime::ApprovalState::Executing
        );
    }

    #[tokio::test]
    async fn production_active_ordinary_queue_releases_and_pumps() {
        let service = service();
        let scope = scope("active-pump");
        let first = service
            .admit_input(command(scope.clone(), "first"))
            .await
            .expect("start first input");
        let pending = service
            .state
            .take_pending(&scope, "first")
            .expect("take pending input");
        service.state.active.lock().unwrap().insert(
            RuntimeKey::from(&scope),
            ActiveAdmission {
                handle: pending.handle.clone(),
                lease: pending.lease.clone(),
                cancellation: pending.cancellation.clone(),
                queue: lotta_runtime::ConversationQueue::default(),
                history: lotta_domain::AdmissionHistory::default(),
            },
        );
        let second = service
            .admit_input(command(scope.clone(), "second"))
            .await
            .expect("queue active ordinary input");
        assert_eq!(second.disposition, InputDisposition::Queued);
        let pumped = service
            .state
            .release_and_pump(&scope, pending, "completed", false)
            .await
            .expect("release and pump")
            .expect("pumped item");
        assert_eq!(pumped.0.client_message_id.as_str(), "second");
        assert_eq!(
            pumped
                .1
                .as_value()
                .get("client_message_id")
                .and_then(|v| v.as_str()),
            Some("second")
        );
        assert!(first.continuation.is_some());
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
        let events = sink.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, scope);
        assert_eq!(events[0].1.discriminant(), "turn_finished");
    }
    #[test]
    fn production_second_turn_includes_persisted_assistant_history() {
        let user = lotta_domain::LocalMessageRole::User;
        let assistant = lotta_domain::LocalMessageRole::Assistant;
        assert_ne!(user, assistant);
    }

    mod skills_and_cwd {
        use super::*;

        const TURN_TIMEOUT_SECS: u64 = 15;

        async fn run_one_turn(
            service: &Arc<ProductionRuntimeService>,
            controller: &Arc<ProductionTurnController>,
            provider: &Arc<CompletingProvider>,
            scope: RuntimeScope,
            message_id: &str,
        ) {
            let input = command(scope.clone(), message_id);
            let admitted = service
                .admit_input(input.clone())
                .await
                .expect("admit input");
            let deferred = lotta_app_server::ws::DeferredInput {
                scope,
                disposition: admitted.disposition,
                continuation: admitted.continuation,
            };
            let controller = Arc::clone(controller);
            let turn = tokio::spawn(async move {
                controller
                    .submit_turn(
                        input,
                        deferred,
                        CancellationToken::new(),
                        Arc::new(Sink(Mutex::new(Vec::new()))),
                    )
                    .await
            });
            let timeout = std::time::Duration::from_secs(TURN_TIMEOUT_SECS);
            // The completing provider signals its permit while streaming; the
            // recorded prompt is captured before either observable, so a plain
            // completion check covers both "reached the provider" and "finished".
            let outcome = tokio::time::timeout(timeout, turn)
                .await
                .expect("turn completion")
                .expect("join");
            assert!(
                provider.calls.load(std::sync::atomic::Ordering::SeqCst) > 0,
                "the turn must reach the provider: {outcome:?}"
            );
            outcome.expect("turn outcome");
        }

        fn last_prompt(provider: &Arc<CompletingProvider>) -> String {
            provider.last_prompt()
        }

        #[tokio::test]
        async fn enabled_skill_is_selected_next_turn_and_disable_removes_it() {
            let provider = Arc::new(CompletingProvider::default());
            let label = "skills-selection";
            let (root, service, controller, skills, _settings) =
                production_controller_fixture(label, provider.clone()).await;
            // The same scope the fixture persisted agent/conversation for.
            let scope = scope(label);
            let global_skills = root.join(".letta").join("skills");
            // The source lives inside the discovery root so Task 36's
            // canonical confinement accepts the linked copy.
            let source = global_skills.join("sources").join("demo-skill");
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(
                source.join("SKILL.md"),
                // The explicit id keeps discovery's canonical visited-set
                // dedup from renaming the skill to its physical source path.
                concat!(
                    "---\nid: demo-skill\nname: demo-skill\n",
                    "description: Demo skill wiring marker.\n---\nBody.\n"
                ),
            )
            .unwrap();
            skills.apply(
                0,
                &lotta_app_server::ws::skills::SkillsCommand::Enable(
                    lotta_app_server::ws::skills::SkillEnableCommand {
                        request_id: "en-1".to_owned(),
                        skill_path: source.display().to_string(),
                    },
                ),
            );
            assert!(
                global_skills.join("demo-skill").exists(),
                "enable links into <storage>/.letta/skills"
            );
            let selected = skills
                .selected_sources()
                .lock()
                .map(|selection| selection.ids())
                .unwrap_or_default();
            assert_eq!(selected, vec!["demo-skill".to_owned()]);

            run_one_turn(&service, &controller, &provider, scope.clone(), "skill-on").await;
            assert!(
                last_prompt(&provider).contains("Demo skill wiring marker."),
                "the enabled skill compiles into the next turn's prompt"
            );

            skills.apply(
                0,
                &lotta_app_server::ws::skills::SkillsCommand::Disable(
                    lotta_app_server::ws::skills::SkillDisableCommand {
                        request_id: "dis-1".to_owned(),
                        name: "demo-skill".to_owned(),
                    },
                ),
            );
            assert!(!global_skills.join("demo-skill").exists());
            run_one_turn(&service, &controller, &provider, scope, "skill-off").await;
            assert!(
                !last_prompt(&provider).contains("Demo skill wiring marker."),
                "the disabled skill leaves the next turn's prompt"
            );
        }

        #[tokio::test]
        async fn device_state_cwd_change_applies_next_turn_and_reminds_once_when_missing() {
            let provider = Arc::new(CompletingProvider::default());
            let label = "device-cwd";
            let (root, service, controller, _skills, settings) =
                production_controller_fixture(label, provider.clone()).await;
            // The same scope the fixture persisted agent/conversation for.
            let scope = scope(label);
            let gone = root.join("workspace").join("deleted-directory");
            let gone_text = gone.display().to_string();

            service
                .change_device_state(ChangeDeviceStateCommand {
                    runtime: scope.clone(),
                    payload: lotta_app_server::ws::command::ChangeDeviceStatePayload {
                        mode: None,
                        cwd: Some(gone_text.clone()),
                        agent_id: None,
                        conversation_id: None,
                    },
                })
                .await
                .expect("device state accepted");
            assert_eq!(
                settings.cwd_override(
                    Some(scope.agent_id.as_str()),
                    scope.conversation_id.as_str()
                ),
                Some(gone_text.clone()),
                "the change persists into the scoped cwd map"
            );

            run_one_turn(&service, &controller, &provider, scope.clone(), "cwd-1").await;
            assert!(
                last_prompt(&provider).contains(&gone_text),
                "the missing changed cwd falls back with its one-time reminder"
            );
            // The bridge records the reminder once: the turn's setup claimed
            // it, so no later turn re-records it without a fresh change.
            assert_eq!(
                settings.claim_missing_cwd_reminder(
                    Some(scope.agent_id.as_str()),
                    scope.conversation_id.as_str()
                ),
                None,
                "the one-time reminder was consumed by the next turn"
            );
        }
    }

    struct TempRoot(std::path::PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!("{label}-{}", next_test_root()));
            std::fs::create_dir_all(root.join("workspace")).unwrap();
            Self(root)
        }
    }

    impl std::ops::Deref for TempRoot {
        type Target = std::path::Path;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Builds one production service over a fresh store with its shared
    /// authoritative state exposed for queue-authority composition.
    fn device_queue_service(
        root: &std::path::Path,
    ) -> (Arc<ProductionRuntimeService>, Arc<ProductionRuntimeState>) {
        let paths = StorePaths::new(root.to_path_buf()).unwrap();
        let state = Arc::new(ProductionRuntimeState::new(Arc::new(
            lotta_runtime::observe::RuntimeObserver::default(),
        )));
        let approvals = Arc::new(lotta_runtime::ApprovalManager::new(
            LocalStore::new(paths.clone()).approval_journal(),
            Arc::new(crate::production_setup::ProductionEditedInputValidator),
        ));
        let settings = test_settings_bridge(paths.root(), paths.root());
        let service = ProductionRuntimeService::new(
            paths,
            Arc::new(TestClock),
            Arc::new(lotta_runtime::hooks::NoopHookRuntime),
            Arc::clone(&state),
            approvals,
            Arc::new(ProductionTurnBrokers::new()),
            settings,
        );
        (Arc::new(service), state)
    }

    /// Composes the WS device surface over one production authority and sink.
    fn device_bridge_with_authority(
        root: &std::path::Path,
        state: &Arc<ProductionRuntimeState>,
        messages: Arc<Mutex<Vec<(u64, DeviceMessage)>>>,
        sink: Arc<Sink>,
    ) -> Arc<DeviceBridge> {
        let sink_messages = Arc::clone(&messages);
        let forward: DeviceForwarder = Arc::new(move |connection, message| {
            sink_messages.lock().unwrap().push((connection, message));
            Ok(())
        });
        let workspace = root.join("workspace");
        let bridge = Arc::new(DeviceBridge::new(forward, &workspace, root).unwrap());
        bridge.register_queue_authority(Arc::new(ProductionQueueAuthority {
            state: Arc::clone(state),
        }));
        bridge.register_event_sink(sink as Arc<dyn RuntimeEventSink>);
        bridge
    }

    #[tokio::test]
    async fn ws_removal_syncs_with_the_production_registry() {
        let root = TempRoot::new("lotta-device-prod");
        let (service, state) = device_queue_service(&root);

        // Admit through the production service: turn start plus one queued input.
        let target = scope("device");
        let started = service
            .admit_input(command(target.clone(), "cm-start"))
            .await
            .unwrap();
        assert_eq!(started.disposition, InputDisposition::Started);
        let queued = service
            .admit_input(command(target.clone(), "cm-remove"))
            .await
            .unwrap();
        assert_eq!(queued.disposition, InputDisposition::Queued);

        // WS removal routed through the same authoritative registry.
        let messages: Arc<Mutex<Vec<(u64, DeviceMessage)>>> = Arc::default();
        let sink = Arc::new(Sink::default());
        let bridge =
            device_bridge_with_authority(&root, &state, Arc::clone(&messages), Arc::clone(&sink));
        bridge
            .apply(1, &decode_remove_queue_item(&target, "queue-cm-remove"))
            .await;

        // The direct answer reports success.
        let captured = messages.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        match &captured[0].1 {
            DeviceMessage::RemoveQueueItem(answer) => assert!(answer.success),
            other => panic!("unexpected device answer {other:?}"),
        }

        // The authoritative broadcast carries the cancelled transition keyed
        // by client_message_id and the remaining (empty) queue.
        let events = sink.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "one listener state event to subscribers");
        assert_eq!(events[0].0, target);
        match &events[0].1 {
            RuntimeEvent::UpdateQueue { queue, removed } => {
                assert_eq!(queue.as_value().as_array().map(Vec::len), Some(0));
                let transitions = removed.as_value().as_array().unwrap();
                assert_eq!(
                    transitions[0]["client_message_id"],
                    serde_json::json!("cm-remove")
                );
                assert_eq!(
                    transitions[0]["disposition"],
                    serde_json::json!("cancelled")
                );
            }
            other => panic!("unexpected runtime event {}", other.discriminant()),
        }

        // Sync: the production registry's queue is empty after WS removal.
        let guard = state.inner.lock().await;
        let handle = guard.registry.lookup(&RuntimeKey::from(&target)).unwrap();
        assert!(guard.registry.queue(&handle).expect("entry").is_empty());
    }

    /// Admits one input, spawns its real turn against [`HeldProvider`], and
    /// waits until the provider parked inside `stream()` — proving the
    /// registry lock is held by the executing turn. Returns the joined task
    /// handle plus the connection cancellation that releases the provider.
    async fn start_held_turn(
        controller: &Arc<ProductionTurnController>,
        service: &ProductionRuntimeService,
        held: &Arc<HeldProvider>,
        target: &RuntimeScope,
        client_message_id: &str,
    ) -> (
        tokio::task::JoinHandle<Result<(), AppServerError>>,
        CancellationToken,
    ) {
        let started = service
            .admit_input(command(target.clone(), client_message_id))
            .await
            .expect("start turn input");
        assert_eq!(started.disposition, InputDisposition::Started);
        let deferred = lotta_app_server::ws::DeferredInput {
            scope: target.clone(),
            disposition: started.disposition,
            continuation: started.continuation,
        };
        let cancellation = CancellationToken::new();
        let sink = Arc::new(Sink::default());
        let command = command(target.clone(), client_message_id);
        let controller = Arc::clone(controller);
        let token = cancellation.clone();
        let turn =
            tokio::spawn(
                async move { controller.submit_turn(command, deferred, token, sink).await },
            );
        tokio::time::timeout(std::time::Duration::from_secs(5), held.waiting.acquire())
            .await
            .expect("provider wait entered")
            .expect("provider wait semaphore")
            .forget();
        assert!(!turn.is_finished(), "provider release gate holds the turn");
        assert!(
            service.state.inner.try_lock().is_err(),
            "executing turn holds the production registry mutex"
        );
        (turn, cancellation)
    }

    struct HeldRemovalFixture {
        _root: TempRoot,
        service: Arc<ProductionRuntimeService>,
        controller: Arc<ProductionTurnController>,
        held: Arc<HeldProvider>,
        target: RuntimeScope,
        observer: Arc<ProductionCancellationObserver>,
    }

    async fn held_removal_fixture() -> HeldRemovalFixture {
        let held = Arc::new(HeldProvider::default());
        let label = "device-held";
        let (fixture_root, service, controller, ..) =
            production_controller_fixture(label, held.clone() as Arc<dyn ProviderPort>).await;
        let root = TempRoot(fixture_root);
        let target = scope(label);
        let observer = Arc::new(ProductionCancellationObserver::default());
        service.state.observe_cancellation(Arc::clone(&observer));
        HeldRemovalFixture {
            _root: root,
            service,
            controller,
            held,
            target,
            observer,
        }
    }

    async fn assert_removed_item_stays_absent(
        service: &ProductionRuntimeService,
        target: &RuntimeScope,
    ) {
        let state = service.state.inner.lock().await;
        let handle = state
            .registry
            .lookup(&RuntimeKey::from(target))
            .expect("held-turn registry entry");
        assert!(
            state
                .registry
                .queue(&handle)
                .expect("held-turn queue")
                .is_empty(),
            "removed item stays absent after release"
        );
    }

    /// A WS removal issued while a turn executes cancels the item on the
    /// active admission's private queue: the post-turn transfer never carries
    /// it, the pump never runs it, and the removed disposition plus snapshot
    /// are emitted to subscribers.
    #[tokio::test]
    async fn ws_removal_during_held_turn_cancels_before_pump() {
        use std::sync::atomic::Ordering;

        let fixture = held_removal_fixture().await;
        let HeldRemovalFixture {
            service,
            controller,
            held,
            target,
            observer,
            ..
        } = &fixture;

        let (turn, connection_cancellation) =
            start_held_turn(controller, service, held, target, "cm-start").await;

        // The next ordinary input queues onto the active admission queue.
        assert_eq!(
            service
                .admit_input(command(target.clone(), "cm-remove"))
                .await
                .expect("queue during turn")
                .disposition,
            InputDisposition::Queued,
            "the input rides the active admission queue while the lock is held"
        );

        let messages: Arc<Mutex<Vec<(u64, DeviceMessage)>>> = Arc::default();
        let queue_events = Arc::new(Sink::default());
        let bridge = device_bridge_with_authority(
            &fixture._root,
            &service.state,
            Arc::clone(&messages),
            Arc::clone(&queue_events),
        );
        bridge
            .apply(1, &decode_remove_queue_item(target, "queue-cm-remove"))
            .await;

        assert_removed_answer(&messages);
        assert_cancelled_broadcast(&queue_events);

        assert!(!turn.is_finished(), "removal leaves the held turn parked");
        assert!(
            service.state.inner.try_lock().is_err(),
            "registry stays held through the real removal and snapshot"
        );

        // Release the turn after deleting its only queued successor.
        held.release();
        connection_cancellation.cancel();
        let settled = tokio::time::timeout(std::time::Duration::from_secs(5), turn)
            .await
            .expect("turn settles after release")
            .expect("turn task joins");
        settled.expect("cancellation release path is clean");
        assert_eq!(
            observer.pumps.load(Ordering::SeqCst),
            0,
            "the removed item is never pumped"
        );
        assert_eq!(
            held.calls.load(Ordering::SeqCst),
            1,
            "no successor turn starts for the removed item"
        );
        assert_removed_item_stays_absent(service, target).await;
        drop(bridge);
    }

    /// Asserts the direct `remove_queue_item` answer reported success.
    fn assert_removed_answer(messages: &Mutex<Vec<(u64, DeviceMessage)>>) {
        let captured = messages.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        match &captured[0].1 {
            DeviceMessage::RemoveQueueItem(answer) => {
                assert!(answer.success, "mid-turn removal is authoritative");
                assert_eq!(answer.item_id, "queue-cm-remove");
            }
            other => panic!("unexpected device answer {other:?}"),
        }
    }

    /// Asserts exactly one `update_queue` broadcast carrying the cancelled
    /// transition keyed by client_message_id and an empty remaining snapshot.
    fn assert_cancelled_broadcast(sink: &Sink) {
        let events = sink.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "one listener state event");
        match &events[0].1 {
            RuntimeEvent::UpdateQueue { queue, removed } => {
                assert_eq!(queue.as_value().as_array().map(Vec::len), Some(0));
                let transitions = removed.as_value().as_array().unwrap();
                assert_eq!(
                    transitions[0]["client_message_id"],
                    serde_json::json!("cm-remove")
                );
                assert_eq!(
                    transitions[0]["disposition"],
                    serde_json::json!("cancelled")
                );
            }
            other => panic!("unexpected runtime event {}", other.discriminant()),
        }
    }

    fn decode_remove_queue_item(
        scope: &RuntimeScope,
        item_id: &str,
    ) -> lotta_app_server::ws::device::DeviceCommand {
        lotta_app_server::ws::device::DeviceCommand::RemoveQueueItem(
            lotta_app_server::ws::device::RemoveQueueItemPayload {
                request_id: "rq-prod".to_owned(),
                runtime: scope.clone(),
                item_id: item_id.to_owned(),
            },
        )
    }

    /// The shell-bundle background source maps real manager sessions into the
    /// pinned bash summary union; an unused bundle reports an empty list.
    #[test]
    fn shell_background_source_maps_session_summaries() {
        let root = std::env::temp_dir().join(format!("lotta-bg-src-{}", next_test_root()));
        std::fs::create_dir_all(root.join("workspace")).unwrap();
        let workspace = root.join("workspace");
        let sandbox: Arc<dyn lotta_tools::builtin::shell::ShellSandbox> = Arc::new(
            OsSandbox::detect(workspace_policy(&root, &workspace).unwrap()),
        );
        let shell =
            ShellToolBundle::new(&workspace.canonicalize().unwrap(), scope("bg"), sandbox).unwrap();
        let source = ShellBackgroundProcesses {
            shell: Arc::new(shell),
        };
        assert!(source.snapshot().is_empty());
    }

    fn next_test_root() -> u128 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        u128::from(COUNTER.fetch_add(1, Ordering::SeqCst))
            + std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_millis())
                .unwrap_or(0)
    }
}
