use lotta_domain::{BoundedJsonValue, BoundedVec, NonEmptyString, RunId, RuntimeScope};
use serde::Deserialize;
use serde_json::Value;

use crate::{errors::ProtocolErrorEnvelope, framing::DecodedFrame};

const SOURCE_TAGS_MAX: usize = 64;
const EXTERNAL_TOOLS_MAX: usize = 256;

/// Options used when runtime start creates an agent.
#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeStartCreateAgentOptions {
    /// Agent creation body.
    pub body: BoundedJsonValue,
    /// Optionally pin the agent globally.
    pub pin_global: Option<bool>,
    /// Optionally enable memory filesystem support.
    pub memfs: Option<bool>,
}

/// Options used when runtime start creates a conversation.
#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeStartCreateConversationOptions {
    /// Optional conversation creation body.
    pub body: Option<BoundedJsonValue>,
}

/// Workspace sandbox roots.
#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeStartWorkspaceSandbox {
    /// Workspace root.
    pub root: NonEmptyString,
    /// Isolation root.
    pub isolation_root: NonEmptyString,
}

/// Client metadata supplied during runtime start.
#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeStartClientInfo {
    /// Stable client name.
    pub name: NonEmptyString,
    /// Human-readable title.
    pub title: Option<NonEmptyString>,
    /// Client version.
    pub version: Option<NonEmptyString>,
}

/// Exact supported skill source names.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Bundled skills.
    Bundled,
    /// Global skills.
    Global,
    /// Agent skills.
    Agent,
    /// Project skills.
    Project,
}

/// Bounded external-tool array.
#[derive(Clone, Debug)]
pub struct RuntimeStartExternalTools(pub BoundedVec<BoundedJsonValue, EXTERNAL_TOOLS_MAX>);

