//! Concrete production turn-setup composition.

use lotta_domain::TurnLease;
use lotta_domain::{
    Agent, AgentId, BoundedJsonValue, CompactionEntry, CompactionEntryType, Conversation,
    ConversationId, InContextMessageIds, LocalMessage, LocalMessageRole, MessageEntry,
    MessageEntryType, MessageId, ModelDescriptor, NonEmptyString, PermissionMode, ProviderStack,
    SessionEntry, SessionEntryType, Timestamp, TranscriptEntry, TranscriptManifest,
    TranscriptMessageFormat,
};
use lotta_extensions::{
    hooks::{
        events::{self, HookEvent, HookRuntime},
        loader::{HookRegistry, HookRegistrySnapshot},
    },
    mods::registry::{ModRegistries, ModRuntimeSnapshot},
    skills::{SkillDiscovery, SkillRoots, SkillSources},
};
use lotta_memfs::{
    CacheRoot, DeliveryCapability, GitMemFs, PromptCompiler, PromptInputs, PromptSections,
    PromptSkill, PromptText,
};
use lotta_providers::{
    context::{ContextWindowSources, effective_window},
    model::{ModelHandle, ModelOverride, resolve_model},
};
use lotta_runtime::boundary::{InitialMemoryBlocks, ProviderName, ProviderText};
use lotta_runtime::hooks::{
    HookFailure, HookLifecycleHost, HookPayload, LifecycleError, LifecycleOperation,
};
use lotta_runtime::ports::{
    AgentStore, ConversationStore, ImagePolicy, MemFsPort, ModelFacingToolName, ProviderContent,
    ProviderContentPart, ProviderContext, ProviderDeadline, ProviderEvent, ProviderMessage,
    ProviderMessageRole, ProviderMessages, ProviderPort, ProviderRequest, ProviderToolChoice,
    ProviderToolDefinition, ProviderTools, ReasoningControls, StopReason, TokenLimit,
    provider_event_channel,
};
use lotta_runtime::retry::{FallbackRoute, ProviderRoute};
use lotta_runtime::turn::{
    AdmissionReceipt, CwdFailure, CwdResolution, ExtensionSnapshot, ReminderClaim,
    ResolvedTurnModel, SetupError, SetupFailure, SetupInput, SetupOrchestrator, SetupOutput,
    SetupPorts, SetupScopeHandle, SetupStatus, SetupStatusSink, SetupToolSource, SkillInventory,
    ToolCandidate, TurnPorts, TurnRunOutcome, TurnToolCatalog,
};
use lotta_runtime::{
    CompactionCommand, CompactionEffects, CompactionMode, CompactionRecovery, CompactionService,
    CompactionSummarizer, CompactionSummary, CompactionTrigger, RuntimeError,
};
use lotta_runtime::{ListenerRuntime, RuntimeHandle};
use lotta_store::{
    CompactionProjection, CompactionTransactionState, LocalStore, LottaStorageLock, MemoryPushJob,
    PostTurnExecution, PostTurnJob, PostTurnJobRunner, PostTurnQueue, ReflectionJob, StoreError,
    StoreErrorKind, StorePaths, WriteMode, atomic_write,
};
use lotta_tools::{
    PermissionDecision, PermissionInvocation, PermissionPolicy, ToolRegistration, ToolRegistry,
    ToolsetId, WorkspacePolicy, names,
    permissions::scopes::{PermissionSourcePaths, load_permissions},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const REMINDERS_MAX: usize = 1_024;
const REMINDER_FILE_BYTES_MAX: u64 = 1_048_576;
const REMINDER_TOKEN_BYTES_MAX: usize = 256;
const SETUP_DEADLINE_SECONDS: u64 = 60;
const REMINDER_PENDING_TTL_SECONDS: u64 = SETUP_DEADLINE_SECONDS + 5;
const REMINDER_PENDING_TTL_MS: u64 = REMINDER_PENDING_TTL_SECONDS * 1_000;
const REMINDER_LOCK_RETRY_MS: u64 = 2;
const REMINDER_LOCK_RETRY_MAX: usize = 500;
const REMINDER_STATE_REVISION: u64 = 1;
const CWD_REMINDER_PATH_BYTES_MAX: usize = 4_096;
const RESOLVE_AGENT_CHANNEL_CAPACITY: usize = 64;
const PRODUCTION_BROKER_WAITERS_MAX: usize = 1_024;
const TRANSCRIPT_MANIFEST_SCHEMA_VERSION: u8 = 2;
const TRANSCRIPT_SESSION_SCHEMA_VERSION: u8 = 3;

/// Required concrete configuration for production setup.
pub struct ProductionSetupConfig {
    /// Validated local-backend layout.
    pub store_paths: StorePaths,
    /// Explicit skill filesystem authorities.
    pub skill_roots: SkillRoots,
    /// Concrete model catalog entries available to this process.
    pub models: Vec<ModelDescriptor>,
    /// Required fallback model handle.
    pub default_model: ModelHandle,
    /// Persistent redacted provider connections available to setup.
    pub connections: Vec<lotta_providers::connections::ConnectionSnapshot>,
    /// Server context-window ceiling.
    pub server_context_window: u64,
    /// Output token ceiling.
    pub output_tokens: u64,
    /// Shared Task 32 registry, already populated with built-ins by composition.
    pub registry: Arc<ToolRegistry>,
    /// Shared runtime-scoped Task 40 lifecycle authority.
    pub tasks: Arc<lotta_tools::builtin::task::TaskLifecyclePort>,
    /// Shared canonical mod registries over the same Task 32 registry.
    pub mod_registries: Arc<ModRegistries>,
    /// Shared canonical hook registry populated by startup.
    pub hook_registry: Arc<HookRegistry>,
    /// Shared canonical hook runtime built by composition.
    pub hook_runtime: Arc<dyn HookRuntime>,
    /// Exact permission setting sources loaded for every turn scope.
    pub permission_sources: PermissionSourcePaths,
    /// Validated workspace confinement policy.
    pub workspace_policy: WorkspacePolicy,
    /// Trusted fallback toolset, overridden by agent settings when present.
    pub toolset: ToolsetId,
    /// Trusted fallback allowlist, overridden by agent settings when present.
    pub allowlist: Option<Vec<String>>,
}

fn provider_route(
    model: &ModelDescriptor,
    provider: &dyn lotta_runtime::ports::ProviderPort,
) -> ProviderRoute {
    let transport = provider
        .route_hint(model.provider_id.as_str())
        .unwrap_or("configured");
    ProviderRoute::new(transport, model.provider_id.as_str())
}

fn configured_fallbacks<'a>(
    prepared: &SetupOutput,
    source_provider: &crate::production_components::ProductionProviderPort,
    fallback_providers: &'a [crate::production_components::ProductionProviderPort],
) -> Result<Vec<lotta_runtime::turn::ConfiguredFallback<'a>>, RuntimeError> {
    let source_route = provider_route(&prepared.model.model, source_provider);
    prepared
        .fallback_candidates
        .iter()
        .cloned()
        .zip(fallback_providers)
        .map(|(destination, fallback_provider)| {
            let destination_route = provider_route(&destination, fallback_provider);
            let route = FallbackRoute::new(source_route.clone(), destination_route, destination)
                .map_err(|error| RuntimeError::InvalidData {
                    context: error.to_string(),
                })?;
            Ok(lotta_runtime::turn::ConfiguredFallback {
                route,
                provider: fallback_provider,
            })
        })
        .collect()
}

/// Live application-loop status boundary.
pub trait ProductionStatusSink: Send + Sync {
    /// Emits one turn status to the owning server loop.
    ///
    /// # Errors
    /// Returns a stable setup error when delivery fails.
    fn emit(&self, status: SetupStatus) -> Result<(), SetupError>;
}

