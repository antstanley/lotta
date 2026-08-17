use super::host::ModHost;
use super::protocol::{RpcMethod, RpcParams, RpcResult};
use super::registrations::{
    CommandRegistration, LifecycleRegistration, ModRegistrationSnapshot, PermissionRegistration,
    ProviderRegistration, ToolRegistration,
};
use super::types::{
    Generation, MOD_REGISTRATIONS_ITEMS_MAX, ModError, ModId, ModOwner, RegistrationName,
};
use lotta_runtime::ports::{
    InternalToolName, ModelFacingToolName, PermissionAction, SecretRedactionSpec,
    ToolApprovalPolicy, ToolDefinition, ToolDescriptionAsset, ToolExecutionOwner, ToolInputSchema,
    ToolOutputLimit, ToolTimeout,
};
use lotta_tools::pipeline::{
    ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
};
use lotta_tools::registry::{
    RegistryError, ToolRegistration as RuntimeToolRegistration, ToolRegistry,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

/// Bounded mod tool execution timeout published into the Task 32 registry.
pub const MOD_TOOL_TIMEOUT: Duration = Duration::from_mins(5);
/// Bounded mod tool result size, in bytes, published into the Task 32 registry.
pub const MOD_TOOL_RESULT_BYTES_MAX: usize = 1_048_576;
/// Bounded mod tool model-facing result size, in characters.
pub const MOD_TOOL_RESULT_MODEL_CHARS_MAX: usize = 32_000;

/// Future returned by command, tool, and lifecycle dispatch.
pub type RegistrationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, ModError>> + Send + 'a>>;

/// Shared active generation map committed with authoritative registration publication.
///
/// Every mod access and invoke path consults this one table, so a stale generation is refused
/// identically whether it is reached through the Task 32 executor or a runtime snapshot handle.
pub struct ActiveGenerationTable {
    current: RwLock<BTreeMap<String, Generation>>,
}
impl ActiveGenerationTable {
    /// Creates an empty generation table.
    #[must_use]
    pub fn new() -> Self {
        Self {
            current: RwLock::new(BTreeMap::new()),
        }
    }
    /// Creates a table already holding the exact active owner generations.
    #[must_use]
    pub fn activated(owners: impl IntoIterator<Item = ModOwner>) -> Self {
        let current = owners
            .into_iter()
            .map(|owner| (owner.id.as_str().to_owned(), owner.generation))
            .collect();
        Self {
            current: RwLock::new(current),
        }
    }
    /// Refuses stale owners before any host call or registration read.
    pub fn validate(&self, owner: &ModOwner) -> Result<(), ModError> {
        let current = self.current.read().map_err(|_| ModError::Publication)?;
        if current.get(owner.id.as_str()) != Some(&owner.generation) {
            return Err(ModError::InvalidScope);
        }
        Ok(())
    }
    /// Returns the exact active generation for one mod identity.
    pub fn current(&self, id: &ModId) -> Result<Option<Generation>, ModError> {
        let current = self.current.read().map_err(|_| ModError::Publication)?;
        Ok(current.get(id.as_str()).copied())
    }
    /// Number of owners with an active generation.
    pub fn len(&self) -> Result<usize, ModError> {
        let current = self.current.read().map_err(|_| ModError::Publication)?;
        Ok(current.len())
    }
    /// Whether no owner is active.
    pub fn is_empty(&self) -> Result<bool, ModError> {
        Ok(self.len()? == 0)
    }
}
impl Default for ActiveGenerationTable {
    fn default() -> Self {
        Self::new()
    }
}

/// One fully validated mod generation ready for atomic publication.
#[derive(Clone)]
pub struct ModPublication {
    /// Exact owner generation.
    pub owner: ModOwner,
    /// Immutable validated six-kind registrations.
    pub registrations: Arc<ModRegistrationSnapshot>,
    /// Callable host that owns this generation.
    pub host: Arc<dyn ModHost>,
}

struct ModRuntimeState {
    registrations: Arc<ModRegistrationSnapshot>,
    hosts: BTreeMap<String, Arc<dyn ModHost>>,
}
impl ModRuntimeState {
    fn empty() -> Self {
        Self {
            registrations: Arc::new(ModRegistrationSnapshot::default()),
            hosts: BTreeMap::new(),
        }
    }
}

/// Atomic six-kind registry publication coordinated with Task 32 tool publication.
pub struct ModRegistries {
    state: RwLock<Arc<ModRuntimeState>>,
    tools: Arc<ToolRegistry>,
    transaction: Mutex<()>,
    generations: Arc<ActiveGenerationTable>,
}
impl ModRegistries {
    /// Creates empty mod stores over an existing Task 32 registry.
    #[must_use]
    pub fn new(tools: Arc<ToolRegistry>) -> Self {
        Self {
            state: RwLock::new(Arc::new(ModRuntimeState::empty())),
            tools,
            transaction: Mutex::new(()),
            generations: Arc::new(ActiveGenerationTable::new()),
        }
    }
    /// Captures the immutable complete mod registration snapshot.
    pub fn snapshot(&self) -> Result<Arc<ModRegistrationSnapshot>, ModError> {
        self.state
            .read()
            .map(|state| Arc::clone(&state.registrations))
            .map_err(|_| ModError::Publication)
    }
    /// Captures the generation-checking runtime view over every retained host.
    pub fn runtime(&self) -> Result<ModRuntimeSnapshot, ModError> {
        let state = self
            .state
            .read()
            .map(|state| Arc::clone(&state))
            .map_err(|_| ModError::Publication)?;
        Ok(ModRuntimeSnapshot {
            state,
            generations: Arc::clone(&self.generations),
        })
    }
    /// Borrows the shared table consulted by every access and invoke path.
    #[must_use]
    pub fn generations(&self) -> &Arc<ActiveGenerationTable> {
        &self.generations
    }
    /// Atomically replaces the complete mod world with exactly these retained generations.
    pub fn commit(&self, published: &[ModPublication]) -> Result<(), ModError> {
        self.commit_with_barrier(published, || {})
    }
    /// Commits through the Task 32 transaction barrier seam.
    ///
    /// `barrier` runs after every candidate is composed and before the Task 32 write lock is taken,
    /// which is the only window a competing publication can still win. A losing transaction leaves
    /// the exact prior mod snapshot, generation table, and tool revision unchanged.
    pub fn commit_with_barrier<B: FnOnce()>(
        &self,
        published: &[ModPublication],
        barrier: B,
    ) -> Result<(), ModError> {
        let _transaction = self.transaction.lock().map_err(|_| ModError::Publication)?;
        let revision = self.tools.revision().map_err(map_registry)?;
        let (toolset, existing) = self
            .tools
            .snapshot()
            .map_err(map_registry)?
            .complete_registrations();
        let mut candidates: Vec<_> = existing
            .into_iter()
            .filter(|item| item.definition.execution_owner != ToolExecutionOwner::ModSidecar)
            .collect();
        let merged = merge(published)?;
        for entry in published {
            candidates.extend(tool_candidate(entry, Arc::clone(&self.generations))?);
        }
        let next = Arc::new(ModRuntimeState {
            registrations: Arc::new(merged.registrations),
            hosts: merged.hosts,
        });
        let mut state_guard = self.state.write().map_err(|_| ModError::Publication)?;
        let mut generation_guard = self
            .generations
            .current
            .write()
            .map_err(|_| ModError::Publication)?;
        self.tools
            .publish_transaction_with_barrier(revision, toolset, &candidates, None, barrier, || {
                *generation_guard = merged.generations;
                *state_guard = next;
            })
            .map_err(map_registry)?;
        Ok(())
    }
    /// Removes all mod registrations while retaining Task 32 non-mod registrations.
    pub fn clear_mods(&self) -> Result<(), ModError> {
        self.commit(&[])
    }
}

/// Generation-checking read view over every retained host's six registration kinds.
#[derive(Clone)]
pub struct ModRuntimeSnapshot {
    state: Arc<ModRuntimeState>,
    generations: Arc<ActiveGenerationTable>,
}
impl ModRuntimeSnapshot {
    /// Borrows the aggregated immutable registrations for every retained host.
    #[must_use]
    pub fn registrations(&self) -> &ModRegistrationSnapshot {
        &self.state.registrations
    }
    /// Resolves a callable tool only while its owning generation is current.
    pub fn tool(&self, name: &RegistrationName) -> Result<ModToolHandle, ModError> {
        let registration =
            self.checked(self.state.registrations.tools.get(name), |item| &item.owner)?;
        Ok(ModToolHandle {
            host: self.host(&registration.owner)?,
            registration: registration.clone(),
            generations: Arc::clone(&self.generations),
        })
    }
    /// Resolves a callable command only while its owning generation is current.
    pub fn command(&self, id: &RegistrationName) -> Result<ModCommandHandle, ModError> {
        let registration = self.checked(self.state.registrations.commands.get(id), |item| {
            &item.owner
        })?;
        Ok(ModCommandHandle {
            host: self.host(&registration.owner)?,
            registration: registration.clone(),
            generations: Arc::clone(&self.generations),
        })
    }
    /// Resolves a callable Task 44 lifecycle event only while its owning generation is current.
    pub fn lifecycle(&self, id: &RegistrationName) -> Result<ModLifecycleHandle, ModError> {
        let registration = self
            .checked(self.state.registrations.lifecycle_events.get(id), |item| {
                &item.owner
            })?;
        Ok(ModLifecycleHandle {
            host: self.host(&registration.owner)?,
            registration: registration.clone(),
            generations: Arc::clone(&self.generations),
        })
    }
    /// Reads a provider declaration only while its owning generation is current.
    pub fn provider(&self, name: &RegistrationName) -> Result<ProviderRegistration, ModError> {
        self.checked(self.state.registrations.providers.get(name), |item| {
            &item.owner
        })
        .cloned()
    }
    /// Reads a permission declaration only while its owning generation is current.
    pub fn permission(&self, id: &RegistrationName) -> Result<PermissionRegistration, ModError> {
        self.checked(self.state.registrations.permissions.get(id), |item| {
            &item.owner
        })
        .cloned()
    }
    /// Reads UI metadata only while its owning generation is current.
    pub fn ui_metadata(
        &self,
        id: &RegistrationName,
    ) -> Result<super::registrations::UiRegistration, ModError> {
        self.checked(self.state.registrations.ui_metadata.get(id), |item| {
            &item.owner
        })
        .cloned()
    }
    fn checked<'item, T>(
        &self,
        found: Option<&'item T>,
        owner: impl Fn(&T) -> &ModOwner,
    ) -> Result<&'item T, ModError> {
        let item = found.ok_or(ModError::InvalidRegistration)?;
        self.generations.validate(owner(item))?;
        Ok(item)
    }
    fn host(&self, owner: &ModOwner) -> Result<Arc<dyn ModHost>, ModError> {
        self.state
            .hosts
            .get(owner.id.as_str())
            .map(Arc::clone)
            .ok_or(ModError::Unavailable)
    }
}

