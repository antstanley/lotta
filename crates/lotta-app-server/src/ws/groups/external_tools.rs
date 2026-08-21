//! WebSocket external-tool command group.
//!
//! Decodes the pinned `runtime_external_tools_update` and
//! `external_tool_call_response` commands and applies them to the Task 41
//! registry
//! ([`lotta_tools::external::ExternalToolManager`](lotta_tools::external::ExternalToolManager)).
//! Updates are the protocol's complete atomic replacement groups: every member
//! is validated before any state changes, so an invalid member leaves
//! registrations, revisions, and pending calls untouched. Outbound
//! `external_tool_call_request` messages carry all six correlation fields
//! (runtime, request ID, tool call ID, name, arguments, optional scope ID),
//! and responses resolve only through the originating connection identity
//! because the bridge always submits them on the handle of the connection that
//! received the frame; Task 41 then rejects any owner or correlation mismatch
//! while the call remains pending.
//!
//! Each controller connection owns one bounded receiver pump per runtime
//! scope. Pumps are cancelled by
//! [`ExternalToolBridge::disconnect`](crate::ws::external_tools::ExternalToolBridge::disconnect)
//! when their connection closes, which also closes the Task 41 handles so
//! every pending call settles with the typed owner-disconnected result.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
};

use lotta_domain::{
    BoundedJsonValue, BoundedVec, NonEmptyString, RuntimeScope,
    bounds::EXTERNAL_TOOLS_PER_RUNTIME_MAX,
};
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::ports::{InternalToolName, ModelFacingToolName};
#[cfg(test)]
use lotta_tools::RegistrySnapshot;
use lotta_tools::external::{
    ConnectionId as ControllerConnectionId, ControllerConnection, ControllerReceiver,
    ExternalCallRequest, ExternalCallResponse, ExternalRequestId, ExternalToolGroup,
    ExternalToolManager, ExternalToolMember, GroupRevision, RegistrySelection, ResponseDisposition,
    RuntimeId as ControllerRuntimeId, ScopeId, ToolCallId as PendingToolCallId,
};
use lotta_tools::{ToolRegistry, toolset::ToolsetId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    error::AppServerError, errors::ProtocolErrorEnvelope, framing::DecodedFrame,
    ws::connection::ConnectionId,
};

const REGISTRY_UNAVAILABLE: &str = "external tool registry unavailable";

/// Maximum `updates[]` entries in one update command.
pub const EXTERNAL_TOOL_UPDATES_PER_COMMAND_MAX: usize = 64;
/// Maximum `runtimes[]` scopes addressed by one update entry.
pub const EXTERNAL_TOOL_UPDATE_RUNTIMES_MAX: usize = 64;
/// Maximum registration groups per update entry.
pub const EXTERNAL_TOOL_UPDATE_GROUPS_MAX: usize = EXTERNAL_TOOLS_PER_RUNTIME_MAX.value;

/// The two concrete external-tool group commands.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ExternalToolsCommand {
    /// Atomic registration replacement for one or more runtimes.
    #[serde(rename = "runtime_external_tools_update")]
    ToolsUpdate(Box<ToolsUpdateCommand>),
    /// Completion of one previously forwarded tool call.
    #[serde(rename = "external_tool_call_response")]
    CallResponse(Box<ToolCallResponseCommand>),
}

/// Pinned `runtime_external_tools_update` payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolsUpdateCommand {
    /// Response correlation identifier.
    pub request_id: NonEmptyString,
    /// Per-runtime replacement groups applied atomically per scope.
    pub updates: BoundedVec<ToolsUpdateGroup, EXTERNAL_TOOL_UPDATES_PER_COMMAND_MAX>,
}

/// One update entry applying a definition set to exact runtimes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolsUpdateGroup {
    /// Runtimes receiving the replacement set; must be non-empty and unique.
    pub runtimes: BoundedVec<RuntimeScope, EXTERNAL_TOOL_UPDATE_RUNTIMES_MAX>,
    /// Complete replacement registration groups for those runtimes.
    pub external_tools: BoundedVec<ToolsRegistrationGroup, EXTERNAL_TOOL_UPDATE_GROUPS_MAX>,
}

