use std::collections::BTreeMap;

use lotta_domain::{BoundedJsonValue, NonEmptyString, QueueItem, QueueRemovalDisposition, RunId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

fn decode_typed<T: DeserializeOwned>(value: &BoundedJsonValue) -> Result<T, serde_json::Error> {
    serde_json::from_value(value.as_value().clone())
}

/// Device permission vocabulary pinned by protocol v2.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DevicePermissionMode {
    /// Standard approval policy.
    Standard,
    /// Automatically accept file edits.
    AcceptEdits,
    /// Permit unrestricted tool use.
    Unrestricted,
    /// Apply the strictest tool policy.
    Strict,
}

/// Device toolset vocabulary pinned by protocol v2.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolsetName {
    /// Codex names.
    Codex,
    /// Codex snake-case names.
    CodexSnake,
    /// Default tools.
    Default,
    /// Gemini names.
    Gemini,
    /// Gemini snake-case names.
    GeminiSnake,
    /// No tools.
    None,
}

/// Device toolset preference pinned by protocol v2.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolsetPreference {
    /// Select automatically.
    Auto,
    /// Select a concrete toolset.
    #[serde(untagged)]
    Named(ToolsetName),
}

/// Current git context advertised to clients.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitContext {
    /// Current branch, or null when detached.
    pub branch: Option<String>,
    /// Recently committed local branches.
    pub recent_branches: Vec<String>,
}

/// One available skill summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AvailableSkillSummary {
    /// Stable skill identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Skill path.
    pub path: String,
    /// Skill source.
    pub source: String,
}

/// One experiment snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExperimentSnapshot {
    /// Experiment identifier.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Description.
    pub description: String,
    /// Optional environment variable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    /// Effective state.
    pub enabled: bool,
    /// State source.
    pub source: String,
    /// Explicit override.
    #[serde(rename = "override")]
    pub override_value: Option<bool>,
}

/// One background process snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BackgroundProcessSummary {
    /// Process kind.
    pub kind: String,
    /// Kind-specific fields.
    #[serde(flatten)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// Reflection settings included in device status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReflectionSettingsSnapshot {
    /// Agent identifier.
    pub agent_id: String,
    /// Trigger mode.
    pub trigger: String,
    /// Trigger step count.
    pub step_count: u64,
    /// Merge mode.
    pub merge: String,
    /// Merge instructions.
    pub merge_instructions: String,
}

/// One pending approval in a device snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PendingApprovalRequest {
    /// Stable request identifier.
    pub request_id: NonEmptyString,
    /// Typed request body.
    pub request: ApprovalRequest,
}

/// Complete authoritative device status pinned by protocol v2.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeviceStatus {
    /// Current transport connection.
    pub current_connection_id: Option<String>,
    /// Human-readable connection name.
    pub connection_name: Option<String>,
    /// Whether the listener is online.
    pub is_online: bool,
    /// Whether a turn is processing.
    pub is_processing: bool,
    /// Current permission policy.
    pub current_permission_mode: DevicePermissionMode,
    /// Effective working directory.
    pub current_working_directory: Option<String>,
    /// Optional cwd revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd_revision: Option<u64>,
    /// Current git context.
    pub git_context: Option<GitContext>,
    /// Listener version.
    pub letta_code_version: Option<String>,
    /// Loaded concrete toolset.
    pub current_toolset: Option<ToolsetName>,
    /// Requested toolset.
    pub current_toolset_preference: ToolsetPreference,
    /// Loaded tool names.
    pub current_loaded_tools: Vec<String>,
    /// Available skills.
    pub current_available_skills: Vec<AvailableSkillSummary>,
    /// Background processes.
    pub background_processes: Vec<BackgroundProcessSummary>,
    /// Pending approvals.
    pub pending_control_requests: Vec<PendingApprovalRequest>,
    /// Experiment snapshots.
    pub experiments: Vec<ExperimentSnapshot>,
    /// Memory directory.
    pub memory_directory: Option<String>,
    /// Persisted cwd overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd_map: Option<BTreeMap<String, String>>,
    /// Listener boot cwd.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_working_directory: Option<String>,
    /// Whether doctor should run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub should_doctor: Option<bool>,
    /// Reflection settings.
    pub reflection_settings: Option<ReflectionSettingsSnapshot>,
    /// Supported remote commands.
    pub supported_commands: Vec<String>,
}

impl TryFrom<BoundedJsonValue> for DeviceStatus {
    type Error = serde_json::Error;

    fn try_from(value: BoundedJsonValue) -> Result<Self, Self::Error> {
        decode_typed(&value)
    }
}