/// Real owned adapters used by the setup orchestrator.
pub struct ProductionSetupPorts {
    store: LocalStore,
    memfs: GitMemFs,
    skills: SkillRoots,
    models: Vec<ModelDescriptor>,
    default_model: ModelHandle,
    connections: Vec<lotta_providers::connections::ConnectionSnapshot>,
    server_context_window: u64,
    output_tokens: u64,
    registry: Arc<ToolRegistry>,
    tasks: Arc<lotta_tools::builtin::task::TaskLifecyclePort>,
    mod_registries: Arc<ModRegistries>,
    hook_registry: Arc<HookRegistry>,
    hook_runtime: Arc<dyn HookRuntime>,
    extension_snapshots: Mutex<HashMap<u64, CanonicalExtensionSnapshot>>,
    scope_snapshots: Mutex<HashMap<u64, ProductionScope>>,
    tool_snapshots: Mutex<HashMap<u64, Arc<lotta_tools::RegistrySnapshot>>>,
    next_scope_snapshot: AtomicU64,
    next_extension_snapshot: AtomicU64,
    next_tool_snapshot: AtomicU64,
    permission_sources: PermissionSourcePaths,
    workspace_policy: WorkspacePolicy,
    toolset: ToolsetId,
    allowlist: Option<Vec<String>>,
    reminders_path: PathBuf,
    prompt_caches: Mutex<HashMap<(AgentId, ConversationId), Arc<CacheRoot>>>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl ProductionSetupPorts {
    /// Runs a production turn through concrete setup and generic real turn ports.
    ///
    /// # Errors
    /// Returns setup, status, provider, tool, or effect failures.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit production dependency injection"
    )]
    pub(crate) async fn run_production_turn(
        &self,
        runtime: &mut ListenerRuntime,
        handle: RuntimeHandle,
        lease: TurnLease,
        input: SetupInput,
        approval: &dyn lotta_runtime::turn::ApprovalPort,
        controller_tools: &dyn lotta_runtime::turn::ControllerToolPort,
        compaction: &dyn lotta_runtime::turn::CompactionPort,
        refresh: Arc<dyn lotta_runtime::turn::RequestRefreshPort>,
        children: &dyn lotta_runtime::turn::TurnChildOwner,
        post_turn: &dyn lotta_runtime::turn::PostTurnPort,
        provider: &crate::production_components::ProductionProviderPort,
        tools: &crate::production_components::ProductionToolPort,
        effects: &dyn lotta_runtime::turn::TurnEffectPort,
        #[cfg(test)] cancellation_stage_observer: Option<
            crate::production_components::CancellationStageObserver,
        >,
    ) -> Result<TurnRunOutcome, RuntimeError> {
        let cancellation = input.cancellation.clone();
        let turn_agent_id = input.agent_id.clone();
        let setup = SetupOrchestrator::new(self);
        let prepared = setup.prepare(input).await.map_err(setup_failure_runtime)?;
        debug_assert_eq!(prepared.status, SetupStatus::Sending);
        let hook_runtime = ProductionSnapshotHookRuntime { setup: self };
        let host = HookLifecycleHost::with_snapshot(&hook_runtime, prepared.extensions.id());
        let stop_payload = events::lifecycle_payload(HookEvent::Stop).map_err(runtime_adapter)?;
        let admission = prepared.admission.clone();
        let status = ProductionDispatchStatus {
            sink: prepared.status_sink.as_ref(),
        };
        let fallback_providers = self.fallback_providers(&prepared, provider)?;
        let scoped_tools = self.scoped_tools(&prepared, tools, &turn_agent_id)?;
        refresh.seed(&prepared).map_err(runtime_adapter)?;
        let refreshed = prepared.request.clone();
        let ports = TurnPorts::new(provider, &scoped_tools, &prepared.tools, effects)
            .with_approvals(approval)
            .with_controller_tools(controller_tools)
            .with_context_ports(compaction, refresh)
            .with_cancellation_ports(children, post_turn);
        self.release_scope(prepared.scope);
        let fallbacks = configured_fallbacks(&prepared, provider, &fallback_providers)?;
        let ports = ports.with_fallbacks(fallbacks).with_provider_start(&status);
        #[cfg(test)]
        let result = lotta_runtime::turn::run_turn_observed(
            runtime,
            handle,
            lease,
            refreshed,
            ports,
            cancellation_stage_observer,
        )
        .await;
        #[cfg(not(test))]
        let result = lotta_runtime::turn::run_turn(runtime, handle, lease, refreshed, ports).await;
        self.finalize_turn(
            &host,
            stop_payload,
            cancellation,
            &admission,
            move || async move { result },
        )
        .await
    }

    /// Fires the stop lifecycle hook over one completed turn and maps hook or
    /// block failures onto the linked interrupted-admission record.
    async fn finalize_turn(
        &self,
        host: &HookLifecycleHost<'_>,
        stop_payload: HookPayload,
        cancellation: CancellationToken,
        admission: &AdmissionReceipt,
        result: impl LifecycleOperation<TurnRunOutcome, RuntimeError>,
    ) -> Result<TurnRunOutcome, RuntimeError> {
        match host.stop(stop_payload, cancellation, result).await {
            Ok(outcome) => Ok(outcome),
            Err(LifecycleError::Operation(error)) => {
                self.record_interrupted(admission, &SetupError::Adapter(error.to_string()))
                    .await?;
                Err(error)
            }
            Err(error) => {
                let setup_error = lifecycle_setup_error(error);
                self.record_interrupted(admission, &setup_error).await?;
                Err(setup_failure_runtime(
                    lotta_runtime::turn::SetupFailure::PostAdmission {
                        receipt: admission.clone(),
                        error: setup_error,
                    },
                ))
            }
        }
    }

    fn fallback_providers(
        &self,
        prepared: &SetupOutput,
        provider: &crate::production_components::ProductionProviderPort,
    ) -> Result<Vec<crate::production_components::ProductionProviderPort>, RuntimeError> {
        prepared
            .fallback_candidates
            .iter()
            .map(|candidate| {
                provider
                    .for_route(candidate.provider_id.as_str())
                    .map_err(setup_error_runtime)
            })
            .collect()
    }

    fn scoped_tools(
        &self,
        prepared: &SetupOutput,
        tools: &crate::production_components::ProductionToolPort,
        agent_id: &lotta_domain::AgentId,
    ) -> Result<crate::production_components::ScopedProductionToolPort, RuntimeError> {
        let scope = self
            .scope_snapshot(prepared.scope)
            .map_err(setup_error_runtime)?;
        let snapshot = prepared
            .tools
            .snapshot()
            .and_then(|handle| self.take_tool_snapshot(handle))
            .ok_or_else(|| RuntimeError::NotFound {
                context: "turn tool snapshot unavailable".into(),
            })?;
        let snapshot = self.scoped_task_snapshot(snapshot, &scope.runtime)?;
        Ok(tools.scoped(
            snapshot,
            Arc::new(scope.permissions),
            Arc::new(self.workspace_policy.clone()),
            // WS-applied agent secrets resolve for tool pipelines running on
            // this exact agent's behalf (Task 52 side-store record).
            Some(Arc::new(
                lotta_app_server::ws::device_support::AgentSecretResolver::over_storage(
                    self.store.paths().root(),
                    agent_id.as_str(),
                ),
            )),
            scope.cwd,
        ))
    }

    fn scoped_task_snapshot(
        &self,
        snapshot: Arc<lotta_tools::RegistrySnapshot>,
        scope: &lotta_domain::RuntimeScope,
    ) -> Result<Arc<lotta_tools::RegistrySnapshot>, RuntimeError> {
        let (toolset, mut registrations) = snapshot.complete_registrations();
        let scoped =
            lotta_tools::builtin::task::registrations(Arc::clone(&self.tasks), scope.clone())
                .map_err(|_| RuntimeError::AdapterFailure {
                    code: "scoped_task_tools",
                    context: "runtime-scoped task lifecycle".into(),
                })?;
        for replacement in scoped {
            let name = replacement.definition.internal_name.as_str();
            if let Some(current) = registrations
                .iter_mut()
                .find(|item| item.definition.internal_name.as_str() == name)
            {
                *current = replacement;
            }
        }
        let registry =
            lotta_tools::ToolRegistry::new(registrations).map_err(Self::tool_registry_runtime)?;
        registry
            .compose(toolset, &[], None)
            .map_err(Self::tool_registry_runtime)
    }

    fn tool_registry_runtime(_: lotta_tools::RegistryError) -> RuntimeError {
        RuntimeError::AdapterFailure {
            code: "scoped_tool_registry",
            context: "runtime-scoped tool snapshot".into(),
        }
    }

    /// Returns the configured lifecycle hook runtime.
    #[must_use]
    pub fn hook_runtime(&self) -> Arc<dyn HookRuntime> {
        Arc::clone(&self.hook_runtime)
    }

    /// Returns the validated production workspace policy.
    #[must_use]
    pub fn workspace_policy(&self) -> &WorkspacePolicy {
        &self.workspace_policy
    }

    /// Returns the shared canonical tool registry used by production execution.
    #[must_use]
    pub fn registry(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.registry)
    }

    /// Returns the shared Task 45 mod registry backing device commands.
    #[must_use]
    pub fn mod_registries(&self) -> Arc<ModRegistries> {
        Arc::clone(&self.mod_registries)
    }

    fn take_tool_snapshot(
        &self,
        handle: lotta_runtime::ToolSnapshotHandle,
    ) -> Option<Arc<lotta_tools::RegistrySnapshot>> {
        self.tool_snapshots.lock().ok()?.remove(&handle.id())
    }

    fn scope_snapshot(&self, handle: SetupScopeHandle) -> Result<ProductionScope, SetupError> {
        self.scope_snapshots
            .lock()
            .map_err(|_| SetupError::Adapter("scope snapshot lock".into()))?
            .get(&handle.id())
            .cloned()
            .ok_or_else(|| SetupError::Adapter("scope snapshot unavailable".into()))
    }

    /// Constructs production adapters and validates required model/tool configuration.
    pub fn new(config: ProductionSetupConfig) -> Result<Self, SetupError> {
        let root = config.store_paths.root().to_path_buf();
        std::fs::create_dir_all(&root).map_err(adapter)?;
        let memfs = GitMemFs::new(root.clone()).map_err(SetupError::from)?;
        let registry = config.registry;
        let mod_registries = config.mod_registries;
        let hook_registry = config.hook_registry;
        let default = config.default_model.to_string();
        if !config
            .models
            .iter()
            .any(|model| model.handle.as_str() == default)
        {
            return Err(SetupError::Availability(default));
        }
        Ok(Self {
            store: LocalStore::new(config.store_paths),
            memfs,
            skills: config.skill_roots,
            models: config.models,
            default_model: config.default_model,
            connections: config.connections,
            server_context_window: config.server_context_window,
            output_tokens: config.output_tokens,
            registry,
            tasks: config.tasks,
            mod_registries,
            hook_registry,
            hook_runtime: config.hook_runtime,
            extension_snapshots: Mutex::new(HashMap::new()),
            next_extension_snapshot: AtomicU64::new(1),
            scope_snapshots: Mutex::new(HashMap::new()),
            next_scope_snapshot: AtomicU64::new(1),
            tool_snapshots: Mutex::new(HashMap::new()),
            next_tool_snapshot: AtomicU64::new(1),
            permission_sources: config.permission_sources,
            workspace_policy: config.workspace_policy,
            toolset: config.toolset,
            allowlist: config.allowlist,
            reminders_path: root.join("settings").join("turn-setup-reminders.json"),
            prompt_caches: Mutex::new(HashMap::new()),
            now_ms: Arc::new(system_epoch_ms),
        })
    }

    fn prompt_cache(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
    ) -> Result<Arc<CacheRoot>, RuntimeError> {
        let key = (agent.clone(), conversation.clone());
        let mut caches = self
            .prompt_caches
            .lock()
            .map_err(|_| runtime_adapter("prompt cache registry"))?;
        if let Some(cache) = caches.get(&key) {
            return Ok(cache.clone());
        }
        let directory = self
            .store
            .paths()
            .conversation_dir(agent, conversation)
            .map_err(runtime_adapter)?;
        std::fs::create_dir_all(&directory).map_err(runtime_adapter)?;
        let canonical = std::fs::canonicalize(&directory).map_err(runtime_adapter)?;
        let cache = Arc::new(CacheRoot::new(&canonical)?);
        caches.insert(key, Arc::clone(&cache));
        Ok(cache)
    }

    fn capture_extension_snapshot(&self) -> Result<ExtensionSnapshot, SetupError> {
        let snapshot = CanonicalExtensionSnapshot {
            mods: self.mod_registries.runtime().map_err(adapter)?,
            hooks: self.hook_registry.snapshot().map_err(adapter)?,
        };
        let id = self.next_extension_snapshot.fetch_add(1, Ordering::Relaxed);
        self.extension_snapshots
            .lock()
            .map_err(|_| SetupError::Adapter("extension snapshot lock".into()))?
            .insert(id, snapshot);
        Ok(ExtensionSnapshot::new(id))
    }

    fn extension_snapshot(
        &self,
        snapshot: &ExtensionSnapshot,
    ) -> Result<CanonicalExtensionSnapshot, SetupError> {
        self.extension_snapshots
            .lock()
            .map_err(|_| SetupError::Adapter("extension snapshot lock".into()))?
            .get(&snapshot.id())
            .cloned()
            .ok_or_else(|| SetupError::Adapter("extension snapshot unavailable".into()))
    }

    fn discover_skills(
        &self,
        cwd: &Path,
    ) -> Result<Vec<lotta_extensions::skills::Skill>, SetupError> {
        let mut roots = self.skills.clone();
        roots.project_working_root = cwd.to_path_buf();
        SkillDiscovery::discover(&roots, SkillSources::ALL).map_err(debug_adapter)
    }

    async fn update_reminders<T>(
        &self,
        cancellation: &CancellationToken,
        update: impl FnOnce(&mut ReminderState) -> Result<T, SetupError>,
    ) -> Result<T, SetupError> {
        let _lock = acquire_reminder_lock(self.store.paths().root(), cancellation).await?;
        let mut state = read_reminders(&self.reminders_path)?;
        recover_consumed(&self.store, &mut state)?;
        let result = update(&mut state)?;
        save_reminders(&self.reminders_path, &state)?;
        Ok(result)
    }

    #[cfg(test)]
    fn with_clock(mut self, clock: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        self.now_ms = Arc::new(clock);
        self
    }
}

#[derive(Deserialize, Serialize)]
struct ReminderState {
    revision: u64,
    entries: BTreeMap<String, ReminderEntry>,
}

impl Default for ReminderState {
    fn default() -> Self {
        Self {
            revision: REMINDER_STATE_REVISION,
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ReminderEntry {
    Pending {
        token: String,
        owner_turn: String,
        input_id: String,
        original_path: PathBuf,
        fallback_path: PathBuf,
        created_at_epoch_ms: u64,
        revision: u64,
    },
    Consumed {
        input_id: String,
        revision: u64,
    },
}

#[derive(Clone)]
struct ProductionScope {
    runtime: lotta_domain::RuntimeScope,
    cwd: PathBuf,
    permissions: PermissionPolicy,
}

struct BuildProviderRequest {
    store: LocalStore,
    agent: AgentId,
    conversation: ConversationId,
    prompt: String,
    input: String,
    input_id: Option<String>,
    model: ResolvedTurnModel,
    tools: Result<Vec<ProviderToolDefinition>, SetupError>,
    server_context_window: u64,
    cancellation: CancellationToken,
}

async fn build_provider_request(
    parts: BuildProviderRequest,
) -> Result<ProviderRequest, SetupError> {
    let mut history = load_transcript_messages(&parts.store, &parts.agent, &parts.conversation)
        .await
        .or_else(|error| {
            if error.kind() == StoreErrorKind::NotFound {
                Ok(Vec::new())
            } else {
                Err(error)
            }
        })
        .map_err(adapter)?;
    if let Some(input_id) = parts.input_id.as_deref() {
        history.retain(|message| message.id.as_str() != input_id);
    }
    let mut messages = history
        .iter()
        .map(local_provider_message)
        .collect::<Result<Vec<_>, _>>()?;
    messages.push(provider_text_message(
        ProviderMessageRole::User,
        &parts.input,
    )?);
    Ok(ProviderRequest {
        model: parts.model.model.clone(),
        system_prompt: Some(ProviderText::new(parts.prompt).map_err(SetupError::from)?),
        messages: ProviderMessages::new(messages).map_err(adapter)?,
        tools: ProviderTools::new(parts.tools?).map_err(adapter)?,
        tool_choice: ProviderToolChoice::Auto,
        image_policy: ImagePolicy::Strict,
        context_tokens_max: TokenLimit::new(parts.model.context_window)
            .map_err(SetupError::from)?,
        output_tokens_max: TokenLimit::new(parts.model.output_tokens).map_err(SetupError::from)?,
        reasoning: ReasoningControls {
            enabled: false,
            effort: None,
            tier: None,
        },
        cancellation: parts.cancellation,
        context: Some(ProviderContext {
            server_max: Some(parts.server_context_window),
            catalog_max: parts.model.model.context_window,
            conversation_max: Some(parts.model.context_window),
            ..ProviderContext::default()
        }),
        deadline: ProviderDeadline::default(),
    })
}

impl SetupPorts for ProductionSetupPorts {
    fn agents(&self) -> &dyn AgentStore {
        &self.store
    }
    fn conversations(&self) -> &dyn ConversationStore {
        &self.store
    }

    fn resolve_conversation(
        &self,
        requested_agent: &AgentId,
        conversation: &ConversationId,
        cancellation: &CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, Conversation> {
        let store = self.store.clone();
        let requested_agent = requested_agent.clone();
        let conversation = conversation.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            match ConversationStore::load(&store, &requested_agent, &conversation).await {
                Ok(value) => Ok(value),
                Err(RuntimeError::NotFound { .. }) | Err(RuntimeError::InvalidData { .. }) => {
                    let (items, mut records) =
                        tokio::sync::mpsc::channel(RESOLVE_AGENT_CHANNEL_CAPACITY);
                    let producer = {
                        let store = store.clone();
                        let cancellation = cancellation.clone();
                        tokio::spawn(async move { store.list(items, cancellation).await })
                    };
                    while let Some(agent) = records.recv().await {
                        match ConversationStore::load(&store, &agent.id, &conversation).await {
                            Ok(value) => {
                                producer.abort();
                                return Ok(value);
                            }
                            Err(RuntimeError::NotFound { .. })
                            | Err(RuntimeError::InvalidData { .. }) => {}
                            Err(error) => {
                                producer.abort();
                                return Err(error);
                            }
                        }
                    }
                    producer.await.map_err(|_| RuntimeError::AdapterFailure {
                        code: "conversation_list_join",
                        context: "conversation resolution".into(),
                    })??;
                    Err(RuntimeError::NotFound {
                        context: "conversation".into(),
                    })
                }
                Err(error) => Err(error),
            }
        })
    }

    fn resolve_cwd(&self, requested: &Path, fallback: &Path) -> Result<CwdResolution, SetupError> {
        let fallback = canonical_directory(fallback, None)
            .map_err(|_| SetupError::Cwd(CwdFailure::InvalidFallback))?;
        match canonical_directory(requested, Some(&fallback)) {
            Ok(path) => Ok(CwdResolution::Requested(path)),
            Err(CwdFailure::InvalidFallback) => Ok(CwdResolution::DeletedFallback {
                original: requested.to_path_buf(),
                fallback,
            }),
            Err(error) => Err(SetupError::Cwd(error)),
        }
    }

    fn apply_scope(
        &self,
        runtime: lotta_domain::RuntimeScope,
        cwd: &Path,
        mode: PermissionMode,
        cancellation: &CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, SetupScopeHandle> {
        let result = (|| {
            if cancellation.is_cancelled() {
                return Err(RuntimeError::Cancelled {
                    context: "setup scope".into(),
                });
            }
            let canonical = std::fs::canonicalize(cwd).map_err(runtime_adapter)?;
            if !canonical.starts_with(self.workspace_policy.root()) {
                return Err(RuntimeError::PermissionDenied {
                    context: "workspace cwd".into(),
                });
            }
            let loaded = load_permissions(&self.permission_sources, &canonical).map_err(|_| {
                RuntimeError::PermissionDenied {
                    context: "permission settings".into(),
                }
            })?;
            let permissions =
                PermissionPolicy::new(&canonical, runtime.clone(), Some(mode), loaded).map_err(
                    |_| RuntimeError::PermissionDenied {
                        context: "permission policy".into(),
                    },
                )?;
            let id = self.next_scope_snapshot.fetch_add(1, Ordering::Relaxed);
            self.scope_snapshots
                .lock()
                .map_err(|_| RuntimeError::AdapterFailure {
                    code: "scope_snapshot_lock",
                    context: "scope snapshot".into(),
                })?
                .insert(
                    id,
                    ProductionScope {
                        runtime,
                        cwd: canonical,
                        permissions,
                    },
                );
            Ok(SetupScopeHandle::new(id))
        })();
        Box::pin(async move { result })
    }

    fn prepare_memfs(
        &self,
        agent: &Agent,
        cancellation: &CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let memfs = self.memfs.clone();
        let id = agent.id.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(RuntimeError::Cancelled {
                    context: "memfs setup".into(),
                });
            }
            match memfs.status(&id).await {
                Ok(_) => Ok(()),
                Err(RuntimeError::NotFound { .. }) => memfs
                    .initialize(
                        &id,
                        &InitialMemoryBlocks::new(Vec::new()).map_err(runtime_adapter)?,
                    )
                    .await
                    .map(|_| ()),
                Err(error) => Err(error),
            }
        })
    }