/// One scoped registration group with its member definitions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolsRegistrationGroup {
    /// Optional hidden scope selecting these tools on input turns.
    pub scope_id: Option<String>,
    /// Complete member set replaced atomically.
    pub tools: BoundedVec<ToolsDefinitionPayload, { EXTERNAL_TOOLS_PER_RUNTIME_MAX.value }>,
}

/// One controller-owned tool definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolsDefinitionPayload {
    /// Exact internal and model-facing name.
    pub name: String,
    /// Optional human label retained for transport projections.
    pub label: Option<String>,
    /// Bounded model description.
    pub description: String,
    /// JSON Schema object for arguments.
    pub parameters: BoundedJsonValue,
}

/// Pinned `external_tool_call_response` payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallResponseCommand {
    /// Forwarded request identifier this response completes.
    pub request_id: NonEmptyString,
    /// Result content, mutually exclusive with `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error text, mutually exclusive with `result`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Optional echoed provider tool-call identity checked when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// The two outbound external-tool group messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ExternalToolsMessage {
    /// Call request forwarded to exactly one owning controller connection.
    #[serde(rename = "external_tool_call_request")]
    CallRequest(ToolCallRequestMessage),
    /// Update application outcome returned to the requesting connection.
    #[serde(rename = "runtime_external_tools_update_response")]
    UpdateResponse(ToolsUpdateResponseMessage),
}

/// Pinned `external_tool_call_request` message with every correlation field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallRequestMessage {
    /// Server-generated collision-safe request identity.
    pub request_id: String,
    /// Runtime scope owning the registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeScope>,
    /// Exact optional registration scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    /// Provider tool-call identity.
    pub tool_call_id: String,
    /// Model-facing tool name.
    pub tool_name: String,
    /// Schema-validated bounded arguments.
    pub input: serde_json::Value,
}

/// Pinned `runtime_external_tools_update_response` message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolsUpdateResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// Whether every addressed runtime accepted the replacement atomically.
    pub success: bool,
    /// Scrubbed failure detail, omitted on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known external-tool commands.
pub fn decode(frame: &DecodedFrame) -> Result<Option<ExternalToolsCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    match tag {
        Tag::RuntimeExternalToolsUpdate => {
            serde_json::from_value::<ToolsUpdateCommand>(frame.value.clone())
                .map_err(|_| invalid(frame))
                .and_then(|command| {
                    validate_update(&command)
                        .map(|()| Some(ExternalToolsCommand::ToolsUpdate(Box::new(command))))
                })
        }
        Tag::ExternalToolCallResponse => {
            serde_json::from_value::<ToolCallResponseCommand>(frame.value.clone())
                .map_err(|_| invalid(frame))
                .and_then(validate_call_response)
                .map(Some)
        }
        _ => Ok(None),
    }
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "external_tool_command_invalid",
        "invalid external tool command",
        frame.request_id.clone(),
    )
}

fn validate_update(command: &ToolsUpdateCommand) -> Result<(), ProtocolErrorEnvelope> {
    let mut seen = HashSet::new();
    for update in command.updates.as_slice() {
        if update.runtimes.is_empty() {
            return Err(shape_error(None));
        }
        for scope in update.runtimes.as_slice() {
            let key = (scope.agent_id.clone(), scope.conversation_id.clone());
            if !seen.insert(key) {
                return Err(shape_error(None));
            }
        }
        let definitions_valid = update.external_tools.as_slice().iter().all(|group| {
            group
                .tools
                .as_slice()
                .iter()
                .all(|member| member.parameters.as_value().is_object())
        });
        if !definitions_valid {
            return Err(shape_error(Some(command.request_id.as_str())));
        }
    }
    Ok(())
}

fn validate_call_response(
    command: ToolCallResponseCommand,
) -> Result<ExternalToolsCommand, ProtocolErrorEnvelope> {
    let exclusive = matches!(
        (&command.result, &command.error),
        (Some(_), None) | (None, Some(_))
    );
    if exclusive {
        Ok(ExternalToolsCommand::CallResponse(Box::new(command)))
    } else {
        Err(shape_error(Some(command.request_id.as_str())))
    }
}