/// Generation-checking tool handle callable outside the Task 32 executor path.
pub struct ModToolHandle {
    registration: ToolRegistration,
    generations: Arc<ActiveGenerationTable>,
    host: Arc<dyn ModHost>,
}
impl ModToolHandle {
    /// Invokes the tool only while its generation is current.
    #[must_use]
    pub fn call(
        &self,
        tool_call_id: String,
        input: Value,
        cancellation: CancellationToken,
    ) -> RegistrationFuture<'_> {
        Box::pin(async move {
            self.generations.validate(&self.registration.owner)?;
            let params = RpcParams::ToolCall {
                owner: self.registration.owner.clone(),
                name: self.registration.name.as_str().into(),
                tool_call_id,
                input,
            };
            call_value(
                self.host.as_ref(),
                &self.registration.owner,
                RpcMethod::ToolCall,
                params,
                cancellation,
            )
            .await
        })
    }
    /// Borrows the exact owning generation.
    #[must_use]
    pub fn owner(&self) -> &ModOwner {
        &self.registration.owner
    }
}

/// Generation-checking command handle.
pub struct ModCommandHandle {
    registration: CommandRegistration,
    generations: Arc<ActiveGenerationTable>,
    host: Arc<dyn ModHost>,
}
impl ModCommandHandle {
    /// Creates a generation-checking callable command.
    #[must_use]
    pub fn new(
        registration: CommandRegistration,
        generations: Arc<ActiveGenerationTable>,
        host: Arc<dyn ModHost>,
    ) -> Self {
        Self {
            registration,
            generations,
            host,
        }
    }
    /// Invokes the command only while its generation is current.
    #[must_use]
    pub fn call(
        &self,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> RegistrationFuture<'_> {
        Box::pin(async move {
            self.generations.validate(&self.registration.owner)?;
            let params = RpcParams::CommandCall {
                owner: self.registration.owner.clone(),
                name: self.registration.id.as_str().into(),
                arguments,
            };
            call_value(
                self.host.as_ref(),
                &self.registration.owner,
                RpcMethod::CommandCall,
                params,
                cancellation,
            )
            .await
        })
    }
    /// Borrows the exact owning generation.
    #[must_use]
    pub fn owner(&self) -> &ModOwner {
        &self.registration.owner
    }
}