    fn skill_inventory(
        &self,
        _agent: &Agent,
        cwd: &Path,
        selected: &[String],
    ) -> Result<SkillInventory, SetupError> {
        let catalog = self.discover_skills(cwd)?;
        let exact = SkillDiscovery::select(&catalog, selected).map_err(debug_adapter)?;
        Ok(SkillInventory {
            available: catalog.into_iter().map(|skill| skill.id).collect(),
            selected: exact.into_iter().map(|skill| skill.id).collect(),
        })
    }

    fn compile_prompt(
        &self,
        agent: &Agent,
        conversation: &Conversation,
        inventory: &SkillInventory,
        scope: SetupScopeHandle,
        reminder: Option<&str>,
        cancellation: &CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, String> {
        let cwd = self
            .scope_snapshots
            .lock()
            .ok()
            .and_then(|snapshots| snapshots.get(&scope.id()).map(|value| value.cwd.clone()));
        let catalog = cwd.as_deref().map_or_else(
            || Err(SetupError::Adapter("scope snapshot unavailable".into())),
            |cwd| self.discover_skills(cwd),
        );
        let selected = catalog.and_then(|catalog| {
            SkillDiscovery::select(&catalog, &inventory.selected).map_err(debug_adapter)
        });
        let cache = self.prompt_cache(&agent.id, &conversation.id);
        let memfs = self.memfs.clone();
        let agent = agent.clone();
        let conversation = conversation.clone();
        let reminder = reminder.map(str::to_owned);
        let cancellation = cancellation.clone();
        Box::pin(async move {
            let skills = selected
                .map_err(runtime_adapter)?
                .into_iter()
                .map(|skill| PromptSkill::new(skill.id, skill.description, Some(skill.location)))
                .collect::<Result<Vec<_>, _>>()?;
            let sections = PromptSections {
                runtime_reminders: reminder
                    .map(PromptText::new)
                    .transpose()?
                    .into_iter()
                    .collect(),
                skills,
                ..PromptSections::default()
            };
            let inputs = PromptInputs::new(
                PromptText::new(agent.system)?,
                agent.id,
                conversation.id,
                0,
                Timestamp::from_utc(chrono::Utc::now()),
                sections,
            )?;
            cache?
                .get_or_compile(
                    &PromptCompiler::new(&memfs),
                    &inputs,
                    DeliveryCapability::RequestBoundaryOnly,
                    cancellation.clone(),
                )
                .await
                .map(|delivery| delivery.delivery.content)
        })
    }

    fn resolve_model(
        &self,
        agent: &Agent,
        conversation: &Conversation,
    ) -> Result<ResolvedTurnModel, SetupError> {
        let agent_handle = ModelHandle::from_str(agent.model.as_str()).map_err(adapter)?;
        let conversation_handle = conversation
            .model
            .as_ref()
            .and_then(|value| value.as_ref())
            .map(|value| ModelHandle::from_str(value))
            .transpose()
            .map_err(adapter)?;
        let resolved = resolve_model(
            &ModelOverride::default(),
            &ModelOverride {
                handle: conversation_handle,
                settings: None,
            },
            &ModelOverride {
                handle: Some(agent_handle),
                settings: None,
            },
            &ModelOverride {
                handle: Some(self.default_model.clone()),
                settings: None,
            },
        )
        .map_err(adapter)?;
        let descriptor = self
            .models
            .iter()
            .find(|model| model.handle.as_str() == resolved.handle.to_string())
            .cloned()
            .ok_or_else(|| SetupError::Availability(resolved.handle.to_string()))?;
        let connection = self
            .connections
            .iter()
            .find(|connection| connection.id == descriptor.provider_id.as_str());
        if !descriptor.available || !connection.is_some_and(|value| value.is_connected) {
            return Err(SetupError::Availability(resolved.handle.to_string()));
        }
        let context_window = effective_window(ContextWindowSources {
            server_max: Some(self.server_context_window),
            model_catalog: descriptor.context_window,
            agent: None,
            conversation: conversation.context_window_limit,
        })
        .map_err(adapter)?;
        let fallback_candidates =
            fallback_candidates(agent, &descriptor, &self.models, &self.connections);
        Ok(ResolvedTurnModel {
            model: descriptor,
            context_window,
            output_tokens: self.output_tokens,
            toolset: configured_toolset(agent)
                .unwrap_or(self.toolset)
                .to_string(),
            allowlist: configured_allowlist(agent).unwrap_or_else(|| self.allowlist.clone()),
            fallback_candidates,
        })
    }

    fn discover_selected(
        &self,
        inventory: &SkillInventory,
        selected: &[String],
    ) -> Result<Vec<String>, SetupError> {
        let unique: BTreeSet<_> = selected.iter().collect();
        if unique.len() != selected.len()
            || selected.iter().any(|id| !inventory.available.contains(id))
        {
            return Err(SetupError::SkillSelection);
        }
        let mut values = selected.to_vec();
        values.sort();
        Ok(values)
    }

    fn load_extensions(
        &self,
        _agent: &Agent,
        _cwd: &Path,
        cancellation: &CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ExtensionSnapshot> {
        let result = if cancellation.is_cancelled() {
            Err(RuntimeError::Cancelled {
                context: "extensions setup".into(),
            })
        } else {
            self.capture_extension_snapshot().map_err(runtime_adapter)
        };
        Box::pin(async move { result })
    }

    fn tool_candidates(
        &self,
        scope_handle: SetupScopeHandle,
        extensions: &ExtensionSnapshot,
        cancellation: &CancellationToken,
    ) -> Result<Vec<ToolCandidate>, SetupError> {
        if cancellation.is_cancelled() {
            return Err(SetupError::Cancelled);
        }
        let snapshots = self
            .scope_snapshots
            .lock()
            .map_err(|_| SetupError::Adapter("scope snapshot lock".into()))?;
        let scope = snapshots
            .get(&scope_handle.id())
            .ok_or_else(|| SetupError::Adapter("scope snapshot unavailable".into()))?;
        if extensions.id() != 0 {
            let snapshot = self.extension_snapshot(extensions)?;
            let _ = snapshot.mods.registrations().len();
            let _ = Arc::strong_count(&snapshot.hooks);
            let _ = Arc::strong_count(&self.hook_runtime);
        }
        self.registry
            .snapshot()
            .map_err(adapter)?
            .complete_registrations()
            .1
            .into_iter()
            .map(|registration| {
                authorize_candidate(
                    scope,
                    ToolCandidate {
                        source: if registration.definition.execution_owner
                            == lotta_runtime::ports::ToolExecutionOwner::ModSidecar
                        {
                            SetupToolSource::Mod
                        } else {
                            SetupToolSource::BuiltIn
                        },
                        definition: (*registration.definition).clone(),
                        model_name: registration.definition.model_name.as_str().to_owned(),
                        authorized: true,
                    },
                )
            })
            .collect()
    }

    fn merge_tools(
        &self,
        model: &ResolvedTurnModel,
        candidates: Vec<ToolCandidate>,
    ) -> Result<TurnToolCatalog, SetupError> {
        let names: BTreeSet<_> = candidates
            .into_iter()
            .filter(|candidate| candidate.authorized)
            .map(|candidate| candidate.definition.internal_name.as_str().to_owned())
            .collect();
        let toolset = ToolsetId::from_str(&model.toolset).map_err(adapter)?;
        let external = self
            .registry
            .snapshot()
            .map_err(adapter)?
            .complete_registrations()
            .1
            .into_iter()
            .filter(|registration| {
                registration.definition.execution_owner
                    == lotta_runtime::ports::ToolExecutionOwner::ModSidecar
                    && names.contains(registration.definition.internal_name.as_str())
            })
            .collect::<Vec<ToolRegistration>>();
        let allowlist = model
            .allowlist
            .as_ref()
            .map(|values| values.iter().map(String::as_str).collect::<Vec<_>>());
        let snapshot = self
            .registry
            .compose(toolset, &external, allowlist.as_deref())
            .map_err(adapter)?;
        let handle_id = self.next_tool_snapshot.fetch_add(1, Ordering::Relaxed);
        self.tool_snapshots
            .lock()
            .map_err(|_| SetupError::Adapter("tool snapshot lock".into()))?
            .insert(handle_id, Arc::clone(&snapshot));
        TurnToolCatalog::new(
            snapshot
                .complete_registrations()
                .1
                .into_iter()
                .map(|registration| {
                    let mut definition = (*registration.definition).clone();
                    definition.model_name = ModelFacingToolName::new(
                        names::model_name(toolset, definition.internal_name.as_str())
                            .unwrap_or(definition.model_name.as_str())
                            .to_owned(),
                    )
                    .map_err(SetupError::from)?;
                    Ok(definition)
                })
                .collect::<Result<Vec<_>, SetupError>>()?,
        )
        .map(|catalog| catalog.with_snapshot(lotta_runtime::ToolSnapshotHandle::new(handle_id)))
        .map_err(SetupError::from)
    }

    fn build_request(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        prompt: String,
        input: &str,
        input_id: Option<&str>,
        model: &ResolvedTurnModel,
        catalog: &TurnToolCatalog,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ProviderRequest> {
        let store = self.store.clone();
        let agent = agent.clone();
        let conversation = conversation.clone();
        let input = input.to_owned();
        let input_id = input_id.map(str::to_owned);
        let model = model.clone();
        let tools = catalog
            .definitions()
            .map(provider_tool)
            .collect::<Result<Vec<_>, _>>();
        let server_context_window = self.server_context_window;
        Box::pin(async move {
            let request = build_provider_request(BuildProviderRequest {
                store,
                agent,
                conversation,
                prompt,
                input,
                input_id,
                model,
                tools,
                server_context_window,
                cancellation,
            })
            .await;
            request.map_err(setup_error_runtime)
        })
    }

    fn admit_input(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        input: &str,
        reminder_claim: Option<&ReminderClaim>,
    ) -> lotta_runtime::ports::PortFuture<'_, AdmissionReceipt> {
        let store = self.store.clone();
        let agent = agent.clone();
        let conversation = conversation.clone();
        let input = input.to_owned();
        let predetermined_id = reminder_claim.map(|claim| claim.input_id().to_owned());
        Box::pin(async move {
            let id = predetermined_id.unwrap_or_else(unique_id);
            let now = Timestamp::from_utc(chrono::Utc::now());
            let entry = TranscriptEntry::Message(MessageEntry {
                entry_type: MessageEntryType::Message,
                id: NonEmptyString::new(id.clone()).map_err(runtime_adapter)?,
                parent_id: None,
                timestamp: now,
                message: LocalMessage {
                    id: MessageId::accept(&id).map_err(runtime_adapter)?,
                    role: LocalMessageRole::User,
                    content: Some(
                        BoundedJsonValue::new(serde_json::json!(input)).map_err(runtime_adapter)?,
                    ),
                    timestamp: chrono::Utc::now().timestamp_millis() as f64,
                    metadata: None,
                    extras: Default::default(),
                },
            });
            append_or_initialize(&store, &agent, &conversation, &entry, now).await?;
            Ok(AdmissionReceipt {
                input_id: id,
                appended: true,
            })
        })
    }

    fn commit_cwd_reminder(
        &self,
        claim: &ReminderClaim,
        receipt: &AdmissionReceipt,
        cancellation: &CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let path = self.reminders_path.clone();
        let root = self.store.paths().root().to_path_buf();
        let claim = claim.token().to_owned();
        let input_id = receipt.input_id.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            commit_claim(&path, &root, &claim, &input_id, &cancellation)
                .await
                .map_err(setup_error_runtime)
        })
    }

