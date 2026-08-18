//! Bounded discovery, collision-safe namespacing, and atomic per-server refresh.

use super::{
    MCP_SERVERS_PER_AGENT_MAX, MCP_TOOLS_PER_SERVER_MAX,
    client::{ServerId, SessionId},
    transport::{
        ClientInfo, MCP_DISCOVERY_PAGES_MAX, McpError, McpTransport, RemoteTool, await_mcp,
    },
};
use lotta_domain::{AgentId, BoundedJsonValue};
use lotta_runtime::{
    bounds::{EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX},
    ports::{
        InternalToolName, ModelFacingToolName, PermissionAction, SecretRedactionPolicy,
        SecretRedactionSpec, ToolApprovalPolicy, ToolDefinition, ToolDescriptionAsset,
        ToolExecutionOwner, ToolInputSchema, ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage,
        ToolOutputLimit, ToolTimeout,
    },
};
use lotta_tools::{
    ToolRegistration, ToolRegistry,
    pipeline::{
        ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fmt::Write as _,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const MCP_CURSOR_BYTES_MAX: usize = 4_096;
const MCP_DISCOVERY_BYTES_MAX: usize = 16 * 1024 * 1024;
const MCP_PUBLICATION_RETRIES_MAX: usize = 8;

/// Secret-free discovery or publication failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DiscoveryError {
    /// Agent/server count exceeded the canonical bound.
    #[error("MCP server limit reached")]
    ServerLimit,
    /// Per-server tool, page, or byte bound was exceeded.
    #[error("MCP discovery limit reached")]
    ToolLimit,
    /// A pagination cursor repeated.
    #[error("MCP discovery pagination cycle")]
    PaginationCycle,
    /// A tool name, description, or schema was invalid.
    #[error("MCP tool definition was invalid")]
    Definition,
    /// Namespaced tool collided with another source.
    #[error("MCP tool name collided")]
    Collision,
    /// Transport session failed.
    #[error("MCP transport failed")]
    Transport,
    /// Task 32 publication revision changed.
    #[error("MCP registry revision changed")]
    RevisionMismatch,
    /// Manager synchronization failed.
    #[error("MCP manager synchronization failed")]
    Poisoned,
}
impl From<McpError> for DiscoveryError {
    fn from(_: McpError) -> Self {
        Self::Transport
    }
}

struct ServerGroup {
    session_id: SessionId,
    transport: Arc<dyn McpTransport>,
    registrations: Arc<Vec<ToolRegistration>>,
}
/// Current group allocation and exact session.
pub type CurrentGroup = (Arc<Vec<ToolRegistration>>, SessionId);

struct State {
    groups: BTreeMap<ServerId, Arc<ServerGroup>>,
}

/// Agent-scoped MCP server manager using one Task 32 registry.
pub struct McpManager {
    agent_id: AgentId,
    registry: Arc<ToolRegistry>,
    state: Mutex<State>,
    mutation: Mutex<()>,
}
impl fmt::Debug for McpManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("McpManager([redacted])")
    }
}
impl McpManager {
    /// Creates one manager preserving supplied native/controller/other-source registrations.
    #[must_use]
    pub fn new(
        agent_id: AgentId,
        registry: Arc<ToolRegistry>,
        other: Vec<ToolRegistration>,
    ) -> Self {
        if registry
            .snapshot()
            .is_ok_and(|snapshot| snapshot.is_empty())
            && !other.is_empty()
        {
            let _ = registry.update(lotta_tools::ToolsetId::None, &other, None);
        }
        Self {
            agent_id,
            registry,
            state: Mutex::new(State {
                groups: BTreeMap::new(),
            }),
            mutation: Mutex::new(()),
        }
    }

    /// Connects, initializes, fully discovers, then atomically installs one server group.
    pub async fn connect(
        &self,
        server: ServerId,
        session_id: SessionId,
        transport: Arc<dyn McpTransport>,
        cancellation: CancellationToken,
    ) -> Result<Arc<Vec<ToolRegistration>>, DiscoveryError> {
        let result = self
            .prepare_and_publish(server, session_id, Arc::clone(&transport), cancellation)
            .await;
        if result.is_err() {
            let cleanup = CancellationToken::new();
            let _ = await_mcp(&cleanup, transport.shutdown()).await;
        }
        result
    }