fn shape_error(request_id: Option<&str>) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "external_tool_command_invalid",
        "invalid external tool command",
        request_id.map(ToOwned::to_owned),
    )
}

/// Push callback delivering one outbound message to one connection.
pub type ExternalForwarder =
    Arc<dyn Fn(ConnectionId, ExternalToolsMessage) -> Result<(), AppServerError> + Send + Sync>;

struct ForwardedCall {
    scope: RuntimeScope,
    runtime_id: ControllerRuntimeId,
    tool_call_id: PendingToolCallId,
    internal_name: InternalToolName,
    model_name: ModelFacingToolName,
    scope_id: Option<ScopeId>,
}

struct ScopeEntry {
    runtime_id: ControllerRuntimeId,
    manager: Arc<ExternalToolManager>,
    revision: u64,
    connections: HashMap<ConnectionId, Arc<ControllerConnection>>,
}

#[derive(Default)]
struct BridgeState {
    scopes: HashMap<RuntimeScope, ScopeEntry>,
    inflight: HashMap<(ConnectionId, String), ForwardedCall>,
    pumps: HashMap<ConnectionId, Vec<CancellationToken>>,
}

/// Applies wire external-tool commands to the Task 41 registry per connection.
pub struct ExternalToolBridge {
    state: Arc<Mutex<BridgeState>>,
    forward: ExternalForwarder,
}

impl ExternalToolBridge {
    /// Creates a bridge pushing forwarded requests through `forward`.
    #[must_use]
    pub fn new(forward: ExternalForwarder) -> Self {
        Self {
            state: Arc::new(Mutex::new(BridgeState::default())),
            forward,
        }
    }

    /// Applies the complete atomic replacement to every addressed runtime.
    ///
    /// Per-runtime application is atomic (Task 41 validates before mutating);
    /// the first failing scope stops the command with a scrubbed detail.
    ///
    /// # Errors
    /// Returns scrubbed registration text when any scope rejects the group.
    pub fn apply_update(
        &self,
        connection: ConnectionId,
        command: &ToolsUpdateCommand,
    ) -> Result<(), String> {
        let mut state = lock(&self.state).map_err(|error| error.to_string())?;
        for update in command.updates.as_slice() {
            let groups = registration_groups(update)?;
            for scope in update.runtimes.as_slice() {
                ensure_connection(&mut state, &self.state, &self.forward, connection, scope)?;
                apply_groups(&mut state, connection, scope, &groups)?;
            }
        }
        Ok(())
    }

    /// Resolves a forwarded call only through this connection's own handle.
    ///
    /// Unknown request identifiers and responses arriving on another
    /// connection return [`ResponseDisposition::UnknownIgnored`] and leave the
    /// pending call untouched; correlation echoes are re-checked by Task 41.
    #[must_use]
    pub fn handle_response(
        &self,
        connection: ConnectionId,
        command: &ToolCallResponseCommand,
    ) -> ResponseDisposition {
        let ignored = ResponseDisposition::UnknownIgnored;
        let key = (connection, command.request_id.as_str().to_owned());
        let Ok(state) = lock(&self.state) else {
            return ignored;
        };
        let Some(record) = state.inflight.get(&key) else {
            return ignored;
        };
        let Some(response) = correlated_response(record, command) else {
            return ResponseDisposition::InvalidResponse;
        };
        let Some(handle) = state
            .scopes
            .get(&record.scope)
            .and_then(|entry| entry.connections.get(&connection))
            .map(Arc::clone)
        else {
            return ignored;
        };
        drop(state);
        let disposition = handle
            .respond(response)
            .unwrap_or(ResponseDisposition::UnknownIgnored);
        if disposition == ResponseDisposition::Resolved
            && let Ok(mut state) = lock(&self.state)
        {
            state.inflight.remove(&key);
        }
        disposition
    }

    /// Closes this connection's handles, cancelling pumps and resolving every
    /// owned pending call with the typed owner-disconnected outcome.
    pub fn disconnect(&self, connection: ConnectionId) {
        let Ok(mut state) = lock(&self.state) else {
            return;
        };
        if let Some(pumps) = state.pumps.remove(&connection) {
            for token in pumps {
                token.cancel();
            }
        }
        for entry in state.scopes.values_mut() {
            if let Some(handle) = entry.connections.remove(&connection) {
                handle.close();
            }
        }
        state.inflight.retain(|(owner, _), _| *owner != connection);
    }