    fn claim_cwd_reminder(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        original: &Path,
        scope_handle: SetupScopeHandle,
        cancellation: &CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, Option<ReminderClaim>> {
        let key = reminder_key(agent, conversation, original);
        let now = (self.now_ms)();
        let fallback = self
            .scope_snapshots
            .lock()
            .map_err(|_| SetupError::Adapter("scope snapshot lock".into()))
            .and_then(|snapshots| {
                snapshots
                    .get(&scope_handle.id())
                    .map(|scope| scope.cwd.clone())
                    .ok_or_else(|| SetupError::Adapter("scope snapshot unavailable".into()))
            });
        let original = original.to_path_buf();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            let result: Result<Option<ReminderClaim>, SetupError> = async {
                let key = key?;
                let fallback = fallback?;
                self.update_reminders(&cancellation, |state| {
                    if !claimable(state.entries.get(&key), now) {
                        return Ok(None);
                    }
                    let token = unique_id();
                    let input_id = unique_id();
                    let revision = state.revision.saturating_add(1);
                    state.revision = revision;
                    state.entries.insert(
                        key,
                        ReminderEntry::Pending {
                            token: token.clone(),
                            owner_turn: input_id.clone(),
                            input_id: input_id.clone(),
                            original_path: original.clone(),
                            fallback_path: fallback.clone(),
                            created_at_epoch_ms: now,
                            revision,
                        },
                    );
                    let message = format!(
                        "Your previous working directory '{}' was deleted. \
                     Continuing in fallback directory '{}'.",
                        original.display(),
                        fallback.display()
                    );
                    Ok(Some(ReminderClaim::new(token, input_id, message)))
                })
                .await
            }
            .await;
            result.map_err(setup_error_runtime)
        })
    }

    fn release_cwd_reminder(
        &self,
        claim: &ReminderClaim,
        cancellation: &CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let claim = claim.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            self.update_reminders(&cancellation, |state| {
                let key = pending_key_for_token(state, claim.token())?;
                state.entries.remove(&key);
                Ok(())
            })
            .await
            .map_err(setup_error_runtime)
        })
    }

    fn record_interrupted(
        &self,
        receipt: &AdmissionReceipt,
        failure: &SetupError,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let path = self
            .reminders_path
            .with_file_name(format!("interrupted-{}.json", receipt.input_id));
        let bytes = serde_json::to_vec(
            &serde_json::json!({"input_id": receipt.input_id, "error": failure.to_string()}),
        )
        .map_err(runtime_adapter);
        Box::pin(async move {
            atomic_write(&path, &bytes?, WriteMode::Standard).map_err(runtime_adapter)
        })
    }

    fn release_scope(&self, scope: SetupScopeHandle) {
        if let Ok(mut snapshots) = self.scope_snapshots.lock() {
            snapshots.remove(&scope.id());
        }
    }
}

async fn append_or_initialize(
    store: &LocalStore,
    agent: &AgentId,
    conversation: &ConversationId,
    entry: &TranscriptEntry,
    now: Timestamp,
) -> Result<(), RuntimeError> {
    match store
        .append_transcript_entry(agent, conversation, entry)
        .await
    {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == StoreErrorKind::NotFound => {
            let manifest = TranscriptManifest {
                schema_version: TRANSCRIPT_MANIFEST_SCHEMA_VERSION,
                message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
                provider_stack: ProviderStack::PiAi,
                created_at: now,
                migrated_from: None,
                migrated_at: None,
                backup_path: None,
            };
            let session_id = format!(
                "session-{}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            );
            let session = TranscriptEntry::Session(SessionEntry {
                entry_type: SessionEntryType::Session,
                version: TRANSCRIPT_SESSION_SCHEMA_VERSION,
                id: NonEmptyString::new(session_id).map_err(runtime_adapter)?,
                timestamp: now,
                cwd: "/".to_owned(),
            });
            match store
                .initialize_transcript(agent, conversation, &manifest, &session)
                .await
            {
                Ok(()) => {}
                Err(error) if error.kind() == StoreErrorKind::StorageConflict => {}
                Err(error) => return Err(runtime_adapter(error)),
            }
            store
                .append_transcript_entry(agent, conversation, entry)
                .await
                .map_err(runtime_adapter)
        }
        Err(error) => Err(runtime_adapter(error)),
    }
}

#[derive(Clone)]
struct CanonicalExtensionSnapshot {
    mods: ModRuntimeSnapshot,
    hooks: Arc<HookRegistrySnapshot>,
}

struct ProductionSnapshotHookRuntime<'a> {
    setup: &'a ProductionSetupPorts,
}

impl HookRuntime for ProductionSnapshotHookRuntime<'_> {
    fn fire(
        &self,
        payload: lotta_runtime::hooks::HookPayload,
        cancellation: CancellationToken,
    ) -> lotta_runtime::hooks::HookFuture<'_> {
        self.setup.hook_runtime.fire(payload, cancellation)
    }

    fn fire_snapshot(
        &self,
        snapshot_id: u64,
        payload: lotta_runtime::hooks::HookPayload,
        cancellation: CancellationToken,
    ) -> lotta_runtime::hooks::HookFuture<'_> {
        Box::pin(async move {
            let snapshot = self
                .setup
                .extension_snapshot(&ExtensionSnapshot::new(snapshot_id))
                .map_err(|_| hook_failure("runtime", "registry", "hook_registry"))?;
            let _ = Arc::strong_count(&snapshot.hooks);
            self.setup.hook_runtime.fire(payload, cancellation).await
        })
    }
}

fn authorize_candidate(
    scope: &ProductionScope,
    mut candidate: ToolCandidate,
) -> Result<ToolCandidate, SetupError> {
    let empty = lotta_runtime::ports::ValidatedToolInput::new(
        BoundedJsonValue::new(serde_json::json!({})).map_err(adapter)?,
    )
    .map_err(SetupError::from)?;
    let invocation = PermissionInvocation::from_definition(&candidate.definition, &empty);
    candidate.authorized = matches!(
        scope.permissions.check(invocation, &[], &[]),
        Ok(PermissionDecision::Allow)
    );
    Ok(candidate)
}

fn canonical_directory(path: &Path, confined: Option<&Path>) -> Result<PathBuf, CwdFailure> {
    let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => CwdFailure::InvalidFallback,
        std::io::ErrorKind::PermissionDenied => CwdFailure::PermissionDenied,
        _ => CwdFailure::InvalidFallback,
    })?;
    if !metadata.is_dir() {
        return Err(CwdFailure::NotDirectory);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| CwdFailure::PermissionDenied)?;
    if confined.is_some_and(|root| !canonical.starts_with(root)) {
        return Err(CwdFailure::SymlinkOrEscape);
    }
    Ok(canonical)
}

fn fallback_candidates(
    agent: &Agent,
    primary: &ModelDescriptor,
    models: &[ModelDescriptor],
    connections: &[lotta_providers::connections::ConnectionSnapshot],
) -> Vec<ModelDescriptor> {
    let configured = agent
        .model_settings
        .get("fallbackModels")
        .or_else(|| agent.extras.get("fallback_models"))
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        });
    let mut candidates = models
        .iter()
        .filter(|model| {
            model.available
                && model.handle != primary.handle
                && connections.iter().any(|connection| {
                    connection.id == model.provider_id.as_str() && connection.is_connected
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.handle.as_str().cmp(right.handle.as_str()));
    if let Some(order) = configured {
        candidates.sort_by_key(|candidate| {
            order
                .iter()
                .position(|handle| handle == candidate.handle.as_str())
                .unwrap_or(usize::MAX)
        });
        candidates.retain(|candidate| {
            order
                .iter()
                .any(|handle| handle == candidate.handle.as_str())
        });
    }
    candidates
}

fn configured_toolset(agent: &Agent) -> Option<ToolsetId> {
    agent
        .model_settings
        .get("toolset")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| ToolsetId::from_str(value).ok())
}

fn configured_allowlist(agent: &Agent) -> Option<Option<Vec<String>>> {
    let value = agent.model_settings.get("toolAllowlist")?;
    if value.is_null() {
        return Some(None);
    }
    value.as_array().map(|values| {
        Some(
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect(),
        )
    })
}

async fn load_transcript_messages(
    store: &LocalStore,
    agent: &AgentId,
    conversation: &ConversationId,
) -> Result<Vec<LocalMessage>, lotta_store::StoreError> {
    store
        .load_transcript(agent, conversation)
        .await
        .map(|loaded| loaded.messages().to_vec())
}

fn provider_text_message(
    role: ProviderMessageRole,
    text: &str,
) -> Result<ProviderMessage, SetupError> {
    let content = ProviderContent::new(vec![ProviderContentPart::Text(
        ProviderText::new(text.to_owned()).map_err(adapter)?,
    )])
    .map_err(adapter)?;
    Ok(ProviderMessage {
        role,
        content,
        tool_call_id: None,
    })
}

fn local_provider_message(message: &LocalMessage) -> Result<ProviderMessage, SetupError> {
    let role = match message.role {
        LocalMessageRole::User => ProviderMessageRole::User,
        LocalMessageRole::Assistant => ProviderMessageRole::Assistant,
        LocalMessageRole::ToolResult => ProviderMessageRole::Tool,
    };
    let content = message
        .content
        .as_ref()
        .map(BoundedJsonValue::as_value)
        .and_then(|value| value.as_str().map(str::to_owned))
        .or_else(|| {
            message
                .content
                .as_ref()
                .map(BoundedJsonValue::as_value)
                .and_then(|value| value.get("text"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| SetupError::Adapter("unsupported transcript message content".into()))?;
    provider_text_message(role, &content)
}

fn provider_tool(
    definition: &lotta_runtime::ports::ToolDefinition,
) -> Result<ProviderToolDefinition, SetupError> {
    Ok(ProviderToolDefinition {
        name: ProviderName::new(definition.model_name.as_str().to_owned())
            .map_err(SetupError::from)?,
        description: ProviderText::new(definition.description.as_str().to_owned())
            .map_err(SetupError::from)?,
        input_schema: BoundedJsonValue::new(definition.input_schema.as_value().clone())
            .map_err(runtime_adapter)?,
    })
}

async fn commit_claim(
    path: &Path,
    root: &Path,
    claim: &str,
    input_id: &str,
    cancellation: &CancellationToken,
) -> Result<(), SetupError> {
    let _lock = acquire_reminder_lock(root, cancellation).await?;
    let mut state = read_reminders(path)?;
    let key = pending_key_for_token(&state, claim)?;
    let entry = state.entries.get(&key);
    let matches_input = matches!(
        entry,
        Some(ReminderEntry::Pending {
            input_id: expected,
            ..
        }) if expected == input_id
    );
    if !matches_input {
        return Err(SetupError::Adapter("invalid reminder input".into()));
    }
    let revision = state.revision.saturating_add(1);
    state.revision = revision;
    state.entries.insert(
        key,
        ReminderEntry::Consumed {
            input_id: input_id.to_owned(),
            revision,
        },
    );
    save_reminders(path, &state)
}

async fn acquire_reminder_lock(
    root: &Path,
    cancellation: &CancellationToken,
) -> Result<LottaStorageLock, SetupError> {
    let delay = std::time::Duration::from_millis(REMINDER_LOCK_RETRY_MS);
    for attempt in 0..=REMINDER_LOCK_RETRY_MAX {
        match LottaStorageLock::try_acquire(root) {
            Ok(lock) => return Ok(lock),
            Err(error)
                if error.kind() == StoreErrorKind::LottaLock
                    && attempt < REMINDER_LOCK_RETRY_MAX =>
            {
                tokio::select! {
                    () = cancellation.cancelled() => return Err(SetupError::Cancelled),
                    () = tokio::time::sleep(delay) => {}
                }
            }
            Err(error) => return Err(adapter(error)),
        }
    }
    Err(SetupError::Deadline)
}

fn read_reminders(path: &Path) -> Result<ReminderState, SetupError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(SetupError::Adapter("invalid reminder file".into()))
        }
        Ok(metadata) if metadata.len() > REMINDER_FILE_BYTES_MAX => {
            Err(SetupError::Adapter("reminder file limit".into()))
        }
        Ok(_) => std::fs::read(path)
            .map_err(adapter)
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(adapter)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ReminderState::default()),
        Err(error) => Err(adapter(error)),
    }
}

fn save_reminders(path: &Path, state: &ReminderState) -> Result<(), SetupError> {
    if state.entries.len() > REMINDERS_MAX {
        return Err(SetupError::Adapter("reminder limit".into()));
    }
    let bytes = serde_json::to_vec(state).map_err(adapter)?;
    if bytes.len() as u64 > REMINDER_FILE_BYTES_MAX {
        return Err(SetupError::Adapter("reminder file limit".into()));
    }
    atomic_write(path, &bytes, WriteMode::Standard).map_err(adapter)
}

fn reminder_key(
    agent: &AgentId,
    conversation: &ConversationId,
    original: &Path,
) -> Result<String, SetupError> {
    if original.as_os_str().len() > CWD_REMINDER_PATH_BYTES_MAX {
        return Err(SetupError::Adapter("reminder path limit".into()));
    }
    Ok(format!(
        "{}:{}:{}",
        agent.as_str(),
        conversation.as_str(),
        original.display()
    ))
}

fn claimable(entry: Option<&ReminderEntry>, now: u64) -> bool {
    match entry {
        None => true,
        Some(ReminderEntry::Consumed { .. }) => false,
        Some(ReminderEntry::Pending {
            created_at_epoch_ms,
            ..
        }) => now.saturating_sub(*created_at_epoch_ms) >= REMINDER_PENDING_TTL_MS,
    }
}

fn pending_key_for_token(state: &ReminderState, token: &str) -> Result<String, SetupError> {
    if token.is_empty() || token.len() > REMINDER_TOKEN_BYTES_MAX {
        return Err(SetupError::Adapter("invalid reminder claim".into()));
    }
    state
        .entries
        .iter()
        .find_map(|(key, entry)| match entry {
            ReminderEntry::Pending { token: current, .. } if current == token => Some(key.clone()),
            _ => None,
        })
        .ok_or_else(|| SetupError::Adapter("invalid reminder claim".into()))
}

fn recover_consumed(store: &LocalStore, state: &mut ReminderState) -> Result<(), SetupError> {
    let pending: Vec<_> = state
        .entries
        .iter()
        .filter_map(|(key, entry)| match entry {
            ReminderEntry::Pending {
                input_id, revision, ..
            } => Some((key.clone(), input_id.clone(), *revision)),
            ReminderEntry::Consumed { .. } => None,
        })
        .collect();
    for (key, input_id, revision) in pending {
        if transcript_contains_input(store, &key, &input_id)? {
            state
                .entries
                .insert(key, ReminderEntry::Consumed { input_id, revision });
        }
    }
    Ok(())
}

