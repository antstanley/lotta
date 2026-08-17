//! Strict pinned request types and launch-context planning.

use lotta_domain::{AgentId, ConversationId};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

/// Maximum UTF-8 bytes in a task description.
pub const SUBAGENT_DESCRIPTION_BYTES_MAX: usize = 1_024;
/// Maximum UTF-8 bytes in a task prompt.
pub const SUBAGENT_PROMPT_BYTES_MAX: usize = 256 * 1_024;
/// Maximum requested agentic turns.
pub const SUBAGENT_TURNS_MAX: u32 = 1_000;
/// Maximum explicitly requested tool names.
pub const SUBAGENT_TOOLS_MAX: usize = 128;
/// Maximum UTF-8 bytes in one tool or model identifier.
pub const SUBAGENT_NAME_BYTES_MAX: usize = 256;
/// Maximum serialized inherited context.
pub const SUBAGENT_CONTEXT_BYTES_MAX: usize = 4 * 1_024 * 1_024;

/// Seven exact built-in subagent profiles from Letta Code 0.30.20.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentType {
    /// Isolated full-capability coding agent.
    GeneralPurpose,
    /// Parent-conversation fork with inherited tools.
    Fork,
    /// Read-only parent-history investigator.
    Recall,
    /// Memory worktree editor merged after success.
    Reflection,
    /// Parent-memory organizer.
    Memory,
    /// Historical trajectory analyzer.
    HistoryAnalyzer,
    /// Fast memory initializer.
    Init,
}

/// Model-selection behavior accepted at the compatibility boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "policy",
    content = "model",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ModelPolicy {
    /// Inherit the exact resolved parent model.
    Inherit(String),
    /// Use pinned automatic selection.
    Auto,
    /// Use pinned fast automatic selection.
    AutoFast,
    /// Use one explicit model handle.
    Explicit(String),
}

/// Explicit bounded tools or the pinned unrestricted set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolPolicy {
    /// All parent-available tools.
    All,
    /// Exact tool allowlist.
    Only(Vec<String>),
}

/// Parent runtime and conversation capability identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParentScope {
    /// Parent agent.
    pub agent_id: AgentId,
    /// Parent conversation.
    pub conversation_id: ConversationId,
    /// Parent runtime incarnation.
    pub runtime_id: String,
}

/// Explicit filesystem roots supplied by the parent runtime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryScope {
    /// Canonical primary memory root, retained for rejection and merge only.
    pub primary_root: PathBuf,
    /// Canonical roots the selected profile may read.
    pub readonly_roots: Vec<PathBuf>,
    /// Canonical roots the selected profile may write.
    pub writable_roots: Vec<PathBuf>,
}

/// Strict public subagent request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentRequest {
    /// Built-in profile.
    #[serde(rename = "type")]
    pub subagent_type: SubagentType,
    /// Short user-facing task description.
    pub description: String,
    /// Complete task prompt.
    pub prompt: String,
    /// Bounded context resolved by the manager before process launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_context: Option<String>,
    /// Model policy.
    pub model: ModelPolicy,
    /// Return a task ID immediately when true.
    pub background: bool,
    /// Suppress incremental stream deltas while retaining terminal state and result.
    #[serde(default)]
    pub silent: bool,
    /// Explicit filesystem roots exposed to the child sandbox.
    #[serde(default)]
    pub filesystem_roots: Vec<PathBuf>,
    /// Host-created reflection worktree; never accepts or exposes the primary root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflection_worktree: Option<PathBuf>,
    /// Optional positive bounded turn limit.
    pub max_turns: Option<u32>,
    /// Requested tool policy.
    pub tools: ToolPolicy,
    /// Explicit memory roots.
    pub memory_scope: Option<MemoryScope>,
    /// Immutable parent capability scope.
    pub parent_scope: ParentScope,
    /// Existing agent deployment, supported only by general-purpose.
    pub existing_agent_id: Option<AgentId>,
    /// Existing conversation deployment, supported only by general-purpose.
    pub existing_conversation_id: Option<ConversationId>,
}

/// Bounded serialized parent context for fork execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForkContext {
    /// Exact serialized parent conversation.
    pub conversation_json: Vec<u8>,
    /// Exact inherited parent scope.
    pub parent_scope: ParentScope,
}

