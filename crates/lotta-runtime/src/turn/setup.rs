//! Canonical ten-stage turn setup and durable admission boundary.

#![allow(
    clippy::missing_errors_doc,
    reason = "object-safe facade methods share the trait-level typed failure contract"
)]

use super::TurnToolCatalog;
use crate::RuntimeError;
use crate::ports::{AgentStore, ConversationStore, ProviderRequest};
use lotta_domain::{Agent, AgentId, Conversation, ConversationId, PermissionMode};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Exact canonical setup order from `03-runtime-and-turns.md`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupStage {
    /// Resolve and authorize durable records.
    ResolveAgentAndConversation,
    /// Resolve a canonical confined cwd.
    ResolveCwd,
    /// Apply sandbox and permission scope.
    ApplyWorkspaceSandboxAndPermissions,
    /// Synchronize or initialize `MemFS`.
    SynchronizeOrInitializeMemFs,
    /// Compile the managed system prompt.
    CompileSystemPrompt,
    /// Resolve model, provider, context, and toolset.
    ResolveModelProviderContextAndToolset,
    /// Discover exact selected skill sources.
    DiscoverSelectedSkills,
    /// Load mods and hooks in canonical precedence.
    LoadModsAndHooks,
    /// Merge and filter all tool sources.
    MergeTools,
    /// Build the request, admit input, and emit loop status.
    BuildProviderRequestAndEmitStatus,
}

impl SetupStage {
    /// Canonical exact ten-stage sequence.
    pub const ORDER: [Self; 10] = [
        Self::ResolveAgentAndConversation,
        Self::ResolveCwd,
        Self::ApplyWorkspaceSandboxAndPermissions,
        Self::SynchronizeOrInitializeMemFs,
        Self::CompileSystemPrompt,
        Self::ResolveModelProviderContextAndToolset,
        Self::DiscoverSelectedSkills,
        Self::LoadModsAndHooks,
        Self::MergeTools,
        Self::BuildProviderRequestAndEmitStatus,
    ];
}

/// Loop status emitted at the admission boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupStatus {
    /// Durable request is about to be sent.
    Sending,
    /// Runtime is waiting for provider events.
    Waiting,
}

/// Stable source identities merged at stage nine.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SetupToolSource {
    /// Native built-in registry.
    BuiltIn,
    /// Model Context Protocol.
    Mcp,
    /// Compatibility mod.
    Mod,
    /// Channel gateway.
    Channel,
    /// Controller-owned external tool.
    ControllerExternal,
}

/// One setup tool candidate retaining source and owner policy.
#[derive(Clone, Debug)]
pub struct ToolCandidate {
    /// Stable source.
    pub source: SetupToolSource,
    /// Canonical runtime definition.
    pub definition: crate::ports::ToolDefinition,
    /// Model-facing identity selected by the complete Task 32 registration.
    pub model_name: String,
    /// Whether permission policy authorizes exposure.
    pub authorized: bool,
}

/// Cwd classification preserving pinned fallback distinctions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CwdResolution {
    /// Requested directory was canonical and confined.
    Requested(PathBuf),
    /// Requested directory disappeared and the canonical fallback is used.
    DeletedFallback {
        /// Original persisted path retained for the one-use reminder key.
        original: PathBuf,
        /// Canonical pinned fallback/project root.
        fallback: PathBuf,
    },
}

impl CwdResolution {
    /// Effective canonical cwd.
    #[must_use]
    pub fn effective(&self) -> &Path {
        match self {
            Self::Requested(path) => path,
            Self::DeletedFallback { fallback, .. } => fallback,
        }
    }
}

/// Typed cwd failures that must not be converted into deletion fallback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CwdFailure {
    /// A symlink or canonical target escapes confinement.
    SymlinkOrEscape,
    /// OS access was denied.
    PermissionDenied,
    /// Requested object exists but is not a directory.
    NotDirectory,
    /// Fallback itself is unavailable or malformed.
    InvalidFallback,
}

/// Prompt-visible skill inventory produced before exact selection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SkillInventory {
    /// Available stable IDs in deterministic order.
    pub available: Vec<String>,
    /// Selected stable IDs in deterministic order.
    pub selected: Vec<String>,
}