    #[cfg(test)]
    pub(crate) fn snapshot(
        &self,
        scope: &RuntimeScope,
        scope_id: Option<&str>,
    ) -> Option<Arc<RegistrySnapshot>> {
        let scope_id = match scope_id.map(|value| ScopeId::new(value.to_owned())) {
            Some(Ok(parsed)) => Some(parsed),
            _ => None,
        };
        let selection = RegistrySelection {
            toolset: ToolsetId::None,
            scope_id: scope_id.as_ref(),
            allowlist: None,
        };
        self.state
            .lock()
            .ok()?
            .scopes
            .get(scope)?
            .manager
            .select(selection)
            .ok()
    }

    #[cfg(test)]
    pub(crate) fn tracked_revision(&self, scope: &RuntimeScope) -> Option<u64> {
        self.state
            .lock()
            .ok()?
            .scopes
            .get(scope)
            .map(|e| e.revision)
    }
}

fn lock(state: &Mutex<BridgeState>) -> Result<MutexGuard<'_, BridgeState>, AppServerError> {
    state.lock().map_err(|_| AppServerError::Internal)
}

#[cfg(test)]
pub(crate) fn inert_forwarder() -> ExternalForwarder {
    Arc::new(|_, _| Ok(()))
}

fn ensure_connection(
    state: &mut BridgeState,
    state_arc: &Arc<Mutex<BridgeState>>,
    forward: &ExternalForwarder,
    connection: ConnectionId,
    scope: &RuntimeScope,
) -> Result<(), String> {
    if state
        .scopes
        .get(scope)
        .is_some_and(|entry| entry.connections.contains_key(&connection))
    {
        return Ok(());
    }
    if !state.scopes.contains_key(scope) {
        let registry = Arc::new(ToolRegistry::new([]).map_err(|error| error.to_string())?);
        let runtime_id = scope_runtime_id(scope)?;
        let manager = ExternalToolManager::production(runtime_id.clone(), registry);
        state.scopes.insert(
            scope.clone(),
            ScopeEntry {
                runtime_id,
                manager: Arc::new(manager),
                revision: 0,
                connections: HashMap::new(),
            },
        );
    }
    let entry = state
        .scopes
        .get_mut(scope)
        .ok_or_else(|| REGISTRY_UNAVAILABLE.to_owned())?;
    let identity = ControllerConnectionId::new(format!("connection-{connection}"))
        .map_err(|_| "external controller identity invalid".to_owned())?;
    let (handle, receiver) = entry
        .manager
        .connect(identity)
        .map_err(|error| error.to_string())?;
    let token = CancellationToken::new();
    spawn_pump(
        Arc::clone(state_arc),
        Arc::clone(forward),
        connection,
        scope.clone(),
        token.clone(),
        receiver,
    );
    state.pumps.entry(connection).or_default().push(token);
    entry.connections.insert(connection, handle);
    Ok(())
}

fn apply_groups(
    state: &mut BridgeState,
    connection: ConnectionId,
    scope: &RuntimeScope,
    groups: &[ExternalToolGroup],
) -> Result<(), String> {
    let entry = state
        .scopes
        .get_mut(scope)
        .ok_or_else(|| REGISTRY_UNAVAILABLE.to_owned())?;
    let handle = entry
        .connections
        .get(&connection)
        .cloned()
        .ok_or_else(|| REGISTRY_UNAVAILABLE.to_owned())?;
    let expected = GroupRevision::new(entry.revision);
    let revision = entry
        .manager
        .update(
            &handle,
            &entry.runtime_id,
            expected,
            groups,
            unscoped_selection(),
        )
        .map_err(|error| error.to_string())?;
    entry.revision = revision.value();
    Ok(())
}