impl<'de> Deserialize<'de> for RuntimeStartExternalTools {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = Vec::<Value>::deserialize(deserializer)?;
        let values = values
            .into_iter()
            .map(BoundedJsonValue::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(serde::de::Error::custom)?;
        BoundedVec::new(values)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// The five concrete Runtime-group commands.
#[derive(Clone, Debug)]
pub enum RuntimeCommand {
    /// Resolve or create a runtime.
    RuntimeStart(Box<RuntimeStartCommand>),
    /// Admit client input.
    Input(InputCommand),
    /// Replay authoritative runtime state.
    Sync(SyncCommand),
    /// Cancel a run or active runtime work.
    AbortMessage(AbortMessageCommand),
    /// Change device state for a runtime.
    ChangeDeviceState(ChangeDeviceStateCommand),
}

/// Existing-or-create runtime resolution request.
#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeStartCommand {
    /// Correlation identifier.
    pub request_id: NonEmptyString,
    /// Existing agent identifier.
    pub agent_id: Option<NonEmptyString>,
    /// Bounded new-agent options.
    pub create_agent: Option<RuntimeStartCreateAgentOptions>,
    /// Existing conversation identifier.
    pub conversation_id: Option<NonEmptyString>,
    /// Bounded new-conversation options.
    pub create_conversation: Option<RuntimeStartCreateConversationOptions>,
    /// Initial working directory; null resets it.
    pub cwd: Option<String>,
    /// Initial permission mode.
    pub mode: Option<RuntimeMode>,
    /// Bounded source-tag extension.
    pub conversation_source_tags: Option<BoundedVec<NonEmptyString, SOURCE_TAGS_MAX>>,
    /// Bounded workspace sandbox extension.
    pub workspace_sandbox: Option<RuntimeStartWorkspaceSandbox>,
    /// Bounded skill-source extension.
    pub skill_sources: Option<BoundedVec<SkillSource, 4>>,
    /// Whether omitted skill sources preserve an override.
    pub preserve_skill_sources: Option<bool>,
    /// Bounded client metadata extension.
    pub client_info: Option<RuntimeStartClientInfo>,
    /// Whether stale approvals should be recovered.
    #[serde(default = "recover_approvals_default")]
    pub recover_approvals: bool,
    /// Whether initial device status must be forced.
    pub force_device_status: Option<bool>,
    /// Whether resolution waits for replay.
    pub wait_for_replay: Option<bool>,
    /// Bounded external-tool registrations.
    pub external_tools: Option<RuntimeStartExternalTools>,
}

/// Baseline runtime permission mode.
#[derive(Clone, Copy, Debug, Deserialize)]
pub enum RuntimeMode {
    /// Prompt for protected actions.
    #[serde(rename = "standard")]
    Standard,
    /// Automatically accept edits.
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    /// Permit unrestricted operations.
    #[serde(rename = "unrestricted")]
    Unrestricted,
    /// Apply strict policy.
    #[serde(rename = "strict")]
    Strict,
}

/// Bounded admitted client input.
#[derive(Clone, Debug, Deserialize)]
pub struct InputCommand {
    /// Optional acknowledgement correlation identifier.
    pub request_id: Option<NonEmptyString>,
    /// Runtime receiving the input.
    pub runtime: RuntimeScope,
    /// Bounded input payload.
    pub payload: BoundedJsonValue,
}

/// Scoped replay request.
#[derive(Clone, Debug, Deserialize)]
pub struct SyncCommand {
    /// Optional response correlation identifier.
    pub request_id: Option<NonEmptyString>,
    /// Runtime whose state is replayed.
    pub runtime: RuntimeScope,
    /// Whether stale approvals should be recovered.
    #[serde(default = "recover_approvals_default")]
    pub recover_approvals: bool,
    /// Whether unchanged device status must be emitted.
    pub force_device_status: Option<bool>,
}

const fn recover_approvals_default() -> bool {
    true
}

/// Scoped run cancellation request.
#[derive(Clone, Debug, Deserialize)]
pub struct AbortMessageCommand {
    /// Optional response correlation identifier.
    pub request_id: Option<NonEmptyString>,
    /// Runtime whose work is cancelled.
    pub runtime: RuntimeScope,
    /// Optional specific run identifier; null means active work.
    pub run_id: Option<RunId>,
}

/// Typed bounded device-state payload.
#[derive(Clone, Debug, Deserialize)]
pub struct ChangeDeviceStatePayload {
    /// Optional permission mode.
    pub mode: Option<RuntimeMode>,
    /// Optional working directory.
    pub cwd: Option<String>,
    /// Optional target agent identifier.
    pub agent_id: Option<NonEmptyString>,
    /// Optional target conversation identifier.
    pub conversation_id: Option<NonEmptyString>,
}

/// Scoped device-state change.
#[derive(Clone, Debug, Deserialize)]
pub struct ChangeDeviceStateCommand {
    /// Runtime receiving the state change.
    pub runtime: RuntimeScope,
    /// Typed state-change payload.
    pub payload: ChangeDeviceStatePayload,
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known Runtime commands.
pub fn decode(frame: &DecodedFrame) -> Result<Option<RuntimeCommand>, ProtocolErrorEnvelope> {
    use lotta_protocol::WsProtocolCommand as Tag;
    let lotta_protocol::DecodeOutcome::Accepted(tag) = frame.effects.outcome else {
        return Ok(None);
    };
    let value = frame.value.clone();
    let parsed = match tag {
        Tag::RuntimeStart => serde_json::from_value(value)
            .map(Box::new)
            .map(RuntimeCommand::RuntimeStart),
        Tag::Input => serde_json::from_value(value).map(RuntimeCommand::Input),
        Tag::Sync => serde_json::from_value(value).map(RuntimeCommand::Sync),
        Tag::AbortMessage => serde_json::from_value(value).map(RuntimeCommand::AbortMessage),
        Tag::ChangeDeviceState => {
            serde_json::from_value(value).map(RuntimeCommand::ChangeDeviceState)
        }
        _ => return Ok(None),
    };
    parsed.map(Some).map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "runtime_command_invalid",
        "invalid runtime command",
        frame.request_id.clone(),
    )
}

impl RuntimeStartCommand {
    /// Rejects each mutually exclusive pair before a service operation.
    ///
    /// Neither choice is rejected because the canonical TypeScript shape keeps
    /// both members optional and the service may supply defaults.
    ///
    /// # Errors
    /// Returns a correlated protocol error when both members of either pair exist.
    pub fn validate_structure(&self) -> Result<(), ProtocolErrorEnvelope> {
        let create_agent_valid = self
            .create_agent
            .as_ref()
            .is_none_or(|options| options.body.as_value().is_object());
        let create_conversation_valid = self.create_conversation.as_ref().is_none_or(|options| {
            options
                .body
                .as_ref()
                .is_none_or(|body| body.as_value().is_object())
        });
        if !create_agent_valid || !create_conversation_valid {
            return Err(ProtocolErrorEnvelope::new(
                "runtime_start_structure_invalid",
                "invalid runtime start structure",
                Some(self.request_id.as_str().to_owned()),
            ));
        }
        Ok(())
    }

    /// Rejects mutually exclusive choices before service work.
    ///
    /// # Errors
    /// Returns a correlated protocol error when both members of either pair exist.
    pub fn validate_choices(&self) -> Result<(), ProtocolErrorEnvelope> {
        let agent_conflict = self.agent_id.is_some() && self.create_agent.is_some();
        let conversation_conflict =
            self.conversation_id.is_some() && self.create_conversation.is_some();
        if agent_conflict || conversation_conflict {
            return Err(ProtocolErrorEnvelope::new(
                "runtime_start_choice_invalid",
                "invalid runtime start choices",
                Some(self.request_id.as_str().to_owned()),
            ));
        }
        Ok(())
    }
}