/// Model/provider/context/toolset selection returned by production configuration.
#[derive(Clone, Debug)]
pub struct ResolvedTurnModel {
    /// Canonical provider model descriptor.
    pub model: lotta_domain::ModelDescriptor,
    /// Positive effective context window.
    pub context_window: u64,
    /// Positive output token limit.
    pub output_tokens: u64,
    /// Configured toolset identity understood by the tool registry adapter.
    pub toolset: String,
    /// Optional post-merge allowlist.
    pub allowlist: Option<Vec<String>>,
    /// Deterministically ordered, available request-local fallback candidates.
    pub fallback_candidates: Vec<lotta_domain::ModelDescriptor>,
}

/// Opaque extension snapshot identity captured by the production composition port.
///
/// Canonical mod and hook snapshot handles remain owned by the production adapter so the runtime
/// crate does not depend on the extension crate.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtensionSnapshot {
    id: u64,
}
impl ExtensionSnapshot {
    /// Creates an opaque production snapshot identity.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self { id }
    }
    /// Returns the opaque production snapshot identity.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }
}

/// Durable input admission receipt used for linked recovery outcomes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmissionReceipt {
    /// Stable durable input identity.
    pub input_id: String,
    /// Whether the durable append completed.
    pub appended: bool,
}

/// Opaque durable reminder claim plus its separately generated prompt text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReminderClaim {
    token: String,
    input_id: String,
    /// Human-readable runtime reminder. This never contains claim metadata.
    pub message: String,
}

impl ReminderClaim {
    /// Constructs an opaque claim owned by a setup adapter.
    #[must_use]
    pub fn new(token: String, input_id: String, message: String) -> Self {
        Self {
            token,
            input_id,
            message,
        }
    }

    /// Returns the predetermined durable input identity.
    #[must_use]
    pub fn input_id(&self) -> &str {
        &self.input_id
    }