fn transcript_contains_input(
    store: &LocalStore,
    key: &str,
    input_id: &str,
) -> Result<bool, SetupError> {
    let (agent, rest) = key
        .split_once(':')
        .ok_or_else(|| SetupError::Adapter("invalid reminder key".into()))?;
    let (conversation, _) = rest
        .split_once(':')
        .ok_or_else(|| SetupError::Adapter("invalid reminder key".into()))?;
    let path = store
        .paths()
        .conversation_dir(
            &AgentId::accept(agent).map_err(adapter)?,
            &ConversationId::accept(conversation).map_err(adapter)?,
        )
        .map_err(adapter)?
        .join("messages.jsonl");
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes
            .windows(input_id.len())
            .any(|window| window == input_id.as_bytes())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(adapter(error)),
    }
}

fn unique_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    Uuid::from_u128(nanos ^ u128::from(std::process::id())).to_string()
}

fn system_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn adapter(error: impl std::fmt::Display) -> SetupError {
    SetupError::Adapter(error.to_string())
}
fn debug_adapter(error: impl std::fmt::Debug) -> SetupError {
    SetupError::Adapter(format!("{error:?}"))
}
fn runtime_adapter(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "production_setup",
        context: error.to_string(),
    }
}

fn hook_failure(owner: &str, hook_id: &str, code: &'static str) -> HookFailure {
    HookFailure {
        owner: lotta_runtime::hooks::HookOwner::new(owner.to_owned()).expect("constant hook owner"),
        hook_id: lotta_runtime::hooks::HookId::new(hook_id.to_owned()).expect("constant hook id"),
        code,
    }
}

fn lifecycle_setup_error<E>(error: LifecycleError<E>) -> SetupError {
    match error {
        LifecycleError::Blocked => SetupError::HookBlocked,
        LifecycleError::Hook(failure) => SetupError::HookFailure {
            owner: failure.owner.as_str().to_owned(),
            hook_id: failure.hook_id.as_str().to_owned(),
            code: failure.code,
        },
        LifecycleError::Operation(_) => unreachable!("operation errors are handled separately"),
    }
}

fn setup_error_runtime(error: SetupError) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "production_setup",
        context: error.to_string(),
    }
}

fn setup_failure_runtime(error: SetupFailure) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: match error {
            SetupFailure::PreAdmission(_) => "turn_setup_pre_admission",
            SetupFailure::PostAdmission { .. } => "turn_setup_post_admission",
        },
        context: format!("{error:?}"),
    }
}

struct ProductionDispatchStatus<'a> {
    sink: &'a dyn SetupStatusSink,
}

impl lotta_runtime::turn::ProviderStartPort for ProductionDispatchStatus<'_> {
    fn provider_start(&self) -> lotta_runtime::ports::PortFuture<'_, ()> {
        Box::pin(async move {
            self.sink
                .emit(SetupStatus::Sending)
                .map_err(runtime_adapter)
        })
    }

    fn provider_waiting(&self) -> lotta_runtime::ports::PortFuture<'_, ()> {
        Box::pin(async move {
            self.sink
                .emit(SetupStatus::Waiting)
                .map_err(runtime_adapter)
        })
    }
}

type BrokerFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, RuntimeError>> + Send + 'a>>;

/// Service that owns transcript compaction mechanics outside Task55.
pub trait ProductionCompactionService: Send + Sync {
    /// Updates durable transcript state for one exact scoped request.
    fn compact(
        &self,
        command: CompactionCommand,
    ) -> BrokerFuture<'_, lotta_runtime::turn::CompactionProgress>;
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ControllerKey {
    scope: lotta_domain::RuntimeScope,
    run_generation: u64,
    call_id: String,
}

/// Shared bounded production broker registry owned by the production component graph.
pub struct ProductionTurnBrokers {
    controller_tools: Mutex<
        HashMap<ControllerKey, tokio::sync::oneshot::Sender<lotta_runtime::ports::ToolOutcome>>,
    >,
    pub(crate) compaction: Mutex<Option<Arc<dyn ProductionCompactionService>>>,
}

impl ProductionTurnBrokers {
    /// Creates empty bounded broker registries.
    #[must_use]
    pub fn new() -> Self {
        Self {
            controller_tools: Mutex::new(HashMap::new()),
            compaction: Mutex::new(None),
        }
    }

    /// Resolves one exact current controller-owned call once.
    pub fn resolve_controller_tool(
        &self,
        scope: &lotta_domain::RuntimeScope,
        lease_generation: u64,
        call_id: &str,
        outcome: lotta_runtime::ports::ToolOutcome,
    ) -> Result<bool, RuntimeError> {
        let key = ControllerKey {
            scope: scope.clone(),
            run_generation: lease_generation,
            call_id: call_id.to_owned(),
        };
        let sender = self
            .controller_tools
            .lock()
            .map_err(|_| broker_error("controller tool lock"))?
            .remove(&key);
        Ok(sender.is_some_and(|sender| sender.send(outcome).is_ok()))
    }

    /// Registers or clears the production compaction service.
    pub fn register_compaction_service(
        &self,
        service: Option<Arc<dyn ProductionCompactionService>>,
    ) {
        if let Ok(mut current) = self.compaction.lock() {
            *current = service;
        }
    }
}

impl Default for ProductionTurnBrokers {
    fn default() -> Self {
        Self::new()
    }
}

fn broker_error(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "production_turn_broker",
        context: context.into(),
    }
}

pub(crate) struct ProductionEditedInputValidator;

impl lotta_runtime::EditedInputValidator for ProductionEditedInputValidator {
    fn validate(
        &self,
        request: &lotta_runtime::ApprovalRequest,
        input: BoundedJsonValue,
    ) -> Result<BoundedJsonValue, RuntimeError> {
        use lotta_domain::BoundedVec;
        use lotta_runtime::ports::*;
        let definition = ToolDefinition::new(
            InternalToolName::new("approval_validation".to_owned())?,
            ModelFacingToolName::new("approval_validation".to_owned())?,
            request.original_schema.clone(),
            ToolDescriptionAsset::new("approval validation".to_owned())?,
            ToolExecutionOwner::Controller,
            ToolApprovalPolicy::Never,
            PermissionAction::new("approval_validation".to_owned())?,
            ToolTimeout::new(std::time::Duration::from_millis(1))?,
            ToolOutputLimit::new(1, 1)?,
            SecretRedactionSpec::new(
                BoundedVec::new(Vec::new()).map_err(|_| broker_error("approval secret spec"))?,
                SecretRedactionPolicy::Omit,
            )?,
        );
        lotta_tools::validate_input(input, &definition)
            .map(|value| BoundedJsonValue::new(value.as_value().clone()))
            .map_err(|_| broker_error("approval edited input schema"))?
            .map_err(|_| broker_error("approval edited input bound"))
    }
}

struct ApprovalBrokerAdapter {
    manager: Arc<lotta_runtime::ApprovalManager>,
    scope: lotta_domain::RuntimeScope,
    run_id: lotta_domain::RunId,
    turn_id: NonEmptyString,
    input_id: NonEmptyString,
    lease_generation: u64,
    clock: Arc<dyn lotta_domain::Clock + Send + Sync>,
}

impl lotta_runtime::turn::ApprovalPort for ApprovalBrokerAdapter {
    fn store_request(
        &self,
        request: lotta_runtime::turn::ControlRequest,
    ) -> Result<(), RuntimeError> {
        if request.lease_generation != self.lease_generation {
            return Err(broker_error("stale approval lease"));
        }
        let created_at = self.clock.now();
        let expires_at = created_at
            .checked_add(chrono::Duration::milliseconds(
                i64::try_from(lotta_runtime::APPROVAL_WAIT_MS_MAX)
                    .map_err(|_| broker_error("approval deadline"))?,
            ))
            .map_err(|_| broker_error("approval deadline"))?;
        self.manager.store_request(lotta_runtime::ApprovalRequest {
            request_id: request.request_id,
            tool_call_id: NonEmptyString::new(request.call_id.as_str().to_owned())
                .map_err(|_| broker_error("approval call id"))?,
            scope: self.scope.clone(),
            run_id: self.run_id.clone(),
            turn_id: self.turn_id.clone(),
            input_id: self.input_id.clone(),
            lease_generation: self.lease_generation,
            tool_name: request.tool_name,
            original_input: request.input,
            original_schema: request.schema,
            created_at,
            expires_at,
            state: lotta_runtime::ApprovalState::Pending,
            revision: 0,
        })?;
        Ok(())
    }

    fn await_resolution(
        &self,
        request: lotta_runtime::turn::ControlRequest,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, lotta_runtime::turn::ApprovalResolution> {
        let manager = Arc::clone(&self.manager);
        let scope = self.scope.clone();
        let lease_generation = self.lease_generation;
        Box::pin(async move {
            if request.lease_generation != lease_generation {
                return Err(broker_error("stale approval lease"));
            }
            let canonical = manager
                .get_request(&scope, &request.request_id)?
                .ok_or_else(|| broker_error("approval request missing"))?;
            let receiver = manager.register_waiter(&canonical)?;
            let timeout = std::time::Duration::from_millis(lotta_runtime::APPROVAL_WAIT_MS_MAX);
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    let latest = manager
                        .get_request(&scope, &request.request_id)?
                        .ok_or_else(|| broker_error("approval request missing"))?;
                    if latest.state == lotta_runtime::ApprovalState::Pending {
                        let _ = manager.interrupt_pending(&latest)?;
                    }
                    Err(RuntimeError::Cancelled { context: "approval wait".into() })
                },
                () = tokio::time::sleep(timeout) => {
                    let _ = manager.expire(&canonical)?;
                    Err(RuntimeError::Timeout { context: "approval expired".into() })
                },
                value = receiver => {
                    let outcome = value.map_err(|_| broker_error("approval waiter dropped"))?;
                    match outcome.resolution {
                        lotta_runtime::ApprovalResolution::Allow => Ok(
                            lotta_runtime::turn::ApprovalResolution::Allow(outcome.edited_input),
                        ),
                        lotta_runtime::ApprovalResolution::Deny => {
                            Ok(lotta_runtime::turn::ApprovalResolution::Deny)
                        }
                        lotta_runtime::ApprovalResolution::Abort => Err(
                            RuntimeError::Cancelled {
                                context: "approval abort".into(),
                            },
                        ),
                    }
                },
            }
        })
    }

    fn mark_allowed(
        &self,
        request: &lotta_runtime::turn::ControlRequest,
        _: &lotta_runtime::ports::ToolOutcome,
    ) -> Result<(), RuntimeError> {
        let canonical = self
            .manager
            .get_request(&self.scope, &request.request_id)?
            .ok_or_else(|| broker_error("approval request missing"))?;
        self.manager
            .mark_allowed(&canonical)?
            .then_some(())
            .ok_or_else(|| broker_error("approval finalization conflict"))
    }

    fn mark_interrupted(
        &self,
        request: &lotta_runtime::turn::ControlRequest,
    ) -> Result<(), RuntimeError> {
        let canonical = self
            .manager
            .get_request(&self.scope, &request.request_id)?
            .ok_or_else(|| broker_error("approval request missing"))?;
        self.manager
            .interrupt(&canonical)?
            .then_some(())
            .ok_or_else(|| broker_error("approval interruption conflict"))
    }
}

struct ControllerToolBrokerAdapter {
    brokers: Arc<ProductionTurnBrokers>,
    scope: lotta_domain::RuntimeScope,
    lease_generation: u64,
    sink: Arc<dyn lotta_app_server::ws::RuntimeEventSink>,
}

impl lotta_runtime::turn::ControllerToolPort for ControllerToolBrokerAdapter {
    fn execute_external(
        &self,
        record: lotta_runtime::turn::ControllerToolRequestRecord,
        request: lotta_runtime::ports::ToolExecutionRequest,
    ) -> lotta_runtime::ports::PortFuture<'_, lotta_runtime::ports::ToolOutcome> {
        let key = ControllerKey {
            scope: self.scope.clone(),
            run_generation: self.lease_generation,
            call_id: record.call_id.as_str().to_owned(),
        };
        let brokers = Arc::clone(&self.brokers);
        let sink = Arc::clone(&self.sink);
        Box::pin(execute_controller_request(brokers, sink, key, request))
    }
}

async fn execute_controller_request(
    brokers: Arc<ProductionTurnBrokers>,
    sink: Arc<dyn lotta_app_server::ws::RuntimeEventSink>,
    key: ControllerKey,
    request: lotta_runtime::ports::ToolExecutionRequest,
) -> Result<lotta_runtime::ports::ToolOutcome, RuntimeError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    {
        let mut waiters = brokers
            .controller_tools
            .lock()
            .map_err(|_| broker_error("controller tool lock"))?;
        if waiters.len() >= PRODUCTION_BROKER_WAITERS_MAX
            || waiters.insert(key.clone(), sender).is_some()
        {
            return Err(broker_error("controller tool waiter unavailable"));
        }
    }
    let payload = BoundedJsonValue::new(serde_json::json!({
        "tool_name": request.definition.model_name.as_str(),
        "input": request.input.as_value(),
        "owner": format!("{:?}", request.definition.execution_owner).to_lowercase(),
    }))
    .map_err(|_| broker_error("controller request payload"))?;
    let event = lotta_app_server::ws::RuntimeEvent::ControllerToolRequest {
        call_id: NonEmptyString::new(key.call_id.clone())
            .map_err(|_| broker_error("controller call id"))?,
        lease_generation: key.run_generation,
        request: payload,
    };
    if sink.emit(&key.scope, event).is_err() {
        brokers
            .controller_tools
            .lock()
            .map_err(|_| broker_error("controller tool lock"))?
            .remove(&key);
        return Err(broker_error("controller request emit"));
    }
    let timeout = request.deadline.get().min(std::time::Duration::from_millis(
        u64::try_from(lotta_runtime::bounds::EXTERNAL_TOOL_CALL_TIMEOUT_MS)
            .map_err(|_| broker_error("external tool timeout"))?,
    ));
    let result = tokio::select! {
        biased;
        () = request.cancellation.cancelled() => Err(RuntimeError::Cancelled {
            context: "controller tool wait".into(),
        }),
        () = tokio::time::sleep(timeout) => Err(RuntimeError::Timeout {
            context: "controller tool wait".into(),
        }),
        value = receiver => value.map_err(|_| broker_error("controller tool waiter dropped")),
    };
    if let Ok(mut waiters) = brokers.controller_tools.lock() {
        waiters.remove(&key);
    }
    result
}