/// Production context/capability plan resolved before process launch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextPlan {
    /// New isolated agent and conversation.
    Isolated,
    /// Serialized parent fork with inherited scope.
    Fork(ForkContext),
    /// Read-only historical-message capability for the exact parent.
    Recall(ParentScope),
    /// Isolated memory worktree; primary is never exposed.
    Reflection,
    /// Exact parent-memory profile.
    ParentMemory,
    /// Existing exact agent/conversation deployment.
    Existing {
        /// Existing agent.
        agent_id: Option<AgentId>,
        /// Existing conversation.
        conversation_id: Option<ConversationId>,
    },
}

/// Stable validation failure without user-controlled contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RequestError {
    /// A scalar or collection violates its bound.
    #[error("invalid subagent request bound")]
    Bound,
    /// Requested fields conflict with the pinned profile.
    #[error("invalid subagent request combination")]
    Combination,
    /// A required capability was absent.
    #[error("missing subagent capability")]
    Capability,
}

/// Injected manager capability resolving parent context before process launch.
pub trait ContextResolver: Send + Sync {
    /// Resolves exact bounded context for one validated request.
    fn resolve<'a>(
        &'a self,
        request: &'a SubagentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, RequestError>> + Send + 'a>>;
}

/// Resolver that enforces isolated, fork, recall, and parent-memory context policy.
pub struct ProductionContextResolver<H> {
    history: H,
    parent_transcript: Vec<u8>,
    parent_memory: Vec<u8>,
}

impl<H> ProductionContextResolver<H> {
    /// Creates a resolver from already-bounded parent transcript and memory snapshots.
    pub fn new(history: H, transcript: Vec<u8>, memory: Vec<u8>) -> Result<Self, RequestError> {
        validate_context_bytes(&transcript)?;
        validate_context_bytes(&memory)?;
        Ok(Self {
            history,
            parent_transcript: transcript,
            parent_memory: memory,
        })
    }
}

impl<H: super::confinement::HistoricalMessagePort> ContextResolver
    for ProductionContextResolver<H>
{
    fn resolve<'a>(
        &'a self,
        request: &'a SubagentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, RequestError>> + Send + 'a>> {
        Box::pin(async move {
            let bytes = match request.subagent_type {
                SubagentType::GeneralPurpose => return Ok(None),
                SubagentType::Fork => self.parent_transcript.clone(),
                SubagentType::Recall => {
                    self.history
                        .read_history(
                            &request.parent_scope.agent_id,
                            &request.parent_scope.conversation_id,
                        )
                        .await?
                }
                SubagentType::Reflection
                | SubagentType::Memory
                | SubagentType::HistoryAnalyzer
                | SubagentType::Init => self.parent_memory.clone(),
            };
            validate_context_bytes(&bytes)?;
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| RequestError::Bound)
        })
    }
}

impl SubagentRequest {
    /// Validates scalar bounds and all pinned cross-field constraints.
    pub fn validate(&self) -> Result<(), RequestError> {
        validate_text(&self.description, SUBAGENT_DESCRIPTION_BYTES_MAX)?;
        validate_text(&self.prompt, SUBAGENT_PROMPT_BYTES_MAX)?;
        if let Some(context) = &self.resolved_context {
            validate_multiline(context, SUBAGENT_CONTEXT_BYTES_MAX)?;
        }
        validate_text(&self.parent_scope.runtime_id, SUBAGENT_NAME_BYTES_MAX)?;
        if self
            .max_turns
            .is_some_and(|turns| turns == 0 || turns > SUBAGENT_TURNS_MAX)
        {
            return Err(RequestError::Bound);
        }
        validate_tools(&self.tools)?;
        validate_model(&self.model)?;
        if self.filesystem_roots.len() > SUBAGENT_TOOLS_MAX {
            return Err(RequestError::Bound);
        }
        if self.subagent_type != SubagentType::Reflection && self.reflection_worktree.is_some() {
            return Err(RequestError::Combination);
        }
        validate_cross_fields(self)
    }