/// Finite runtime loop status vocabulary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LoopStatus {
    /// Sending a provider request.
    SendingApiRequest,
    /// Waiting for a provider response.
    WaitingForApiResponse,
    /// Retrying a provider request.
    RetryingApiRequest,
    /// Processing a provider response.
    ProcessingApiResponse,
    /// Executing a client-side tool.
    ExecutingClientSideTool,
    /// Executing a command.
    ExecutingCommand,
    /// Waiting for approval.
    WaitingOnApproval,
    /// Waiting for input.
    WaitingOnInput,
}

/// Complete authoritative loop snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LoopState {
    /// Current loop status.
    pub status: LoopStatus,
    /// Active run identifiers.
    pub active_run_ids: Vec<String>,
    /// Client tool calls currently executing.
    pub executing_tool_call_ids: Vec<String>,
}

impl TryFrom<BoundedJsonValue> for LoopState {
    type Error = serde_json::Error;

    fn try_from(value: BoundedJsonValue) -> Result<Self, Self::Error> {
        decode_typed(&value)
    }
}

/// One ordered queue removal transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueueRemovalTransition {
    /// Originating client message identifier.
    pub client_message_id: NonEmptyString,
    /// Exact removal disposition.
    pub disposition: QueueRemovalDisposition,
}

/// One subagent tool call snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubagentToolCall {
    /// Tool call identifier.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Serialized arguments.
    pub args: String,
}

/// Pinned subagent lifecycle vocabulary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    /// Accepted but not started.
    Pending,
    /// Currently running.
    Running,
    /// Completed successfully.
    Completed,
    /// Failed.
    Error,
}

/// Complete authoritative subagent snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SubagentState {
    /// Stable subagent identifier.
    pub subagent_id: String,
    /// Subagent implementation type.
    pub subagent_type: String,
    /// User-visible description.
    pub description: String,
    /// Optional original prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Current lifecycle status.
    pub status: SubagentStatus,
    /// Agent URL, when available.
    pub agent_url: Option<String>,
    /// Child conversation identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Selected model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Whether execution is backgrounded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_background: Option<bool>,
    /// Whether notifications are suppressed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub silent: Option<bool>,
    /// Parent tool call identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Parent agent identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    /// Parent conversation identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_conversation_id: Option<String>,
    /// Start time in epoch milliseconds.
    pub start_time: u64,
    /// Tool calls observed so far.
    pub tool_calls: Vec<SubagentToolCall>,
    /// Total token usage.
    pub total_tokens: u64,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Optional terminal error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TryFrom<BoundedJsonValue> for SubagentState {
    type Error = serde_json::Error;

    fn try_from(value: BoundedJsonValue) -> Result<Self, Self::Error> {
        decode_typed(&value)
    }
}

/// One permission suggestion.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PermissionSuggestion {
    /// Stable suggestion identifier.
    pub id: String,
    /// Display text.
    pub text: String,
}

/// Typed can-use-tool approval request body.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalRequest {
    /// Fixed protocol subtype.
    pub subtype: ApprovalSubtype,
    /// Requested tool.
    pub tool_name: NonEmptyString,
    /// Validated tool input.
    pub input: BoundedJsonValue,
    /// Stable tool call identifier.
    pub tool_call_id: NonEmptyString,
    /// Suggested permission grants.
    pub permission_suggestions: Vec<PermissionSuggestion>,
    /// Path blocked by policy.
    pub blocked_path: Option<String>,
    /// Optional diff previews retained as bounded protocol data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diffs: Option<BoundedJsonValue>,
}

impl TryFrom<BoundedJsonValue> for ApprovalRequest {
    type Error = serde_json::Error;

    fn try_from(value: BoundedJsonValue) -> Result<Self, Self::Error> {
        decode_typed(&value)
    }
}

/// Approval request subtype vocabulary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalSubtype {
    /// Permission request for one tool call.
    CanUseTool,
}

/// Client tool completion vocabulary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientToolStatus {
    /// Tool completed successfully.
    Success,
    /// Tool failed.
    Error,
}

/// Complete client-side tool start lifecycle delta.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientToolStart {
    /// Stable lifecycle message identifier.
    pub id: NonEmptyString,
    /// RFC3339 lifecycle date.
    pub date: NonEmptyString,
    /// Fixed lifecycle message type.
    pub message_type: ClientToolStartType,
    /// Optional run identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Stable tool call identifier.
    pub tool_call_id: NonEmptyString,
    /// Optional tool name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<NonEmptyString>,
    /// Optional serialized tool arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_args: Option<String>,
}

/// Client tool start discriminant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ClientToolStartType {
    /// Client tool start.
    #[serde(rename = "client_tool_start")]
    ClientToolStart,
}

/// Complete client-side tool end lifecycle delta.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientToolEnd {
    /// Stable lifecycle message identifier.
    pub id: NonEmptyString,
    /// RFC3339 lifecycle date.
    pub date: NonEmptyString,
    /// Fixed lifecycle message type.
    pub message_type: ClientToolEndType,
    /// Optional run identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Stable tool call identifier.
    pub tool_call_id: NonEmptyString,
    /// Terminal status.
    pub status: ClientToolStatus,
}