    /// Returns the opaque adapter token; never include it in prompt text.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Setup input retained before any allocating setup stage.
pub trait SetupStatusSink: Send + Sync {
    /// Emits one status for this exact setup call.
    fn emit(&self, status: SetupStatus) -> Result<(), SetupError>;
}

/// Opaque canonical scope identity retained for one prepared turn.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SetupScopeHandle(u64);

impl SetupScopeHandle {
    /// Creates a handle from a production-owned unique identifier.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
    /// Returns the opaque identifier.
    #[must_use]
    pub const fn id(self) -> u64 {
        self.0
    }
}

/// Setup input retained before any allocating setup stage.
pub struct SetupInput {
    /// Requested agent scope.
    pub agent_id: AgentId,
    /// Requested conversation scope.
    pub conversation_id: ConversationId,
    /// Requested persisted cwd.
    pub cwd: PathBuf,
    /// Exact fallback/project root.
    pub fallback_cwd: PathBuf,
    /// Raw user input admitted only at stage ten.
    pub user_input: String,
    /// Exact selected skill IDs.
    pub selected_skills: Vec<String>,
    /// Configured permission mode.
    pub permission_mode: PermissionMode,
    /// Cooperative cancellation shared by every stage.
    pub cancellation: CancellationToken,
    /// Whole-setup deadline.
    pub deadline: Duration,
    /// Per-call status destination retained through dispatch.
    pub status_sink: std::sync::Arc<dyn SetupStatusSink>,
}

/// Production setup boundaries. Implementations adapt actual store, `MemFS`, extension, provider,
/// sandbox, and tool registry modules; the orchestrator contains no synthetic subsystem state.
pub trait SetupPorts: Send + Sync {
    /// Agent records.
    fn agents(&self) -> &dyn AgentStore;
    /// Conversation records.
    fn conversations(&self) -> &dyn ConversationStore;
    /// Resolves a conversation globally so ownership can be validated before scoped access.
    fn resolve_conversation(
        &self,
        requested_agent: &AgentId,
        conversation: &ConversationId,
        _cancellation: &CancellationToken,
    ) -> crate::ports::PortFuture<'_, Conversation> {
        self.conversations().load(requested_agent, conversation)
    }
    /// Resolves canonical cwd with typed distinctions.
    fn resolve_cwd(&self, requested: &Path, fallback: &Path) -> Result<CwdResolution, SetupError>;
    /// Applies the real permission and workspace sandbox engines.
    fn apply_scope(
        &self,
        cwd: &Path,
        mode: PermissionMode,
        cancellation: &CancellationToken,
    ) -> crate::ports::PortFuture<'_, SetupScopeHandle>;
    /// Synchronizes or initializes the real agent `MemFS` repository.
    fn prepare_memfs(
        &self,
        agent: &Agent,
        cancellation: &CancellationToken,
    ) -> crate::ports::PortFuture<'_, ()>;
    /// Discovers available skills for prompt inventory.
    fn skill_inventory(
        &self,
        agent: &Agent,
        cwd: &Path,
        selected: &[String],
    ) -> Result<SkillInventory, SetupError>;
    /// Compiles managed prompt, current memory, inventory, and reminders through the prompt cache.
    fn compile_prompt(
        &self,
        agent: &Agent,
        conversation: &Conversation,
        inventory: &SkillInventory,
        scope: SetupScopeHandle,
        reminder: Option<&str>,
        cancellation: &CancellationToken,
    ) -> crate::ports::PortFuture<'_, String>;
    /// Resolves persisted model precedence, connection availability, context, and toolset.
    fn resolve_model(
        &self,
        agent: &Agent,
        conversation: &Conversation,
    ) -> Result<ResolvedTurnModel, SetupError>;
    /// Resolves exact selected sources from the same inventory without duplicates.
    fn discover_selected(
        &self,
        inventory: &SkillInventory,
        selected: &[String],
    ) -> Result<Vec<String>, SetupError>;
    /// Loads agent/global/project mods and hooks with owner and limits enforced.
    fn load_extensions(
        &self,
        agent: &Agent,
        cwd: &Path,
        cancellation: &CancellationToken,
    ) -> crate::ports::PortFuture<'_, ExtensionSnapshot>;
    /// Collects five real tool-source candidates.
    fn tool_candidates(
        &self,
        scope: SetupScopeHandle,
        extensions: &ExtensionSnapshot,
        cancellation: &CancellationToken,
    ) -> Result<Vec<ToolCandidate>, SetupError>;
    /// Composes the actual Task32 registry after permission and allowlist filtering.
    fn merge_tools(
        &self,
        model: &ResolvedTurnModel,
        candidates: Vec<ToolCandidate>,
    ) -> Result<TurnToolCatalog, SetupError>;
    /// Builds a bounded Task46/53 provider request without durable transcript mutation.
    #[allow(clippy::too_many_arguments, reason = "explicit scoped setup boundary")]
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
    ) -> crate::ports::PortFuture<'_, ProviderRequest>;
    /// Atomically appends the user input without consuming a claimed reminder.
    fn admit_input(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        input: &str,
        reminder_claim: Option<&ReminderClaim>,
    ) -> crate::ports::PortFuture<'_, AdmissionReceipt>;
    /// Commits a claimed reminder after durable input admission.
    fn commit_cwd_reminder(
        &self,
        claim: &ReminderClaim,
        receipt: &AdmissionReceipt,
        cancellation: &CancellationToken,
    ) -> crate::ports::PortFuture<'_, ()>;
    /// Claims a durable one-use reminder without consuming it.
    fn claim_cwd_reminder(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        original: &Path,
        scope: SetupScopeHandle,
        cancellation: &CancellationToken,
    ) -> crate::ports::PortFuture<'_, Option<ReminderClaim>>;
    /// Releases an unconsumed reminder claim after a pre-admission failure.
    fn release_cwd_reminder(
        &self,
        claim: &ReminderClaim,
        cancellation: &CancellationToken,
    ) -> crate::ports::PortFuture<'_, ()>;
    /// Records exactly one linked post-admission interruption/error idempotently.
    fn record_interrupted(
        &self,
        receipt: &AdmissionReceipt,
        failure: &SetupError,
    ) -> crate::ports::PortFuture<'_, ()>;
    /// Releases one exact stage-three scope snapshot.
    fn release_scope(&self, scope: SetupScopeHandle);
}

/// Successful complete setup.
pub struct SetupOutput {
    /// Validated provider request.
    pub request: ProviderRequest,
    /// Exact resolved model inputs retained for request refresh.
    pub model: ResolvedTurnModel,
    /// Deterministically ordered available fallback models retained for execution.
    pub fallback_candidates: Vec<lotta_domain::ModelDescriptor>,
    /// Exact compiled prompt retained for request refresh.
    pub prompt: String,
    /// Exact submitted user input retained for request refresh.
    pub input: String,
    /// Turn-local validated tool catalog.
    pub tools: TurnToolCatalog,
    /// Exact log appended only after each completed stage.
    pub stages: Vec<SetupStage>,
    /// Linked durable input identity.
    pub admission: AdmissionReceipt,
    /// Exact immutable extensions captured at stage eight for this turn.
    pub extensions: ExtensionSnapshot,
    /// Exact immutable production scope captured at stage three.
    pub scope: SetupScopeHandle,
    /// Per-call status destination for provider dispatch.
    pub status_sink: std::sync::Arc<dyn SetupStatusSink>,
    /// Loop status that must be emitted immediately before provider dispatch.
    pub status: SetupStatus,
}