/// Generation-checking Task 44 lifecycle handle.
pub struct ModLifecycleHandle {
    registration: LifecycleRegistration,
    generations: Arc<ActiveGenerationTable>,
    host: Arc<dyn ModHost>,
}
impl ModLifecycleHandle {
    /// Creates a generation-checking Task 44 lifecycle handle.
    #[must_use]
    pub fn new(
        registration: LifecycleRegistration,
        generations: Arc<ActiveGenerationTable>,
        host: Arc<dyn ModHost>,
    ) -> Self {
        Self {
            registration,
            generations,
            host,
        }
    }
    /// Invokes one exact Task 44 event only while current.
    #[must_use]
    pub fn call(&self, payload: Value, cancellation: CancellationToken) -> RegistrationFuture<'_> {
        Box::pin(async move {
            self.generations.validate(&self.registration.owner)?;
            let name =
                serde_json::to_string(&self.registration.event).map_err(|_| ModError::Protocol)?;
            let params = RpcParams::LifecycleCall {
                owner: self.registration.owner.clone(),
                name,
                payload,
            };
            call_value(
                self.host.as_ref(),
                &self.registration.owner,
                RpcMethod::LifecycleCall,
                params,
                cancellation,
            )
            .await
        })
    }
    /// Borrows the exact owning generation.
    #[must_use]
    pub fn owner(&self) -> &ModOwner {
        &self.registration.owner
    }
}