struct CompactionBrokerAdapter {
    brokers: Arc<ProductionTurnBrokers>,
    scope: lotta_domain::RuntimeScope,
    cancellation: CancellationToken,
    sink: Arc<dyn lotta_app_server::ws::RuntimeEventSink>,
}

impl lotta_runtime::turn::CompactionPort for CompactionBrokerAdapter {
    fn compact(
        &self,
        request: ProviderRequest,
        lease: TurnLease,
        detail: lotta_runtime::ports::ProviderContextOverflowDetail,
        trigger: CompactionTrigger,
    ) -> lotta_runtime::ports::PortFuture<'_, lotta_runtime::turn::CompactionProgress> {
        let service = self
            .brokers
            .compaction
            .lock()
            .ok()
            .and_then(|service| service.clone());
        let scope = self.scope.clone();
        let cancellation = self.cancellation.clone();
        let sink = Arc::clone(&self.sink);
        Box::pin(async move {
            // Task58 owns mechanics; Task55 only invokes an explicitly registered service.
            let service = service.ok_or(RuntimeError::CompactionUnavailable)?;
            let reason = match trigger {
                CompactionTrigger::Manual => "manual",
                CompactionTrigger::Pressure => "pressure",
                CompactionTrigger::ProviderOverflow => "provider_overflow",
            };
            let compaction_request = lotta_runtime::turn::CompactionRequest {
                scope: scope.clone(),
                lease_generation: lease.generation(),
                reason: NonEmptyString::new(reason.to_owned())
                    .map_err(|_| broker_error("compaction reason"))?,
                tokens_before: detail.estimated.tokens,
                messages_before: request.messages.as_slice().len(),
            };
            sink.emit(
                &scope,
                lotta_app_server::ws::RuntimeEvent::CompactionRequest {
                    lease_generation: compaction_request.lease_generation,
                    tokens_before: compaction_request.tokens_before,
                    messages_before: compaction_request.messages_before,
                    reason: compaction_request.reason,
                },
            )
            .map_err(|_| broker_error("compaction request emit"))?;
            service
                .compact(CompactionCommand {
                    request_id: NonEmptyString::new(format!(
                        "context-{}-{}",
                        lease.generation(),
                        detail.attempt
                    ))
                    .map_err(|_| broker_error("compaction request id"))?,
                    scope,
                    lease,
                    request,
                    detail,
                    trigger,
                    mode: CompactionMode::default(),
                    cancellation,
                })
                .await
        })
    }
}

pub(crate) struct ProductionProviderSummarizer {
    pub(crate) provider: Arc<crate::production_components::ProductionProviderPort>,
}