    /// Resolves context behavior with injected parent bytes and capabilities.
    pub fn context_plan(&self, parent_json: Option<Vec<u8>>) -> Result<ContextPlan, RequestError> {
        self.validate()?;
        if self.existing_agent_id.is_some() || self.existing_conversation_id.is_some() {
            return Ok(ContextPlan::Existing {
                agent_id: self.existing_agent_id.clone(),
                conversation_id: self.existing_conversation_id.clone(),
            });
        }
        match self.subagent_type {
            SubagentType::GeneralPurpose => Ok(ContextPlan::Isolated),
            SubagentType::Fork => Ok(ContextPlan::Fork(fork_context(self, parent_json)?)),
            SubagentType::Recall => Ok(ContextPlan::Recall(self.parent_scope.clone())),
            SubagentType::Reflection => Ok(ContextPlan::Reflection),
            SubagentType::Memory | SubagentType::HistoryAnalyzer | SubagentType::Init => {
                Ok(ContextPlan::ParentMemory)
            }
        }
    }
}

fn validate_context_bytes(bytes: &[u8]) -> Result<(), RequestError> {
    if bytes.is_empty() || bytes.len() > SUBAGENT_CONTEXT_BYTES_MAX || bytes.contains(&0) {
        Err(RequestError::Bound)
    } else {
        Ok(())
    }
}

fn fork_context(
    request: &SubagentRequest,
    parent_json: Option<Vec<u8>>,
) -> Result<ForkContext, RequestError> {
    let conversation_json = parent_json.ok_or(RequestError::Capability)?;
    if conversation_json.is_empty() || conversation_json.len() > SUBAGENT_CONTEXT_BYTES_MAX {
        return Err(RequestError::Bound);
    }
    Ok(ForkContext {
        conversation_json,
        parent_scope: request.parent_scope.clone(),
    })
}

fn validate_cross_fields(request: &SubagentRequest) -> Result<(), RequestError> {
    let existing =
        request.existing_agent_id.is_some() || request.existing_conversation_id.is_some();
    if existing && request.subagent_type != SubagentType::GeneralPurpose {
        return Err(RequestError::Combination);
    }
    let memory_profile = matches!(
        request.subagent_type,
        SubagentType::Reflection
            | SubagentType::Memory
            | SubagentType::HistoryAnalyzer
            | SubagentType::Init
    );
    if memory_profile != request.memory_scope.is_some() {
        return Err(RequestError::Combination);
    }
    if request.subagent_type == SubagentType::Recall && request.tools != recall_tools() {
        return Err(RequestError::Combination);
    }
    if request.subagent_type == SubagentType::Reflection && request.tools != reflection_tools() {
        return Err(RequestError::Combination);
    }
    Ok(())
}

fn validate_tools(tools: &ToolPolicy) -> Result<(), RequestError> {
    let ToolPolicy::Only(names) = tools else {
        return Ok(());
    };
    if names.is_empty() || names.len() > SUBAGENT_TOOLS_MAX {
        return Err(RequestError::Bound);
    }
    for name in names {
        validate_text(name, SUBAGENT_NAME_BYTES_MAX)?;
    }
    Ok(())
}

fn validate_model(model: &ModelPolicy) -> Result<(), RequestError> {
    if let ModelPolicy::Explicit(value) | ModelPolicy::Inherit(value) = model {
        validate_text(value, SUBAGENT_NAME_BYTES_MAX)?;
    }
    Ok(())
}

fn validate_multiline(value: &str, bytes_max: usize) -> Result<(), RequestError> {
    if value.is_empty() || value.len() > bytes_max || value.contains('\0') {
        Err(RequestError::Bound)
    } else {
        Ok(())
    }
}

fn validate_text(value: &str, bytes_max: usize) -> Result<(), RequestError> {
    if value.is_empty() || value.len() > bytes_max || value.contains(['\0', '\n', '\r']) {
        Err(RequestError::Bound)
    } else {
        Ok(())
    }
}

/// Exact pinned recall tool policy.
#[must_use]
pub fn recall_tools() -> ToolPolicy {
    ToolPolicy::Only(vec!["Bash".into(), "Read".into(), "TaskOutput".into()])
}

/// Exact pinned reflection tool policy.
#[must_use]
pub fn reflection_tools() -> ToolPolicy {
    ToolPolicy::Only(vec!["Bash".into(), "Edit".into()])
}