async fn call_value(
    host: &dyn ModHost,
    owner: &ModOwner,
    method: RpcMethod,
    params: RpcParams,
    cancellation: CancellationToken,
) -> Result<Value, ModError> {
    match host.call(owner, method, params, cancellation).await? {
        RpcResult::Value { value } => Ok(value),
        _ => Err(ModError::Protocol),
    }
}

struct MergedMods {
    registrations: ModRegistrationSnapshot,
    hosts: BTreeMap<String, Arc<dyn ModHost>>,
    generations: BTreeMap<String, Generation>,
}

/// Aggregates every retained host's six registration kinds into one immutable candidate.
fn merge(published: &[ModPublication]) -> Result<MergedMods, ModError> {
    let mut registrations = ModRegistrationSnapshot::default();
    let mut hosts = BTreeMap::new();
    let mut generations = BTreeMap::new();
    let mut names = BTreeSet::new();
    let mut total = 0_usize;
    for entry in published {
        let key = entry.owner.id.as_str().to_owned();
        if generations
            .insert(key.clone(), entry.owner.generation)
            .is_some()
        {
            return Err(ModError::InvalidRegistration);
        }
        hosts.insert(key, Arc::clone(&entry.host));
        total = total
            .checked_add(entry.registrations.len())
            .filter(|count| *count <= MOD_REGISTRATIONS_ITEMS_MAX)
            .ok_or(ModError::InvalidRegistration)?;
        let source = entry.registrations.as_ref();
        merge_kind(&mut registrations.tools, &source.tools, &mut names)?;
        merge_kind(&mut registrations.commands, &source.commands, &mut names)?;
        merge_kind(&mut registrations.providers, &source.providers, &mut names)?;
        merge_kind(
            &mut registrations.permissions,
            &source.permissions,
            &mut names,
        )?;
        merge_kind(
            &mut registrations.lifecycle_events,
            &source.lifecycle_events,
            &mut names,
        )?;
        merge_kind(
            &mut registrations.ui_metadata,
            &source.ui_metadata,
            &mut names,
        )?;
    }
    Ok(MergedMods {
        registrations,
        hosts,
        generations,
    })
}