impl CompactionSummarizer for ProductionProviderSummarizer {
    fn summarize(
        &self,
        mut request: ProviderRequest,
        messages: Vec<ProviderMessage>,
        cancellation: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<CompactionSummary, RuntimeError>> + Send + '_>> {
        let provider = Arc::clone(&self.provider);
        Box::pin(async move {
            request.messages = ProviderMessages::new(messages)
                .map_err(|_| broker_error("compaction summary messages"))?;
            request.tools = ProviderTools::new(Vec::new())
                .map_err(|_| broker_error("compaction summary tools"))?;
            request.tool_choice = ProviderToolChoice::None;
            request.cancellation = cancellation;
            let (sink, mut events) = provider_event_channel(8, &request.cancellation)?;
            let future = provider.stream(request, sink);
            tokio::pin!(future);
            let mut summary = String::new();
            let mut provider_done = false;
            loop {
                tokio::select! {
                    result = &mut future, if !provider_done => {
                        result?;
                        provider_done = true;
                    }
                    event = events.receive() => match event? {
                        Some(ProviderEvent::TextDelta { text }) => summary.push_str(text.as_str()),
                        Some(ProviderEvent::Stop { reason: StopReason::EndTurn }) => break,
                        Some(ProviderEvent::Error { .. }) => {
                            return Err(RuntimeError::AdapterFailure {
                                code: "compaction_provider",
                                context: "summary provider failed".into(),
                            });
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
            }
            let summary = summary.trim();
            if summary.is_empty() {
                return Err(broker_error("empty compaction summary"));
            }
            Ok(CompactionSummary(summary.to_owned()))
        })
    }
}

pub(crate) struct ProductionCompactionEffects {
    pub(crate) setup: Arc<ProductionSetupPorts>,
    pub(crate) runtime_state: Arc<crate::production_components::ProductionRuntimeState>,
}

impl CompactionEffects for ProductionCompactionEffects {
    fn claim(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<CompactionRecovery, RuntimeError>> + Send + '_>> {
        let command = command.clone();
        Box::pin(async move {
            let (_, transaction) = self
                .setup
                .store
                .claim_compaction(&command.scope, &command.request_id)
                .map_err(RuntimeError::from)?;
            recovery(&transaction)
        })
    }

    fn record_projection(
        &self,
        command: &CompactionCommand,
        summary: CompactionSummary,
        retained: Vec<ProviderMessage>,
        progress: lotta_runtime::turn::CompactionProgress,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        let command = command.clone();
        Box::pin(async move {
            let current = self
                .setup
                .store
                .compaction_transaction(&command.scope, &command.request_id)
                .map_err(RuntimeError::from)?
                .ok_or_else(|| broker_error("compaction transaction missing"))?;
            let retained_message_ids =
                retained_message_ids(&self.setup.store, &command, retained.len())
                    .await?
                    .into_iter()
                    .map(|id| NonEmptyString::new(id.as_str().to_owned()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(runtime_adapter)?;
            self.setup
                .store
                .record_compaction_projection(
                    current.revision,
                    &command.scope,
                    &command.request_id,
                    CompactionProjection {
                        summary: summary.0,
                        retained_message_ids,
                        tokens_before: progress.tokens_before,
                        tokens_after: progress.tokens_after,
                        messages_before: progress.messages_before,
                        messages_after: progress.messages_after,
                    },
                )
                .map_err(RuntimeError::from)?;
            Ok(())
        })
    }

    fn lease_is_current(&self, scope: &lotta_domain::RuntimeScope, lease: &TurnLease) -> bool {
        self.runtime_state.lease_is_current(scope, lease)
    }

    fn pre_compact(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        let cancellation = command.cancellation.clone();
        let runtime = self.setup.hook_runtime();
        Box::pin(async move {
            let host = HookLifecycleHost::new(runtime.as_ref());
            let payload =
                events::lifecycle_payload(HookEvent::PreCompact).map_err(runtime_adapter)?;
            host.pre_compact(payload, cancellation, || async {
                Ok::<(), RuntimeError>(())
            })
            .await
            .map_err(|error| match error {
                LifecycleError::Operation(error) => error,
                error => {
                    setup_failure_runtime(SetupFailure::PreAdmission(lifecycle_setup_error(error)))
                }
            })
        })
    }

    fn append(
        &self,
        command: &CompactionCommand,
        summary: CompactionSummary,
        retained: Vec<ProviderMessage>,
        progress: lotta_runtime::turn::CompactionProgress,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        Box::pin(append_production_compaction(
            self,
            command.clone(),
            summary,
            retained,
            progress,
        ))
    }

    fn publish(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        Box::pin(publish_production_compaction(self, command.clone()))
    }

    fn post_compact(
        &self,
        command: &CompactionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + '_>> {
        let cancellation = command.cancellation.clone();
        let runtime = self.setup.hook_runtime();
        Box::pin(async move {
            let host = HookLifecycleHost::new(runtime.as_ref());
            let payload =
                events::lifecycle_payload(HookEvent::PostCompact).map_err(runtime_adapter)?;
            host.post_compact(payload, cancellation, || async {
                Ok::<(), RuntimeError>(())
            })
            .await
            .map_err(|error| match error {
                LifecycleError::Operation(error) => error,
                error => {
                    setup_failure_runtime(SetupFailure::PreAdmission(lifecycle_setup_error(error)))
                }
            })
        })
    }
}

fn recovery(
    transaction: &lotta_store::CompactionTransaction,
) -> Result<CompactionRecovery, RuntimeError> {
    let progress =
        transaction
            .projection
            .as_ref()
            .map(|projection| lotta_runtime::turn::CompactionProgress {
                tokens_before: projection.tokens_before,
                tokens_after: projection.tokens_after,
                messages_before: projection.messages_before,
                messages_after: projection.messages_after,
            });
    match transaction.state {
        CompactionTransactionState::Pending if progress.is_none() => {
            Ok(CompactionRecovery::Pending)
        }
        CompactionTransactionState::Pending => {
            Ok(CompactionRecovery::Planned(progress.ok_or_else(|| {
                broker_error("compaction progress missing")
            })?))
        }
        CompactionTransactionState::Appended { .. } => {
            Ok(CompactionRecovery::Appended(progress.ok_or_else(|| {
                broker_error("compaction progress missing")
            })?))
        }
        CompactionTransactionState::Published => Ok(CompactionRecovery::Published(
            progress.ok_or_else(|| broker_error("compaction progress missing"))?,
        )),
    }
}

async fn append_production_compaction(
    effects: &ProductionCompactionEffects,
    command: CompactionCommand,
    summary: CompactionSummary,
    retained: Vec<ProviderMessage>,
    progress: lotta_runtime::turn::CompactionProgress,
) -> Result<(), RuntimeError> {
    let current = effects
        .setup
        .store
        .compaction_transaction(&command.scope, &command.request_id)
        .map_err(RuntimeError::from)?
        .ok_or_else(|| broker_error("compaction transaction missing"))?;
    if matches!(current.state, CompactionTransactionState::Appended { .. }) {
        return Ok(());
    }
    let projection = current
        .projection
        .clone()
        .ok_or_else(|| broker_error("compaction projection missing"))?;
    let summary = if summary.0.is_empty() {
        CompactionSummary(projection.summary.clone())
    } else {
        summary
    };
    let now = Timestamp::from_utc(chrono::Utc::now());
    let message_id = MessageId::accept(format!("compact-{}", command.request_id.as_str()))
        .map_err(runtime_adapter)?;
    let message = LocalMessage {
        id: message_id,
        role: LocalMessageRole::User,
        content: Some(
            BoundedJsonValue::new(serde_json::json!(summary.0)).map_err(runtime_adapter)?,
        ),
        timestamp: chrono::Utc::now().timestamp_millis() as f64,
        metadata: None,
        extras: Default::default(),
    };
    let details = compaction_details(command.trigger, progress)?;
    let entry = TranscriptEntry::Compaction(CompactionEntry {
        entry_type: CompactionEntryType::Compaction,
        id: command.request_id.clone(),
        parent_id: None,
        timestamp: now,
        summary: summary.0.clone(),
        first_kept_entry_id: projection
            .retained_message_ids
            .first()
            .map(|id| id.as_str().to_owned()),
        tokens_before: progress.tokens_before,
        tokens_after: Some(progress.tokens_after),
        messages_before: Some(progress.messages_before),
        messages_after: Some(progress.messages_after),
        message,
        details: Some(details),
    });
    let _ = retained;
    effects
        .setup
        .store
        .append_compaction_if_absent(
            current.revision,
            &command.scope,
            &command.request_id,
            &entry,
        )
        .await
        .map_err(RuntimeError::from)?;
    Ok(())
}

async fn publish_production_compaction(
    effects: &ProductionCompactionEffects,
    command: CompactionCommand,
) -> Result<(), RuntimeError> {
    let current = effects
        .setup
        .store
        .compaction_transaction(&command.scope, &command.request_id)
        .map_err(RuntimeError::from)?
        .ok_or_else(|| broker_error("compaction transaction missing"))?;
    if matches!(current.state, CompactionTransactionState::Published) {
        return Ok(());
    }
    let projection = current
        .projection
        .clone()
        .ok_or_else(|| broker_error("compaction projection missing"))?;
    let mut ids = vec![
        MessageId::accept(format!("compact-{}", command.request_id.as_str()))
            .map_err(runtime_adapter)?,
    ];
    ids.extend(
        projection
            .retained_message_ids
            .iter()
            .map(|id| MessageId::accept(id.as_str().to_owned()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(runtime_adapter)?,
    );
    effects
        .setup
        .store
        .publish_compaction_context(
            &command.scope.agent_id,
            &command.scope.conversation_id,
            projection.summary,
            InContextMessageIds::new(ids).map_err(runtime_adapter)?,
            Timestamp::from_utc(chrono::Utc::now()),
        )
        .await
        .map_err(RuntimeError::from)?;
    effects
        .setup
        .store
        .mark_compaction_published(current.revision, &command.scope, &command.request_id)
        .map_err(RuntimeError::from)?;
    Ok(())
}

async fn retained_message_ids(
    store: &LocalStore,
    command: &CompactionCommand,
    retained: usize,
) -> Result<Vec<MessageId>, RuntimeError> {
    let loaded = store
        .load_transcript(&command.scope.agent_id, &command.scope.conversation_id)
        .await
        .map_err(RuntimeError::from)?;
    Ok(loaded
        .messages()
        .iter()
        .rev()
        .take(retained)
        .map(|message| message.id.clone())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect())
}

fn compaction_details(
    trigger: CompactionTrigger,
    progress: lotta_runtime::turn::CompactionProgress,
) -> Result<lotta_domain::BoundedMap<1_024>, RuntimeError> {
    let value = serde_json::json!({
        "stats": {
            "trigger": match trigger {
                CompactionTrigger::Manual => "manual",
                CompactionTrigger::Pressure => "pressure",
                CompactionTrigger::ProviderOverflow => "provider_overflow",
            },
            "context_tokens_before": progress.tokens_before,
            "context_tokens_after": progress.tokens_after,
            "messages_count_before": progress.messages_before,
            "messages_count_after": progress.messages_after,
        }
    });
    let object = value
        .as_object()
        .ok_or_else(|| broker_error("compaction details"))?;
    lotta_domain::BoundedMap::new(
        object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
    .map_err(runtime_adapter)
}

pub(crate) struct RegisteredProductionCompaction {
    pub(crate) service:
        CompactionService<ProductionProviderSummarizer, ProductionCompactionEffects>,
}

impl ProductionCompactionService for RegisteredProductionCompaction {
    fn compact(
        &self,
        command: CompactionCommand,
    ) -> BrokerFuture<'_, lotta_runtime::turn::CompactionProgress> {
        Box::pin(self.service.compact(command))
    }
}

struct RequestRefreshAdapter {
    setup: Arc<ProductionSetupPorts>,
    state: Arc<AsyncMutex<PromptRefreshState>>,
    agent: AgentId,
    conversation: ConversationId,
    input: String,
    input_id: String,
    cancellation: CancellationToken,
}

struct PromptRefreshState {
    prompt: String,
    model: ResolvedTurnModel,
    catalog: TurnToolCatalog,
}

async fn compile_prompt_for_refresh(
    setup: &ProductionSetupPorts,
    state: &PromptRefreshState,
    agent_id: &AgentId,
    conversation_id: &ConversationId,
    cancellation: &CancellationToken,
) -> Result<String, RuntimeError> {
    let agent = AgentStore::load(&setup.store, agent_id).await?;
    let conversation = ConversationStore::load(&setup.store, agent_id, conversation_id).await?;
    let sections = PromptSections {
        tool_guidance: prompt_tool_guidance(&state.catalog)?,
        model_guidance: vec![PromptText::new(format!(
            "Active model: {}. Context window: {} tokens. Output limit: {} tokens.",
            state.model.model.handle.as_str(),
            state.model.context_window,
            state.model.output_tokens,
        ))?],
        ..PromptSections::default()
    };
    let inputs = PromptInputs::new(
        PromptText::new(agent.system)?,
        agent.id,
        conversation.id,
        0,
        Timestamp::from_utc(chrono::Utc::now()),
        sections,
    )?;
    setup
        .prompt_cache(agent_id, conversation_id)?
        .get_or_compile(
            &PromptCompiler::new(&setup.memfs),
            &inputs,
            DeliveryCapability::RequestBoundaryOnly,
            cancellation.clone(),
        )
        .await
        .map(|delivery| delivery.delivery.content)
}

fn prompt_tool_guidance(catalog: &TurnToolCatalog) -> Result<Vec<PromptText>, RuntimeError> {
    catalog
        .definitions()
        .map(|definition| {
            PromptText::new(format!(
                "Available tool: {}.",
                definition.model_name.as_str()
            ))
        })
        .collect()
}

impl lotta_runtime::turn::RequestRefreshPort for RequestRefreshAdapter {
    fn seed(&self, prepared: &SetupOutput) -> Result<(), RuntimeError> {
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| broker_error("refresh state lock"))?;
        state.prompt.clone_from(&prepared.prompt);
        state.model.clone_from(&prepared.model);
        state.catalog = TurnToolCatalog::new(prepared.tools.definitions().cloned().collect())?;
        Ok(())
    }

    fn refresh(
        &self,
        _request: ProviderRequest,
        _: TurnLease,
    ) -> lotta_runtime::ports::PortFuture<'static, ProviderRequest> {
        let setup = Arc::clone(&self.setup);
        let state = self.state.clone();
        let agent = self.agent.clone();
        let conversation = self.conversation.clone();
        let input = self.input.clone();
        let input_id = self.input_id.clone();
        let cancellation = self.cancellation.clone();
        Box::pin(async move {
            let mut state = state.lock().await;
            let compiled =
                compile_prompt_for_refresh(&setup, &state, &agent, &conversation, &cancellation)
                    .await?;
            if compiled.is_empty() {
                return Err(broker_error("empty refreshed prompt"));
            }
            state.prompt = compiled;
            if state.prompt.is_empty() {
                return Err(broker_error("empty seeded prompt"));
            }
            setup
                .build_request(
                    &agent,
                    &conversation,
                    state.prompt.clone(),
                    &input,
                    Some(&input_id),
                    &state.model,
                    &state.catalog,
                    cancellation,
                )
                .await
        })
    }
}

/// Object-safe production turn controller injected into the application server.
pub struct ProductionTurnController {
    setup: Arc<ProductionSetupPorts>,
    runtime_state: Arc<crate::production_components::ProductionRuntimeState>,
    provider: Arc<crate::production_components::ProductionProviderPort>,
    tools: Arc<crate::production_components::ProductionToolPort>,
    store: LocalStore,
    clock: Arc<dyn lotta_domain::Clock + Send + Sync>,
    fallback_cwd: PathBuf,
    turn_sequence: AtomicU64,
    brokers: Arc<ProductionTurnBrokers>,
    approvals: Arc<lotta_runtime::ApprovalManager>,
    reflection: Arc<dyn ReflectionJob>,
    memory_push: Arc<dyn MemoryPushJob>,
    skills: Arc<lotta_app_server::ws::skills::SkillsBridge>,
    settings: Arc<lotta_app_server::ws::settings::SettingsBridge>,
    #[cfg(test)]
    cancellation_stage_observer:
        Mutex<Option<crate::production_components::CancellationStageObserver>>,
}

impl ProductionTurnController {
    /// Creates the controller with all real runtime/provider/tool/turn dependencies.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit production dependency graph"
    )]
    pub(crate) fn new(
        setup: Arc<ProductionSetupPorts>,
        provider: Arc<crate::production_components::ProductionProviderPort>,
        tools: Arc<crate::production_components::ProductionToolPort>,
        store: LocalStore,
        clock: Arc<dyn lotta_domain::Clock + Send + Sync>,
        fallback_cwd: PathBuf,
        runtime_state: Arc<crate::production_components::ProductionRuntimeState>,
        brokers: Arc<ProductionTurnBrokers>,
        approvals: Arc<lotta_runtime::ApprovalManager>,
        reflection: Arc<dyn ReflectionJob>,
        memory_push: Arc<dyn MemoryPushJob>,
        skills: Arc<lotta_app_server::ws::skills::SkillsBridge>,
        settings: Arc<lotta_app_server::ws::settings::SettingsBridge>,
    ) -> Self {
        Self {
            setup,
            runtime_state,
            provider,
            tools,
            store,
            clock,
            fallback_cwd,
            turn_sequence: AtomicU64::new(1),
            brokers,
            approvals,
            reflection,
            memory_push,
            skills,
            settings,
            #[cfg(test)]
            cancellation_stage_observer: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_cancellation_stages(
        &self,
        observer: crate::production_components::CancellationStageObserver,
    ) {
        *self
            .cancellation_stage_observer
            .lock()
            .expect("cancellation stage observer") = Some(observer);
    }

    fn validate_submission(
        command: &lotta_app_server::ws::command::InputCommand,
        deferred: &lotta_app_server::ws::DeferredInput,
    ) -> Result<String, lotta_app_server::error::AppServerError> {
        let admission_id = deferred
            .continuation
            .as_ref()
            .and_then(|value| value.as_value().get("client_message_id"))
            .and_then(serde_json::Value::as_str)
            .ok_or(lotta_app_server::error::AppServerError::Malformed)?
            .to_owned();
        if deferred.scope != command.runtime
            || input_client_message_id(command.payload.as_value())? != admission_id
        {
            return Err(lotta_app_server::error::AppServerError::Malformed);
        }
        Ok(admission_id)
    }

    async fn run_prompt_hook(
        &self,
        command: &lotta_app_server::ws::command::InputCommand,
        text: &str,
        cancellation: CancellationToken,
    ) -> Result<(), lotta_app_server::error::AppServerError> {
        let payload = user_prompt_payload(command, text)?;
        let host = HookLifecycleHost::new(self.setup.hook_runtime.as_ref());
        match host
            .user_prompt_submit(payload, cancellation, || async {
                Ok::<(), RuntimeError>(())
            })
            .await
        {
            Ok(()) => Ok(()),
            Err(LifecycleError::Operation(error)) => Err(app_server_error(error)),
            Err(error) => Err(app_server_error(setup_failure_runtime(
                SetupFailure::PreAdmission(lifecycle_setup_error(error)),
            ))),
        }
    }

    /// Resolves the requested cwd for one upcoming turn.
    ///
    /// A scoped device-state cwd override wins: an existing directory serves
    /// as requested while a missing one is handed to setup unresolved so its
    /// durable deleted-cwd reminder machinery fires (once) against the boot
    /// fallback. Without an override the conversation's persisted directory,
    /// then the boot fallback, apply.
    fn scoped_cwd(
        &self,
        scope: &lotta_domain::RuntimeScope,
        conversation: &Conversation,
    ) -> PathBuf {
        let agent = Some(scope.agent_id.as_str());
        let conversation_id = scope.conversation_id.as_str();
        let has_override = self.settings.cwd_override(agent, conversation_id).is_some();
        if !has_override {
            return persisted_cwd(conversation).unwrap_or_else(|| self.fallback_cwd.clone());
        }
        match self.settings.cwd_for_next_turn(agent, conversation_id) {
            lotta_runtime::turn::CwdResolution::Requested(path) => path,
            lotta_runtime::turn::CwdResolution::DeletedFallback { original, .. } => {
                let _claimed = self
                    .settings
                    .claim_missing_cwd_reminder(agent, conversation_id);
                original
            }
        }
    }

    /// Returns the runtime-selected skill sources shared with the skills
    /// bridge, in selection order.
    fn selected_skill_ids(&self) -> Vec<String> {
        self.skills
            .selected_sources()
            .lock()
            .map(|selection| selection.ids())
            .unwrap_or_default()
    }

    async fn prepare_turn(
        &self,
        command: &lotta_app_server::ws::command::InputCommand,
        text: String,
        cancellation: CancellationToken,
        sink: Arc<dyn lotta_app_server::ws::RuntimeEventSink>,
    ) -> Result<
        (SetupInput, crate::production_components::ProductionEffects),
        lotta_app_server::error::AppServerError,
    > {
        let sequence = self.turn_sequence.fetch_add(1, Ordering::Relaxed);
        let turn_id = NonEmptyString::new(format!("turn-{sequence}"))
            .map_err(|_| lotta_app_server::error::AppServerError::Internal)?;
        let run_id = lotta_domain::RunId::generate_sequence(sequence)
            .map_err(|_| lotta_app_server::error::AppServerError::Internal)?;
        let agent = AgentStore::load(&self.setup.store, &command.runtime.agent_id)
            .await
            .map_err(app_server_error)?;
        let conversation = ConversationStore::load(
            &self.setup.store,
            &command.runtime.agent_id,
            &command.runtime.conversation_id,
        )
        .await
        .map_err(app_server_error)?;
        let status_sink: Arc<dyn SetupStatusSink> = Arc::new(WebSocketStatusSink {
            scope: command.runtime.clone(),
            sink: Arc::clone(&sink),
        });
        let input = SetupInput {
            agent_id: command.runtime.agent_id.clone(),
            conversation_id: command.runtime.conversation_id.clone(),
            cwd: self.scoped_cwd(&command.runtime, &conversation),
            fallback_cwd: self.fallback_cwd.clone(),
            user_input: text,
            selected_skills: self.selected_skill_ids(),
            permission_mode: configured_permission_mode(&agent),
            cancellation,
            deadline: std::time::Duration::from_secs(SETUP_DEADLINE_SECONDS),
            status_sink,
        };
        let input_id = input_client_message_id(command.payload.as_value())?;
        let effects = crate::production_components::ProductionEffects::new(
            self.store.clone(),
            command.runtime.clone(),
            sink,
            Arc::clone(&self.runtime_state),
            turn_id,
            run_id,
            NonEmptyString::new(input_id)
                .map_err(|_| lotta_app_server::error::AppServerError::Malformed)?,
        );
        #[cfg(test)]
        let effects = if let Some(observer) = self
            .runtime_state
            .cancellation_observer
            .lock()
            .expect("cancellation observer")
            .clone()
        {
            effects.observe_cancellation(observer)
        } else {
            effects
        };
        Ok((input, effects))
    }

    async fn run_admitted(
        &self,
        command: &lotta_app_server::ws::command::InputCommand,
        pending: &crate::production_components::PendingAdmission,
        cancellation: CancellationToken,
        sink: Arc<dyn lotta_app_server::ws::RuntimeEventSink>,
    ) -> Result<lotta_runtime::turn::TurnRunOutcome, lotta_app_server::error::AppServerError> {
        let text = canonical_user_text(command.payload.as_value())?;
        self.run_prompt_hook(command, &text, cancellation.clone())
            .await?;
        let scope = command.runtime.clone();
        let turn_cancellation = cancellation.clone();
        let (input, effects) = self
            .prepare_turn(command, text, cancellation, Arc::clone(&sink))
            .await?;
        let approval = self.approval_adapter(&scope, pending, &effects);
        let controller_tools = self.controller_tool_adapter(&scope, pending, &sink);
        let compaction = CompactionBrokerAdapter {
            brokers: Arc::clone(&self.brokers),
            scope: scope.clone(),
            cancellation: turn_cancellation.clone(),
            sink,
        };
        let refresh = self
            .request_refresh(command, &scope, &input, turn_cancellation)
            .await?;
        let children = self
            .tools
            .child_owner(&scope, pending.lease.generation())
            .map_err(app_server_error)?;
        let post_turn = ProductionPostTurn::new(
            self.store.clone(),
            scope.clone(),
            pending.lease.generation(),
            Arc::clone(&self.reflection),
            Arc::clone(&self.memory_push),
        );
        #[cfg(test)]
        let cancellation_stage_observer = self
            .cancellation_stage_observer
            .lock()
            .expect("cancellation stage observer")
            .clone();
        let mut state = self.runtime_state.inner.lock().await;
        self.setup
            .run_production_turn(
                &mut state.registry,
                pending.handle.clone(),
                pending.lease.clone(),
                input,
                &approval,
                &controller_tools,
                &compaction,
                refresh,
                &children,
                &post_turn,
                self.provider.as_ref(),
                self.tools.as_ref(),
                &effects,
                #[cfg(test)]
                cancellation_stage_observer,
            )
            .await
            .map_err(app_server_error)
    }

    fn approval_adapter(
        &self,
        scope: &lotta_domain::RuntimeScope,
        pending: &crate::production_components::PendingAdmission,
        effects: &crate::production_components::ProductionEffects,
    ) -> ApprovalBrokerAdapter {
        ApprovalBrokerAdapter {
            manager: Arc::clone(&self.approvals),
            scope: scope.clone(),
            run_id: effects.run_id.clone(),
            turn_id: effects.turn_id.clone(),
            input_id: effects.input_id.clone(),
            lease_generation: pending.lease.generation(),
            clock: Arc::clone(&self.clock),
        }
    }

    fn controller_tool_adapter(
        &self,
        scope: &lotta_domain::RuntimeScope,
        pending: &crate::production_components::PendingAdmission,
        sink: &Arc<dyn lotta_app_server::ws::RuntimeEventSink>,
    ) -> ControllerToolBrokerAdapter {
        ControllerToolBrokerAdapter {
            brokers: Arc::clone(&self.brokers),
            scope: scope.clone(),
            lease_generation: pending.lease.generation(),
            sink: Arc::clone(sink),
        }
    }

    async fn request_refresh(
        &self,
        command: &lotta_app_server::ws::command::InputCommand,
        scope: &lotta_domain::RuntimeScope,
        input: &SetupInput,
        cancellation: CancellationToken,
    ) -> Result<
        Arc<dyn lotta_runtime::turn::RequestRefreshPort>,
        lotta_app_server::error::AppServerError,
    > {
        let agent = AgentStore::load(&self.setup.store, &scope.agent_id)
            .await
            .map_err(app_server_error)?;
        let conversation =
            ConversationStore::load(&self.setup.store, &scope.agent_id, &scope.conversation_id)
                .await
                .map_err(app_server_error)?;
        let model = self
            .setup
            .resolve_model(&agent, &conversation)
            .map_err(|error| app_server_error(setup_error_runtime(error)))?;
        Ok(Arc::new(RequestRefreshAdapter {
            setup: Arc::clone(&self.setup),
            state: Arc::new(AsyncMutex::new(PromptRefreshState {
                prompt: String::new(),
                model,
                catalog: TurnToolCatalog::new(Vec::new()).expect("empty catalog"),
            })),
            agent: scope.agent_id.clone(),
            conversation: scope.conversation_id.clone(),
            input: input.user_input.clone(),
            input_id: input_client_message_id(command.payload.as_value())?,
            cancellation,
        }))
    }

    fn emit_completion(
        command: &lotta_app_server::ws::command::InputCommand,
        sink: &dyn lotta_app_server::ws::RuntimeEventSink,
    ) -> Result<(), lotta_app_server::error::AppServerError> {
        sink.emit(
            &command.runtime,
            lotta_app_server::ws::RuntimeEvent::UpdateLoopStatus {
                loop_status: lotta_app_server::ws::event::LoopState {
                    status: lotta_app_server::ws::event::LoopStatus::WaitingOnInput,
                    active_run_ids: Vec::new(),
                    executing_tool_call_ids: Vec::new(),
                },
            },
        )?;
        Ok(())
    }
}

fn attach_controller_error(
    primary: Option<lotta_app_server::error::AppServerError>,
    cleanup: lotta_app_server::error::AppServerError,
) -> lotta_app_server::error::AppServerError {
    match primary {
        Some(primary) => lotta_app_server::error::AppServerError::CleanupAttached {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        },
        None => cleanup,
    }
}

fn activate_submission(
    controller: &ProductionTurnController,
    command: &lotta_app_server::ws::command::InputCommand,
    deferred: &lotta_app_server::ws::DeferredInput,
    listener_cancellation: CancellationToken,
) -> Result<
    (
        crate::production_components::PendingAdmission,
        CancellationToken,
    ),
    lotta_app_server::error::AppServerError,
> {
    let admission_id = ProductionTurnController::validate_submission(command, deferred)?;
    let pending = controller
        .runtime_state
        .take_pending(&command.runtime, &admission_id)?;
    let active_cancellation = pending.cancellation.clone();
    let key = lotta_runtime::RuntimeKey::from(&command.runtime);
    let active = crate::production_components::ActiveAdmission {
        handle: pending.handle.clone(),
        lease: pending.lease.clone(),
        cancellation: active_cancellation.clone(),
        queue: Arc::new(std::sync::Mutex::new(
            lotta_runtime::ConversationQueue::default(),
        )),
        history: lotta_domain::AdmissionHistory::default(),
    };
    let replaced = {
        let mut active_state = controller
            .runtime_state
            .active
            .lock()
            .map_err(|_| lotta_app_server::error::AppServerError::Internal)?;
        active_state.insert(key, active).is_some()
    };
    if replaced {
        return Err(lotta_app_server::error::AppServerError::Internal);
    }
    let watcher = active_cancellation.clone();
    tokio::spawn(async move {
        listener_cancellation.cancelled().await;
        watcher.cancel();
    });
    Ok((pending, active_cancellation))
}

async fn submit_production_turn(
    controller: &ProductionTurnController,
    command: lotta_app_server::ws::command::InputCommand,
    deferred: lotta_app_server::ws::DeferredInput,
    cancellation: CancellationToken,
    sink: Arc<dyn lotta_app_server::ws::RuntimeEventSink>,
) -> Result<(), lotta_app_server::error::AppServerError> {
    let mut command = command;
    let mut deferred = deferred;
    let mut cancellation = cancellation;
    let mut primary = None;
    loop {
        let (pending, active_cancellation) =
            match activate_submission(controller, &command, &deferred, cancellation) {
                Ok(active) => active,
                Err(error) => return Err(attach_controller_error(primary, error)),
            };
        let result = controller
            .run_admitted(&command, &pending, active_cancellation, Arc::clone(&sink))
            .await;
        let cancelled = matches!(
            result,
            Ok(lotta_runtime::turn::TurnRunOutcome::Cancelled(_))
        );
        if cancelled {
            primary = None;
        } else if primary.is_none() {
            primary = result.err();
        }
        let reason = if cancelled {
            "user_cancellation"
        } else if primary.is_none() {
            "completed"
        } else {
            "error"
        };
        let pumped = match controller
            .runtime_state
            .release_and_pump(&command.runtime, pending, reason, cancelled)
            .await
        {
            Ok(pumped) => pumped,
            Err(error) => return Err(attach_controller_error(primary, error)),
        };
        if !cancelled
            && let Err(error) = ProductionTurnController::emit_completion(&command, sink.as_ref())
        {
            primary = Some(attach_controller_error(primary, error));
        }
        let Some((item, continuation)) = pumped else {
            return primary.map_or(Ok(()), Err);
        };
        if primary.as_ref().is_some_and(|error| {
            matches!(error, lotta_app_server::error::AppServerError::Malformed)
        }) {
            primary = None;
        }
        command = lotta_app_server::ws::command::InputCommand {
            request_id: None,
            runtime: command.runtime.clone(),
            payload: item.content,
        };
        deferred = lotta_app_server::ws::DeferredInput {
            scope: command.runtime.clone(),
            disposition: lotta_domain::InputDisposition::Started,
            continuation: Some(continuation),
        };
        cancellation = CancellationToken::new();
    }
}

impl lotta_app_server::ws::TurnController for ProductionTurnController {
    fn submit_turn(
        &self,
        command: lotta_app_server::ws::command::InputCommand,
        deferred: lotta_app_server::ws::DeferredInput,
        cancellation: CancellationToken,
        sink: Arc<dyn lotta_app_server::ws::RuntimeEventSink>,
    ) -> lotta_app_server::ws::service::ServiceFuture<'_, ()> {
        Box::pin(submit_production_turn(
            self,
            command,
            deferred,
            cancellation,
            sink,
        ))
    }
}

pub(crate) struct UnavailableReflection;

impl ReflectionJob for UnavailableReflection {
    fn reflect(&self, _: &PostTurnJob) -> Result<PostTurnExecution, StoreError> {
        Ok(PostTurnExecution::Unavailable)
    }
}

pub(crate) struct UnavailableMemoryPush;

impl MemoryPushJob for UnavailableMemoryPush {
    fn push_memory(&self, _: &PostTurnJob) -> Result<PostTurnExecution, StoreError> {
        Ok(PostTurnExecution::Unavailable)
    }
}

struct ProductionPostTurn {
    queue: PostTurnQueue,
    scope: lotta_domain::RuntimeScope,
    lease_generation: u64,
    reflection: Arc<dyn ReflectionJob>,
    memory_push: Arc<dyn MemoryPushJob>,
}

impl ProductionPostTurn {
    fn new(
        store: LocalStore,
        scope: lotta_domain::RuntimeScope,
        lease_generation: u64,
        reflection: Arc<dyn ReflectionJob>,
        memory_push: Arc<dyn MemoryPushJob>,
    ) -> Self {
        Self {
            queue: PostTurnQueue::new(&store),
            scope,
            lease_generation,
            reflection,
            memory_push,
        }
    }

    fn enqueue_and_drain(&self) -> Result<(), RuntimeError> {
        self.queue
            .enqueue_turn(&self.scope, self.lease_generation)
            .map_err(RuntimeError::from)?;
        PostTurnJobRunner::new(
            &self.queue,
            self.reflection.as_ref(),
            self.memory_push.as_ref(),
        )
        .drain()
        .map_err(RuntimeError::from)
    }
}

impl lotta_runtime::turn::PostTurnPort for ProductionPostTurn {
    fn run(&self) -> lotta_runtime::ports::PortFuture<'_, ()> {
        Box::pin(async move { self.enqueue_and_drain() })
    }
}

struct WebSocketStatusSink {
    scope: lotta_domain::RuntimeScope,
    sink: Arc<dyn lotta_app_server::ws::RuntimeEventSink>,
}

impl SetupStatusSink for WebSocketStatusSink {
    fn emit(&self, status: SetupStatus) -> Result<(), SetupError> {
        let status = match status {
            SetupStatus::Sending => lotta_app_server::ws::event::LoopStatus::SendingApiRequest,
            SetupStatus::Waiting => lotta_app_server::ws::event::LoopStatus::WaitingForApiResponse,
        };
        let loop_status = lotta_app_server::ws::event::LoopState {
            status,
            active_run_ids: Vec::new(),
            executing_tool_call_ids: Vec::new(),
        };
        self.sink
            .emit(
                &self.scope,
                lotta_app_server::ws::RuntimeEvent::UpdateLoopStatus { loop_status },
            )
            .map_err(|_| SetupError::Adapter("websocket status delivery".into()))
    }
}

fn persisted_cwd(conversation: &Conversation) -> Option<PathBuf> {
    conversation
        .extras
        .get("working_directory")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
}

fn configured_permission_mode(agent: &Agent) -> PermissionMode {
    match agent
        .model_settings
        .get("permissionMode")
        .and_then(serde_json::Value::as_str)
    {
        Some("standard") => PermissionMode::Standard,
        Some("acceptEdits") => PermissionMode::AcceptEdits,
        Some("strict") => PermissionMode::Strict,
        _ => PermissionMode::Unrestricted,
    }
}

fn input_client_message_id(
    value: &serde_json::Value,
) -> Result<String, lotta_app_server::error::AppServerError> {
    value
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .and_then(|messages| messages.first())
        .and_then(|message| message.get("client_message_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or(lotta_app_server::error::AppServerError::Malformed)
}

fn user_prompt_payload(
    command: &lotta_app_server::ws::command::InputCommand,
    text: &str,
) -> Result<lotta_runtime::hooks::HookPayload, lotta_app_server::error::AppServerError> {
    let mut fields = serde_json::Map::new();
    fields.insert(
        "agent_id".into(),
        serde_json::Value::String(command.runtime.agent_id.as_str().to_owned()),
    );
    fields.insert(
        "conversation_id".into(),
        serde_json::Value::String(command.runtime.conversation_id.as_str().to_owned()),
    );
    fields.insert("prompt".into(), serde_json::Value::String(text.to_owned()));
    fields.insert(
        "prompt_metadata".into(),
        serde_json::json!({
            "client_message_id": input_client_message_id(command.payload.as_value())?,
            "message_count": command
                .payload
                .as_value()
                .get("messages")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len),
        }),
    );
    events::payload(HookEvent::UserPromptSubmit, fields)
        .map_err(|_| lotta_app_server::error::AppServerError::Malformed)
}

fn canonical_user_text(
    value: &serde_json::Value,
) -> Result<String, lotta_app_server::error::AppServerError> {
    let messages = value
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .ok_or(lotta_app_server::error::AppServerError::Malformed)?;
    let mut text = String::new();
    for message in messages {
        let content = message
            .get("content")
            .and_then(serde_json::Value::as_str)
            .or_else(|| message.as_str())
            .or_else(|| {
                message
                    .get("content")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|parts| parts.first())
                    .and_then(|part| part.get("text"))
                    .and_then(serde_json::Value::as_str)
            })
            .ok_or(lotta_app_server::error::AppServerError::Malformed)?;
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(content);
    }
    if text.is_empty() {
        return Err(lotta_app_server::error::AppServerError::Malformed);
    }
    Ok(text)
}

fn app_server_error(_: RuntimeError) -> lotta_app_server::error::AppServerError {
    lotta_app_server::error::AppServerError::Internal
}

#[cfg(test)]
mod tests;