/// Whether a failure happened before or after durable user-input admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetupFailure {
    /// Transcript is unchanged.
    PreAdmission(SetupError),
    /// Input exists and exactly one linked recovery outcome was recorded.
    PostAdmission {
        /// Durable input identity.
        receipt: AdmissionReceipt,
        /// Typed terminal failure.
        error: SetupError,
    },
}

/// Stable typed setup errors.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SetupError {
    /// Cross-agent conversation access.
    #[error("cross-agent conversation access")]
    CrossAgent,
    /// Archived or otherwise invalid conversation.
    #[error("conversation is archived or invalid")]
    ArchivedOrInvalid,
    /// Typed cwd failure.
    #[error("cwd resolution failed: {0:?}")]
    Cwd(CwdFailure),
    /// Required model/provider/toolset is unavailable.
    #[error("model or provider unavailable: {0}")]
    Availability(String),
    /// Skill selection is missing or duplicated.
    #[error("invalid skill selection")]
    SkillSelection,
    /// Tool conflict or unauthorized exposure.
    #[error("tool merge failed: {0}")]
    ToolMerge(String),
    /// Cooperative cancellation.
    #[error("turn setup cancelled")]
    Cancelled,
    /// Lifecycle hook blocked the operation.
    #[error("lifecycle hook blocked operation")]
    HookBlocked,
    /// Stable owner/id/code-attributed lifecycle hook failure.
    #[error("lifecycle hook failed: {owner}/{hook_id}/{code}")]
    HookFailure {
        /// Failing registration owner.
        owner: String,
        /// Failing hook identifier.
        hook_id: String,
        /// Stable scrubbed machine code.
        code: &'static str,
    },
    /// Whole setup deadline elapsed.
    #[error("turn setup deadline elapsed")]
    Deadline,
    /// Adapter boundary failure.
    #[error("turn setup adapter failed: {0}")]
    Adapter(String),
}

impl From<RuntimeError> for SetupError {
    fn from(value: RuntimeError) -> Self {
        match value {
            RuntimeError::Cancelled { .. } => Self::Cancelled,
            RuntimeError::Timeout { .. } => Self::Deadline,
            other => Self::Adapter(other.to_string()),
        }
    }
}

/// Public production orchestrator over concrete facade ports.
pub struct SetupOrchestrator<'a> {
    pub(super) ports: &'a dyn SetupPorts,
    pub(super) deadline: tokio::sync::Mutex<Option<Instant>>,
}

impl<'a> SetupOrchestrator<'a> {
    /// Creates an orchestrator over production adapters.
    #[must_use]
    pub const fn new(ports: &'a dyn SetupPorts) -> Self {
        Self {
            ports,
            deadline: tokio::sync::Mutex::const_new(None),
        }
    }

    /// Executes all ten stages and the exact durable admission boundary.
    ///
    /// # Errors
    /// Returns a typed pre-admission failure with no transcript mutation, or a linked
    /// post-admission failure after recording an idempotent interruption outcome.
    pub async fn prepare(&self, input: SetupInput) -> Result<SetupOutput, SetupFailure> {
        let end = Instant::now()
            .checked_add(input.deadline)
            .ok_or_else(|| SetupFailure::pre(SetupError::Deadline))?;
        *self.deadline.lock().await = Some(end);
        let result = self.prepare_inner(input).await;
        *self.deadline.lock().await = None;
        result
    }

    pub(super) fn check_deadline(&self) -> Result<(), SetupFailure> {
        let expired = self.deadline.try_lock().map_or(true, |deadline| {
            deadline.is_some_and(|end| Instant::now() >= end)
        });
        if expired {
            Err(SetupFailure::pre(SetupError::Deadline))
        } else {
            Ok(())
        }
    }
}

impl SetupFailure {
    pub(crate) fn pre(error: SetupError) -> Self {
        Self::PreAdmission(error)
    }
}