    async fn prepare_and_publish(
        &self,
        server: ServerId,
        session_id: SessionId,
        transport: Arc<dyn McpTransport>,
        cancellation: CancellationToken,
    ) -> Result<Arc<Vec<ToolRegistration>>, DiscoveryError> {
        {
            let state = self.state.lock().map_err(|_| DiscoveryError::Poisoned)?;
            if !state.groups.contains_key(&server)
                && state.groups.len() >= MCP_SERVERS_PER_AGENT_MAX.value
            {
                return Err(DiscoveryError::ServerLimit);
            }
        }
        await_mcp(
            &cancellation,
            transport.initialize(
                ClientInfo {
                    name: "lotta".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
                cancellation.clone(),
            ),
        )
        .await?;
        let remote = discover_all(transport.as_ref(), cancellation).await?;
        let candidate = Arc::new(build_group(
            &server,
            &session_id,
            Arc::clone(&transport),
            remote,
        )?);
        self.publish(server, session_id, transport, candidate).await
    }

    async fn publish(
        &self,
        server: ServerId,
        session_id: SessionId,
        transport: Arc<dyn McpTransport>,
        candidate: Arc<Vec<ToolRegistration>>,
    ) -> Result<Arc<Vec<ToolRegistration>>, DiscoveryError> {
        let (old_group, result) = {
            let _mutation_guard = self.mutation.lock().map_err(|_| DiscoveryError::Poisoned)?;
            let mut state = self.state.lock().map_err(|_| DiscoveryError::Poisoned)?;
            if !state.groups.contains_key(&server)
                && state.groups.len() >= MCP_SERVERS_PER_AGENT_MAX.value
            {
                return Err(DiscoveryError::ServerLimit);
            }
            let tracked: BTreeSet<String> = state
                .groups
                .get(&server)
                .into_iter()
                .flat_map(|group| group.registrations.iter())
                .map(|registration| registration.definition.internal_name.as_str().to_owned())
                .collect();
            let mut publication = None;
            for _ in 0..MCP_PUBLICATION_RETRIES_MAX {
                let revision = self.registry.revision().map_err(map_registry)?;
                let snapshot = self.registry.snapshot().map_err(map_registry)?;
                let (toolset, mut complete) = snapshot.complete_registrations();
                complete.retain(|registration| {
                    let name = registration.definition.internal_name.as_str();
                    !tracked.contains(name)
                        || state.groups.iter().any(|(source, group)| {
                            source != &server
                                && group
                                    .registrations
                                    .iter()
                                    .any(|item| item.definition.internal_name.as_str() == name)
                        })
                });
                complete.extend(candidate.iter().cloned());
                match self.registry.publish(revision, toolset, &complete, None) {
                    Ok(_) => {
                        publication = Some(());
                        break;
                    }
                    Err(lotta_tools::RegistryError::RevisionMismatch) => {}
                    Err(error) => return Err(map_registry(error)),
                }
            }
            publication.ok_or(DiscoveryError::RevisionMismatch)?;
            let old = state.groups.insert(
                server,
                Arc::new(ServerGroup {
                    session_id,
                    transport: Arc::clone(&transport),
                    registrations: Arc::clone(&candidate),
                }),
            );
            (old, Ok(Arc::clone(&candidate)))
        };
        if let Some(old) = old_group {
            let cleanup = CancellationToken::new();
            let _ = await_mcp(&cleanup, old.transport.shutdown()).await;
        }
        result
    }

    /// Returns the current number of installed server groups for diagnostics.
    pub fn server_count(&self) -> Result<usize, DiscoveryError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| DiscoveryError::Poisoned)?
            .groups
            .len())
    }
    /// Returns the current exact per-server group allocation and session for diagnostics/tests.
    pub fn group(&self, server: &ServerId) -> Result<Option<CurrentGroup>, DiscoveryError> {
        let state = self.state.lock().map_err(|_| DiscoveryError::Poisoned)?;
        Ok(state
            .groups
            .get(server)
            .map(|group| (Arc::clone(&group.registrations), group.session_id.clone())))
    }
    /// Returns the bound manager's exact agent identifier.
    #[must_use]
    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
}