/// Client tool end discriminant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ClientToolEndType {
    /// Client tool end.
    #[serde(rename = "client_tool_end")]
    ClientToolEnd,
}

/// Typed lifecycle deltas plus bounded non-lifecycle stream compatibility.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum StreamDelta {
    /// Client tool began.
    ClientToolStart(ClientToolStart),
    /// Client tool ended.
    ClientToolEnd(ClientToolEnd),
    /// Other pinned stream delta families not changed by Task 73.
    Other(BoundedJsonValue),
}

/// One runtime broadcast payload.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum RuntimeEvent {
    /// Approval control request.
    #[serde(rename = "control_request")]
    ControlRequest {
        /// Stable request identifier.
        request_id: NonEmptyString,
        /// Typed complete approval request.
        request: ApprovalRequest,
        /// Optional baseline compatibility agent identifier.
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<NonEmptyString>,
        /// Optional baseline compatibility conversation identifier.
        #[serde(skip_serializing_if = "Option::is_none")]
        conversation_id: Option<NonEmptyString>,
    },
    /// Explicit terminal state recovered from the durable approval journal.
    #[serde(rename = "approval_recovery")]
    ApprovalRecovery {
        /// Stable request identifier.
        request_id: NonEmptyString,
        /// Provider tool-call identifier.
        tool_call_id: NonEmptyString,
        /// Explicit terminal state.
        state: lotta_runtime::ApprovalState,
        /// Durable state before restart mutation, when one occurred.
        #[serde(skip_serializing_if = "Option::is_none")]
        original_state: Option<lotta_runtime::ApprovalState>,
        /// Current durable revision.
        revision: u64,
    },
    /// Controller-owned external tool execution request.
    #[serde(rename = "controller_tool_request")]
    ControllerToolRequest {
        /// Stable provider call identifier.
        call_id: NonEmptyString,
        /// Captured lifecycle lease generation.
        lease_generation: u64,
        /// Bounded non-secret request body.
        request: BoundedJsonValue,
    },
    /// Context compaction request delegated to a production service.
    #[serde(rename = "compaction_request")]
    CompactionRequest {
        /// Captured lifecycle lease generation.
        lease_generation: u64,
        /// Estimated model-visible tokens before compaction.
        tokens_before: u64,
        /// Model-visible messages before compaction.
        messages_before: usize,
        /// Stable compaction reason.
        reason: NonEmptyString,
    },
    /// Authoritative device status snapshot.
    #[serde(rename = "update_device_status")]
    UpdateDeviceStatus {
        /// Complete device status.
        device_status: Box<DeviceStatus>,
    },
    /// Authoritative loop status snapshot.
    #[serde(rename = "update_loop_status")]
    UpdateLoopStatus {
        /// Complete loop status.
        loop_status: LoopState,
    },
    /// Authoritative queue snapshot and ordered removals.
    #[serde(rename = "update_queue")]
    UpdateQueue {
        /// Complete queue snapshot.
        queue: Vec<QueueItem>,
        /// Ordered queue-removal transitions.
        removed: Vec<QueueRemovalTransition>,
    },
    /// One bounded stream delta.
    #[serde(rename = "stream_delta")]
    StreamDelta {
        /// Typed lifecycle or compatible stream delta.
        delta: StreamDelta,
        /// Optional originating subagent.
        #[serde(skip_serializing_if = "Option::is_none")]
        subagent_id: Option<NonEmptyString>,
    },
    /// Exactly-once terminal turn event.
    #[serde(rename = "turn_finished")]
    TurnFinished {
        /// Turn identifier.
        turn_id: NonEmptyString,
        /// Optional run identifier.
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        /// Exact bounded stop-reason discriminant.
        stop_reason: NonEmptyString,
        /// Optional scrubbed error detail.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<NonEmptyString>,
    },
    /// Authoritative subagent snapshot.
    #[serde(rename = "update_subagent_state")]
    UpdateSubagentState {
        /// Complete subagent list.
        subagents: Vec<SubagentState>,
    },
}

impl RuntimeEvent {
    /// Returns the exact wire discriminant.
    #[must_use]
    pub const fn discriminant(&self) -> &'static str {
        match self {
            Self::ControlRequest { .. } => "control_request",
            Self::ApprovalRecovery { .. } => "approval_recovery",
            Self::ControllerToolRequest { .. } => "controller_tool_request",
            Self::CompactionRequest { .. } => "compaction_request",
            Self::UpdateDeviceStatus { .. } => "update_device_status",
            Self::UpdateLoopStatus { .. } => "update_loop_status",
            Self::UpdateQueue { .. } => "update_queue",
            Self::StreamDelta { .. } => "stream_delta",
            Self::TurnFinished { .. } => "turn_finished",
            Self::UpdateSubagentState { .. } => "update_subagent_state",
        }
    }
}