fn merge_kind<T: Clone>(
    target: &mut BTreeMap<RegistrationName, T>,
    source: &BTreeMap<RegistrationName, T>,
    names: &mut BTreeSet<String>,
) -> Result<(), ModError> {
    for (name, value) in source {
        if !names.insert(name.as_str().to_owned()) {
            return Err(ModError::InvalidRegistration);
        }
        target.insert(name.clone(), value.clone());
    }
    Ok(())
}

struct ModToolExecutor {
    owner: ModOwner,
    name: String,
    host: Arc<dyn ModHost>,
    generations: Arc<ActiveGenerationTable>,
}
impl ToolExecutor for ModToolExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        Box::pin(async move {
            self.generations
                .validate(&self.owner)
                .map_err(|_| ExecutorError)?;
            let input = request.input.as_value().clone();
            let tool_call_id = request.tool_call_id.as_str().to_owned();
            let cancellation = request.cancellation.child_token();
            let params = RpcParams::ToolCall {
                owner: self.owner.clone(),
                name: self.name.clone(),
                tool_call_id,
                input,
            };
            let result = self
                .host
                .call(&self.owner, RpcMethod::ToolCall, params, cancellation)
                .await
                .map_err(|_| ExecutorError)?;
            match result {
                RpcResult::Value {
                    value: Value::String(output),
                } => Ok(RawToolOutcome::Success(output)),
                _ => Err(ExecutorError),
            }
        })
    }
}

fn tool_candidate(
    entry: &ModPublication,
    generations: Arc<ActiveGenerationTable>,
) -> Result<Vec<RuntimeToolRegistration>, ModError> {
    let mut registrations = Vec::with_capacity(entry.registrations.tools.len());
    for tool in entry.registrations.tools.values() {
        let definition = tool_definition(tool)?;
        registrations.push(RuntimeToolRegistration {
            definition: Arc::new(definition),
            executor: Arc::new(ModToolExecutor {
                owner: tool.owner.clone(),
                name: tool.name.as_str().into(),
                host: Arc::clone(&entry.host),
                generations: Arc::clone(&generations),
            }),
        });
    }
    Ok(registrations)
}

fn tool_definition(tool: &ToolRegistration) -> Result<ToolDefinition, ModError> {
    let schema = ToolInputSchema::new(
        lotta_domain::BoundedJsonValue::new(tool.input_schema.clone())
            .map_err(|_| ModError::InvalidRegistration)?,
    )
    .map_err(|_| ModError::InvalidRegistration)?;
    let secrets = serde_json::from_value::<SecretRedactionSpec>(
        serde_json::json!({"fields":[],"policy":"redact"}),
    )
    .map_err(|_| ModError::InvalidRegistration)?;
    Ok(ToolDefinition::new(
        InternalToolName::new(tool.name.as_str().into())
            .map_err(|_| ModError::InvalidRegistration)?,
        ModelFacingToolName::new(tool.name.as_str().into())
            .map_err(|_| ModError::InvalidRegistration)?,
        schema,
        ToolDescriptionAsset::new(tool.description.clone())
            .map_err(|_| ModError::InvalidRegistration)?,
        ToolExecutionOwner::ModSidecar,
        ToolApprovalPolicy::Never,
        PermissionAction::new("execute".into()).map_err(|_| ModError::InvalidRegistration)?,
        ToolTimeout::new(MOD_TOOL_TIMEOUT).map_err(|_| ModError::InvalidRegistration)?,
        ToolOutputLimit::new(MOD_TOOL_RESULT_BYTES_MAX, MOD_TOOL_RESULT_MODEL_CHARS_MAX)
            .map_err(|_| ModError::InvalidRegistration)?,
        secrets,
    ))
}

fn map_registry(_: RegistryError) -> ModError {
    ModError::Publication
}