fn spawn_pump(
    state: Arc<Mutex<BridgeState>>,
    forward: ExternalForwarder,
    connection: ConnectionId,
    scope: RuntimeScope,
    token: CancellationToken,
    mut receiver: ControllerReceiver,
) {
    tokio::spawn(async move {
        loop {
            let request = tokio::select! {
                biased;
                () = token.cancelled() => break,
                request = receiver.recv() => request,
            };
            let Some(request) = request else { break };
            record_inflight(&state, connection, &scope, &request);
            let _ = forward(connection, call_request_message(&scope, &request));
        }
    });
}

fn record_inflight(
    state: &Mutex<BridgeState>,
    connection: ConnectionId,
    scope: &RuntimeScope,
    request: &ExternalCallRequest,
) {
    if let Ok(mut state) = state.lock() {
        state.inflight.insert(
            (connection, request.request_id.as_str().to_owned()),
            ForwardedCall {
                scope: scope.clone(),
                runtime_id: request.runtime_id.clone(),
                tool_call_id: request.tool_call_id.clone(),
                internal_name: request.internal_name.clone(),
                model_name: request.model_name.clone(),
                scope_id: request.scope_id.clone(),
            },
        );
    }
}

fn call_request_message(
    scope: &RuntimeScope,
    request: &ExternalCallRequest,
) -> ExternalToolsMessage {
    ExternalToolsMessage::CallRequest(ToolCallRequestMessage {
        request_id: request.request_id.as_str().to_owned(),
        runtime: Some(scope.clone()),
        scope_id: request.scope_id.as_ref().map(|id| id.as_str().to_owned()),
        tool_call_id: request.tool_call_id.as_str().to_owned(),
        tool_name: request.model_name.as_str().to_owned(),
        input: request.arguments.as_value().clone(),
    })
}

fn correlated_response(
    record: &ForwardedCall,
    command: &ToolCallResponseCommand,
) -> Option<ExternalCallResponse> {
    let request_id = ExternalRequestId::new(command.request_id.as_str().to_owned()).ok()?;
    let tool_call_id = match &command.tool_call_id {
        Some(echoed) => PendingToolCallId::new(echoed.clone()).ok()?,
        None => record.tool_call_id.clone(),
    };
    Some(ExternalCallResponse {
        runtime_id: record.runtime_id.clone(),
        request_id,
        tool_call_id,
        internal_name: record.internal_name.clone(),
        model_name: record.model_name.clone(),
        scope_id: record.scope_id.clone(),
        result: command.result.clone(),
        error: command.error.clone(),
    })
}

fn registration_groups(update: &ToolsUpdateGroup) -> Result<Vec<ExternalToolGroup>, String> {
    let mut groups = Vec::new();
    groups
        .try_reserve_exact(update.external_tools.len())
        .map_err(|_| REGISTRY_UNAVAILABLE.to_owned())?;
    for group in update.external_tools.as_slice() {
        groups.push(ExternalToolGroup {
            scope_id: group.scope_id.clone(),
            tools: group
                .tools
                .as_slice()
                .iter()
                .map(|member| ExternalToolMember {
                    name: member.name.clone(),
                    label: member.label.clone(),
                    description: member.description.clone(),
                    parameters: member.parameters.as_value().clone(),
                })
                .collect(),
        });
    }
    Ok(groups)
}

fn scope_runtime_id(scope: &RuntimeScope) -> Result<ControllerRuntimeId, String> {
    let key = format!(
        "{}:{}",
        scope.agent_id.as_str(),
        scope.conversation_id.as_str()
    );
    ControllerRuntimeId::new(key).map_err(|_| "external runtime identity invalid".to_owned())
}

fn unscoped_selection() -> RegistrySelection<'static> {
    RegistrySelection {
        toolset: ToolsetId::None,
        scope_id: None,
        allowlist: None,
    }
}

#[cfg(test)]
#[path = "external_tools_correlation_tests.rs"]
mod correlation;
#[cfg(test)]
#[path = "external_tools_fixture_round_trip_tests.rs"]
mod fixture_round_trip;
#[cfg(test)]
#[path = "external_tools_owner_disconnect_tests.rs"]
mod owner_disconnect;
#[cfg(test)]
#[path = "external_tools_support.rs"]
mod support;
#[cfg(test)]
#[path = "external_tools_update_tests.rs"]
mod update;