async fn discover_all(
    transport: &dyn McpTransport,
    cancellation: CancellationToken,
) -> Result<Vec<RemoteTool>, DiscoveryError> {
    let mut tools = Vec::new();
    let mut cursors = BTreeSet::new();
    let mut cursor = None;
    let mut bytes = 0usize;
    for _ in 0..MCP_DISCOVERY_PAGES_MAX {
        let page = transport
            .tools_list(cursor.as_deref(), cancellation.clone())
            .await?;
        if tools
            .len()
            .checked_add(page.tools.len())
            .ok_or(DiscoveryError::ToolLimit)?
            > MCP_TOOLS_PER_SERVER_MAX.value
        {
            return Err(DiscoveryError::ToolLimit);
        }
        for tool in page.tools {
            bytes = bytes
                .checked_add(
                    serde_json::to_vec(&tool.input_schema)
                        .map_err(|_| DiscoveryError::Definition)?
                        .len(),
                )
                .and_then(|sum| sum.checked_add(tool.name.len()))
                .ok_or(DiscoveryError::ToolLimit)?;
            if bytes > MCP_DISCOVERY_BYTES_MAX {
                return Err(DiscoveryError::ToolLimit);
            }
            tools.push(tool);
        }
        let Some(next) = page.next_cursor else {
            return Ok(tools);
        };
        if next.is_empty() || next.len() > MCP_CURSOR_BYTES_MAX || !cursors.insert(next.clone()) {
            return Err(DiscoveryError::PaginationCycle);
        }
        cursor = Some(next);
    }
    Err(DiscoveryError::ToolLimit)
}

fn build_group(
    server: &ServerId,
    session: &SessionId,
    transport: Arc<dyn McpTransport>,
    tools: Vec<RemoteTool>,
) -> Result<Vec<ToolRegistration>, DiscoveryError> {
    let mut registrations = Vec::new();
    registrations
        .try_reserve_exact(tools.len())
        .map_err(|_| DiscoveryError::ToolLimit)?;
    let mut names = BTreeSet::new();
    for tool in tools {
        let namespaced = namespace(server.as_str(), &tool.name)?;
        if !names.insert(namespaced.clone()) {
            return Err(DiscoveryError::Collision);
        }
        registrations.push(registration(
            namespaced,
            tool,
            session.clone(),
            Arc::clone(&transport),
        )?);
    }
    Ok(registrations)
}

/// Exact pinned namespacing: `mcp__<encoded server>__<encoded tool>`.
pub fn namespace(server: &str, tool: &str) -> Result<String, DiscoveryError> {
    if server.is_empty() || tool.is_empty() {
        return Err(DiscoveryError::Definition);
    }
    let result = format!("mcp__{}__{}", encode_name(server), encode_name(tool));
    InternalToolName::new(result.clone()).map_err(|_| DiscoveryError::Definition)?;
    Ok(result)
}
/// Decodes one exact MCP namespace produced by [`namespace`].
pub fn decode_namespace(value: &str) -> Result<(String, String), DiscoveryError> {
    let encoded = value
        .strip_prefix("mcp__")
        .ok_or(DiscoveryError::Definition)?;
    let (server, tool) = encoded.split_once("__").ok_or(DiscoveryError::Definition)?;
    let decoded = (decode_name(server)?, decode_name(tool)?);
    if namespace(&decoded.0, &decoded.1).as_deref() != Ok(value) {
        return Err(DiscoveryError::Definition);
    }
    Ok(decoded)
}
fn encode_name(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || *byte == b'-' {
            result.push(char::from(*byte));
        } else {
            result.push('_');
            let _ = write!(result, "{byte:02X}");
        }
    }
    result
}
fn decode_name(value: &str) -> Result<String, DiscoveryError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'_' {
            if index + 2 >= bytes.len() {
                return Err(DiscoveryError::Definition);
            }
            let text = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| DiscoveryError::Definition)?;
            decoded.push(u8::from_str_radix(text, 16).map_err(|_| DiscoveryError::Definition)?);
            index += 3;
        } else if bytes[index].is_ascii_alphanumeric() || bytes[index] == b'-' {
            decoded.push(bytes[index]);
            index += 1;
        } else {
            return Err(DiscoveryError::Definition);
        }
    }
    String::from_utf8(decoded).map_err(|_| DiscoveryError::Definition)
}
fn registration(
    name: String,
    remote: RemoteTool,
    session: SessionId,
    transport: Arc<dyn McpTransport>,
) -> Result<ToolRegistration, DiscoveryError> {
    if remote.name.is_empty() {
        return Err(DiscoveryError::Definition);
    }
    let description = remote
        .description
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("Tool {} from MCP server", remote.name));
    let bounded =
        BoundedJsonValue::new(remote.input_schema).map_err(|_| DiscoveryError::Definition)?;
    let schema = ToolInputSchema::new(bounded).map_err(|_| DiscoveryError::Definition)?;
    jsonschema::validator_for(schema.as_value()).map_err(|_| DiscoveryError::Definition)?;
    let definition = ToolDefinition::new(
        InternalToolName::new(name.clone()).map_err(|_| DiscoveryError::Definition)?,
        ModelFacingToolName::new(name).map_err(|_| DiscoveryError::Definition)?,
        schema,
        ToolDescriptionAsset::new(description).map_err(|_| DiscoveryError::Definition)?,
        ToolExecutionOwner::Mcp,
        ToolApprovalPolicy::Never,
        PermissionAction::new("execute".into()).map_err(|_| DiscoveryError::Definition)?,
        ToolTimeout::new(Duration::from_millis(
            EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u64,
        ))
        .map_err(|_| DiscoveryError::Definition)?,
        ToolOutputLimit::new(
            TOOL_RESULT_BYTES_MAX.value,
            TOOL_RESULT_MODEL_CHARS_MAX.value,
        )
        .map_err(|_| DiscoveryError::Definition)?,
        SecretRedactionSpec::new(
            lotta_domain::BoundedVec::new(Vec::new()).map_err(|_| DiscoveryError::Definition)?,
            SecretRedactionPolicy::Redact,
        )
        .map_err(|_| DiscoveryError::Definition)?,
    );
    Ok(ToolRegistration {
        definition: Arc::new(definition),
        executor: Arc::new(McpExecutor {
            transport,
            session,
            remote_name: remote.name,
        }),
    })
}

struct McpExecutor {
    transport: Arc<dyn McpTransport>,
    session: SessionId,
    remote_name: String,
}
impl ToolExecutor for McpExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let transport = Arc::clone(&self.transport);
        let name = self.remote_name.clone();
        let _session = self.session.clone();
        Box::pin(async move {
            let result = tokio::time::timeout(
                request.deadline.get(),
                transport.tools_call(
                    &request.tool_call_id,
                    &name,
                    request.input.as_value().clone(),
                    request.cancellation.clone(),
                ),
            )
            .await;
            match result {
                Ok(Ok(value)) if value.is_error => {
                    Ok(RawToolOutcome::Failure(ToolOutcome::ToolDefinedError {
                        code: ToolOutcomeCode::new("mcp_error".into())
                            .map_err(|_| ExecutorError)?,
                        message: ToolOutcomeMessage::new("MCP tool returned an error".into())
                            .map_err(|_| ExecutorError)?,
                    }))
                }
                Ok(Ok(value)) => serde_json::to_string(&value)
                    .map(RawToolOutcome::Success)
                    .map_err(|_| ExecutorError),
                Ok(Err(McpError::Cancelled)) => {
                    Ok(RawToolOutcome::Failure(ToolOutcome::Interruption {
                        message: ToolOutcomeMessage::new("MCP tool call cancelled".into())
                            .map_err(|_| ExecutorError)?,
                    }))
                }
                Ok(Err(_)) => Err(ExecutorError),
                Err(_) => Ok(RawToolOutcome::Failure(ToolOutcome::Timeout {
                    message: ToolOutcomeMessage::new("MCP tool call timed out".into())
                        .map_err(|_| ExecutorError)?,
                })),
            }
        })
    }
}

fn map_registry(error: lotta_tools::RegistryError) -> DiscoveryError {
    match error {
        lotta_tools::RegistryError::RevisionMismatch => DiscoveryError::RevisionMismatch,
        lotta_tools::RegistryError::DuplicateInternal(_)
        | lotta_tools::RegistryError::DuplicateModel(_) => DiscoveryError::Collision,
        _ => DiscoveryError::Definition,
    }
}
