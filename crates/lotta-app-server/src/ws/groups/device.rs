//! WebSocket device command group.
//!
//! Decodes and routes the pinned §WebSocket command groups Device row — the
//! `execute_command`, `remove_queue_item`, `search_branches`,
//! `checkout_branch`, `secret_list`, and `secret_apply` commands — mirroring
//! the pinned listener handlers. Routing rules this server keeps:
//!
//! * `remove_queue_item` never mutates queue storage directly: it resolves the
//!   registered [`crate::ws::device::QueueAuthority`] — the production port
//!   over the same
//!   [`lotta_runtime::ListenerRuntime`] instance turns use — and calls its
//!   removal operation, so every answer carries the wire disposition and the
//!   authoritative post-mutation state rebroadcast as a `RuntimeEvent::
//!   UpdateQueue` through the runtime router to scope subscribers.
//! * `execute_command` first dispatches the pinned built-in slash commands,
//!   then resolves remaining identifiers through the Task 45 mod command
//!   registry when one is registered; identifiers no mod published answer the
//!   pinned failure shape without side effects. Commands resolve per runtime
//!   scope: the requesting connection must subscribe to the payload's scope,
//!   and the mod call receives scoped cwd/conversation/cancellation context.
//! * Branch operations run bounded `git` invocations confined to this server's
//!   workspace root (see [`crate::ws::device_support`]).
//! * Secret list/apply persist through the Task 52 local-backend secrets side
//!   store; `secret_list` returns the pinned `{key, value}` entries whose
//!   plaintext values are intentionally exposed to the authenticated secrets
//!   modal (see the pinned `protocol_v2.ts` secret-list contract).
//!
//! Background-process snapshots ride inside `RuntimeEvent::UpdateDeviceStatus`
//! (re-emitted after a successful checkout and `/reload`, like the pinned
//! baseline) and are deliberately not model-facing tools; see
//! [`crate::ws::device::BackgroundProcessSource`].

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lotta_domain::{NonEmptyString, RuntimeScope};
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::QueueMutation;
use lotta_runtime::queue_snapshot::{QueueMutationEvent, QueueSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    error::AppServerError,
    errors::ProtocolErrorEnvelope,
    framing::DecodedFrame,
    ws::connection::ConnectionId,
    ws::device_support::{
        AgentSecretsStore, BRANCH_CHECKOUT_TIMEOUT_MS, BRANCH_QUERY_BYTES_MAX,
        BRANCH_RESULTS_DEFAULT_MAX, BRANCH_RESULTS_MAX, BRANCH_SEARCH_TIMEOUT_MS, GitBranchInfo,
        normalize_secret_name, parse_branches, run_git, valid_branch_name, valid_secret_name,
    },
    ws::event::RuntimeEvent,
    ws::service::RuntimeEventSink,
};

/// Scrubbed failure detail for a rejected slash/mod dispatch.
const COMMAND_UNKNOWN_PREFIX: &str = "Unknown command: ";
/// Scrubbed failure detail when git cannot complete a branch operation.
const BRANCH_FAILURE_SEARCH: &str = "Failed to search branches";
/// Scrubbed failure detail when git cannot complete a branch operation.
const BRANCH_FAILURE_CHECKOUT: &str = "Failed to checkout branch";
/// Scrubbed failure detail for a rejected branch reference name.
const BRANCH_NAME_INVALID: &str = "invalid branch name";
/// Pinned rejection text prefix for invalid secret names.
const SECRET_NAME_INVALID_PREFIX: &str = "Invalid secret name '";
/// Pinned rejection text suffix for invalid secret names.
const SECRET_NAME_INVALID_SUFFIX: &str = "'. Use uppercase letters, numbers, and underscores only.";

/// The built-in slash commands dispatched by this server's `execute_command`.
///
/// Mirrors the pinned `SUPPORTED_REMOTE_COMMANDS` list plus the two aliases
/// its switch statement accepts (`reflect`, `set-max-context`). Built-ins
/// backed by real server ports run; the rest answer the pinned failure shape
/// rather than pretending success.
pub const BUILTIN_COMMANDS: [&str; 11] = [
    "clear",
    "doctor",
    "init",
    "remember",
    "compact",
    "reload",
    "reflect",
    "context-limit",
    "set-max-context",
    "channels",
    "upgrade-letta-code",
];

/// Pinned `execute_command` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecuteCommandPayload {
    /// Response correlation identifier.
    pub request_id: String,
    /// Slash or mod command identifier to run.
    pub command_id: String,
    /// Runtime scope the command targets.
    pub runtime: RuntimeScope,
    /// Optional arguments following the command name.
    #[serde(default)]
    pub args: Option<String>,
}

/// Pinned `remove_queue_item` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RemoveQueueItemPayload {
    /// Response correlation identifier.
    pub request_id: String,
    /// Runtime scope owning the queue.
    pub runtime: RuntimeScope,
    /// Queue item identifier to remove.
    pub item_id: String,
}

/// Pinned `search_branches` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchBranchesPayload {
    /// Response correlation identifier.
    pub request_id: String,
    /// Case-insensitive substring filter; empty lists all branches.
    pub query: String,
    /// Optional result cap falling back to the pinned default of twenty.
    #[serde(default)]
    pub max_results: Option<usize>,
    /// Optional working directory confined to this server's workspace root.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Pinned `checkout_branch` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CheckoutBranchPayload {
    /// Response correlation identifier.
    pub request_id: String,
    /// Branch name to check out.
    pub branch: String,
    /// Whether to create the branch when it does not exist.
    #[serde(default)]
    pub create: Option<bool>,
    /// Optional working directory confined to this server's workspace root.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Pinned `secret_list` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SecretListPayload {
    /// Response correlation identifier.
    pub request_id: String,
    /// Agent whose stored secret entries to list.
    pub agent_id: String,
}

/// Pinned `secret_apply` payload.
///
/// Values are plaintext by contract; the explicit [`Debug`] impl redacts them
/// so diagnostics never capture secret material.
#[derive(Clone, Deserialize, Serialize)]
pub struct SecretApplyPayload {
    /// Response correlation identifier.
    pub request_id: String,
    /// Agent whose secret record is mutated atomically.
    pub agent_id: String,
    /// Keys to add or replace.
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    /// Keys to remove; keys also present in `set` resolve to removal.
    #[serde(default)]
    pub unset: Vec<String>,
}

impl std::fmt::Debug for SecretApplyPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted: BTreeMap<&String, &str> =
            self.set.keys().map(|key| (key, "[redacted]")).collect();
        formatter
            .debug_struct("SecretApplyPayload")
            .field("request_id", &self.request_id)
            .field("agent_id", &self.agent_id)
            .field("set", &redacted)
            .field("unset", &self.unset)
            .finish()
    }
}

/// The six concrete Device-row commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum DeviceCommand {
    /// Runs one slash or mod command for a runtime.
    #[serde(rename = "execute_command")]
    ExecuteCommand(Box<ExecuteCommandPayload>),
    /// Removes one queued input through the authoritative Task 18 queue.
    #[serde(rename = "remove_queue_item")]
    RemoveQueueItem(RemoveQueueItemPayload),
    /// Lists branches filtered by substring under the workspace root.
    #[serde(rename = "search_branches")]
    SearchBranches(SearchBranchesPayload),
    /// Checks out (optionally creating) one branch under the workspace root.
    #[serde(rename = "checkout_branch")]
    CheckoutBranch(CheckoutBranchPayload),
    /// Lists stored secret entries for one agent.
    #[serde(rename = "secret_list")]
    SecretList(SecretListPayload),
    /// Applies one atomic batch of secret mutations for one agent.
    #[serde(rename = "secret_apply")]
    SecretApply(SecretApplyPayload),
}

/// Pinned `execute_command_response` message.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExecuteCommandResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// Whether the command resolved and ran.
    pub success: bool,
    /// Command output, or scrubbed failure detail on rejection.
    pub output: String,
}

/// Pinned `remove_queue_item_response` message.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RemoveQueueItemResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// Whether the item existed and was removed.
    pub success: bool,
    /// Echoed item identifier.
    pub item_id: String,
}

/// Pinned `search_branches_response` message.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchBranchesResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// Matching branches in repository order.
    pub branches: Vec<GitBranchInfo>,
    /// Whether the search completed.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `checkout_branch_response` message.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CheckoutBranchResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// The branch now checked out, queried from HEAD after the mutation.
    pub branch: String,
    /// Whether the checkout completed.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One stored secret entry carrying the pinned `{key, value}` pair.
///
/// Plaintext values are intentionally exposed to the authenticated secrets
/// modal per the pinned protocol contract; they are write-only everywhere else.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SecretEntry {
    /// Stored secret name.
    pub key: String,
    /// Stored secret plaintext.
    pub value: String,
}

/// Pinned-arity `secret_list_response` message.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SecretListResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// Whether the listing completed.
    pub success: bool,
    /// Sorted stored secret entries for the agent.
    pub secrets: Vec<SecretEntry>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `secret_apply_response` message.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SecretApplyResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// Whether the batch applied.
    pub success: bool,
    /// Sorted names retained after the apply.
    pub names: Vec<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// All outbound device group response messages.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum DeviceMessage {
    /// Slash/mod execution result.
    #[serde(rename = "execute_command_response")]
    ExecuteCommand(ExecuteCommandResponseMessage),
    /// Queue-removal result.
    #[serde(rename = "remove_queue_item_response")]
    RemoveQueueItem(RemoveQueueItemResponseMessage),
    /// Branch-search result.
    #[serde(rename = "search_branches_response")]
    SearchBranches(SearchBranchesResponseMessage),
    /// Branch-checkout result.
    #[serde(rename = "checkout_branch_response")]
    CheckoutBranch(CheckoutBranchResponseMessage),
    /// Secret-entry listing result.
    #[serde(rename = "secret_list_response")]
    SecretList(SecretListResponseMessage),
    /// Secret-batch result.
    #[serde(rename = "secret_apply_response")]
    SecretApply(SecretApplyResponseMessage),
}

/// Authoritative queue-removal port shared between this group and the
/// production runtime.
///
/// When injected, removals mutate the exact [`lotta_runtime::ListenerRuntime`]
/// instance the turn pipeline uses — so a removed item can never be pumped by
/// an active turn afterward, and answers carry the real wire transition.
pub trait QueueAuthority: Send + Sync {
    /// Cancels one queued item on the authoritative queue for `scope`.
    ///
    /// # Errors
    /// Returns the production registry failure verbatim.
    fn remove_queued<'a>(
        &'a self,
        scope: &'a RuntimeScope,
        item_id: &'a NonEmptyString,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Option<QueueMutation>, lotta_runtime::RuntimeError>,
                > + Send
                + 'a,
        >,
    >;
}

/// Resolves the runtime scopes one connection currently subscribes to.
///
/// Backs both the `execute_command` scope check and the per-connection
/// device-status refresh fan-out.
pub type ScopeGate = Arc<dyn Fn(ConnectionId) -> Vec<RuntimeScope> + Send + Sync>;

/// Resolves the scoped working directory feeding command context.
pub type CwdResolver = Arc<dyn Fn(&RuntimeScope) -> PathBuf + Send + Sync>;

/// Future returned by one built-in command run.
pub type BuiltinRun =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>;

/// Runs one built-in slash command.
///
/// Composed by the host over bridges the device group cannot see directly
/// (conversation compaction, status re-advertisement); failures carry the
/// scrubbed output text of the pinned failure shape.
pub type BuiltinRunner =
    Arc<dyn Fn(ConnectionId, ExecuteCommandPayload, CancellationToken) -> BuiltinRun + Send + Sync>;

/// Source of the background-process section of device-status snapshots.
///
/// This stays a protocol service: nothing registers these names as model tools.
pub trait BackgroundProcessSource: Send + Sync {
    /// Returns the current running-process summary list.
    fn snapshot(&self) -> Vec<BackgroundProcessSummary>;
}

/// Pinned bash background-process summary.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BashBackgroundProcessSummary {
    /// Stable process identity.
    pub process_id: String,
    /// Launched command line.
    pub command: String,
    /// Start instant in epoch milliseconds, when known.
    pub started_at_ms: Option<i64>,
    /// Lifecycle status.
    pub status: String,
    /// Exit code, once settled.
    pub exit_code: Option<i32>,
}

/// Pinned `BackgroundProcessSummary` union over the process kinds this server
/// tracks; shell sessions are the one kind with a live production source.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackgroundProcessSummary {
    /// A launched shell process.
    Bash(BashBackgroundProcessSummary),
}

/// Default source reporting an empty snapshot until a host registers one.
#[derive(Debug, Default)]
pub struct NoRunningProcesses;

impl BackgroundProcessSource for NoRunningProcesses {
    fn snapshot(&self) -> Vec<BackgroundProcessSummary> {
        Vec::new()
    }
}

/// Push callback delivering one outbound message to one connection.
pub type DeviceForwarder =
    Arc<dyn Fn(ConnectionId, DeviceMessage) -> Result<(), AppServerError> + Send + Sync>;

#[cfg(test)]
pub(crate) fn inert_forwarder() -> DeviceForwarder {
    Arc::new(|_, _| Ok(()))
}

fn lock<T>(state: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Per-connection detached work bound to a cancellation token and joins.
struct ConnectionWorkers {
    cancellation: CancellationToken,
    joins: Vec<JoinHandle<()>>,
}

/// Applies wire device commands against the authoritative queue, the mod
/// registry, git, and the secrets store, emitting responses through the
/// forwarder and listener state through the runtime event sink.
pub struct DeviceBridge {
    workspace_root: PathBuf,
    secrets: AgentSecretsStore,
    queue_authority: Mutex<Option<Arc<dyn QueueAuthority>>>,
    mod_commands: Mutex<Option<Arc<lotta_extensions::mods::registry::ModRegistries>>>,
    background: Mutex<Arc<dyn BackgroundProcessSource>>,
    event_sink: Mutex<Option<Arc<dyn RuntimeEventSink>>>,
    scopes: Mutex<Option<ScopeGate>>,
    cwd_of: Mutex<Option<CwdResolver>>,
    builtins: Mutex<Option<BuiltinRunner>>,
    workers: Mutex<HashMap<ConnectionId, ConnectionWorkers>>,
    forward: DeviceForwarder,
}

impl DeviceBridge {
    /// Creates the production bridge over one workspace root and storage root.
    ///
    /// # Errors
    /// Returns [`crate::error::AppServerError::Config`] when the workspace
    /// root is not an absolute path.
    pub fn new(
        forward: DeviceForwarder,
        workspace_root: &Path,
        storage_dir: &Path,
    ) -> Result<Self, AppServerError> {
        if !workspace_root.is_absolute() {
            return Err(AppServerError::Config("device workspace root invalid"));
        }
        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            secrets: AgentSecretsStore::new(storage_dir),
            queue_authority: Mutex::new(None),
            mod_commands: Mutex::new(None),
            background: Mutex::new(Arc::new(NoRunningProcesses)),
            event_sink: Mutex::new(None),
            scopes: Mutex::new(None),
            cwd_of: Mutex::new(None),
            builtins: Mutex::new(None),
            workers: Mutex::new(HashMap::new()),
            forward,
        })
    }

    /// Registers the authoritative queue port backing `remove_queue_item`.
    pub fn register_queue_authority(&self, authority: Arc<dyn QueueAuthority>) {
        *lock(&self.queue_authority) = Some(authority);
    }

    /// Registers the Task 45 mod command registry backing `execute_command`.
    pub fn register_mod_commands(
        &self,
        registries: Arc<lotta_extensions::mods::registry::ModRegistries>,
    ) {
        *lock(&self.mod_commands) = Some(registries);
    }

    /// Registers a mod registry when the host supplied one.
    pub fn register_mod_commands_if_set(
        &self,
        registries: Option<Arc<lotta_extensions::mods::registry::ModRegistries>>,
    ) {
        if let Some(registries) = registries {
            self.register_mod_commands(registries);
        }
    }

    /// Registers the sink broadcasting listener state (`update_queue`,
    /// `update_device_status`) to runtime-scope subscribers.
    pub fn register_event_sink(&self, sink: Arc<dyn RuntimeEventSink>) {
        *lock(&self.event_sink) = Some(sink);
    }

    /// Registers the subscription gate validating `execute_command` targets.
    pub fn register_scope_gate(&self, gate: ScopeGate) {
        *lock(&self.scopes) = Some(gate);
    }

    /// Registers the resolver supplying scoped working directories.
    pub fn register_cwd_resolver(&self, resolver: CwdResolver) {
        *lock(&self.cwd_of) = Some(resolver);
    }

    /// Registers the runner dispatching pinned built-in slash commands.
    pub fn register_builtin_runner(&self, runner: BuiltinRunner) {
        *lock(&self.builtins) = Some(runner);
    }

    /// Replaces the background-process source feeding status snapshots.
    pub fn register_background_processes(&self, source: Arc<dyn BackgroundProcessSource>) {
        *lock(&self.background) = source;
    }

    /// Registers a background source when the host supplied one.
    pub fn register_background_processes_if_set(
        &self,
        source: Option<Arc<dyn BackgroundProcessSource>>,
    ) {
        if let Some(source) = source {
            self.register_background_processes(source);
        }
    }

    /// Routes one decoded command as tracked work bound to the connection.
    ///
    /// Like the pinned listener the work runs detached, but it stays owned:
    /// each task selects on its connection's cancellation token and its join
    /// handle is retained until [`Self::disconnect`] reaps it.
    pub fn handle(self: &Arc<Self>, connection: ConnectionId, command: &DeviceCommand) {
        let this = Arc::clone(self);
        let command = command.clone();
        let cancellation = {
            let mut workers = lock(&self.workers);
            workers
                .entry(connection)
                .or_insert_with(|| ConnectionWorkers {
                    cancellation: CancellationToken::new(),
                    joins: Vec::new(),
                })
                .cancellation
                .clone()
        };
        let join = tokio::spawn(async move {
            let work = this.apply(connection, &command);
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {},
                () = work => {},
            }
        });
        if let Some(entry) = lock(&self.workers).get_mut(&connection) {
            entry.joins.push(join);
        }
    }

    /// Cancels one connection's detached device work and reaps its tasks.
    pub async fn disconnect(&self, connection: ConnectionId) {
        let mut joins = Vec::new();
        if let Ok(mut workers) = self.workers.lock()
            && let Some(mut entry) = workers.remove(&connection)
        {
            entry.cancellation.cancel();
            joins.append(&mut entry.joins);
        }
        for join in joins.drain(..) {
            let _ = join.await;
        }
    }

    /// Applies one command inline, emitting messages through the forwarder.
    pub async fn apply(&self, connection: ConnectionId, command: &DeviceCommand) {
        match command {
            DeviceCommand::ExecuteCommand(payload) => {
                Box::pin(self.execute_command(connection, payload)).await;
            }
            DeviceCommand::RemoveQueueItem(payload) => {
                self.remove_queue_item(connection, payload).await;
            }
            DeviceCommand::SearchBranches(payload) => {
                self.search_branches(connection, payload).await;
            }
            DeviceCommand::CheckoutBranch(payload) => {
                self.checkout_branch(connection, payload).await;
            }
            DeviceCommand::SecretList(payload) => self.secret_list(connection, payload),
            DeviceCommand::SecretApply(payload) => self.secret_apply(connection, payload),
        }
    }

    async fn execute_command(&self, connection: ConnectionId, command: &ExecuteCommandPayload) {
        let outcome = match self.command_outcome(connection, command).await {
            Ok(output) => (true, output),
            Err(output) => (false, output),
        };
        let (success, output) = outcome;
        self.emit(
            connection,
            DeviceMessage::ExecuteCommand(ExecuteCommandResponseMessage {
                request_id: command.request_id.clone(),
                success,
                output,
            }),
        );
    }

    /// Validates the command target, then runs the pinned built-in table or a
    /// mod-published command with scoped context.
    async fn command_outcome(
        &self,
        connection: ConnectionId,
        command: &ExecuteCommandPayload,
    ) -> Result<String, String> {
        let unknown = || format!("{COMMAND_UNKNOWN_PREFIX}{}", command.command_id);
        if let Some(gate) = lock(&self.scopes).as_ref()
            && !gate(connection).contains(&command.runtime)
        {
            tracing::warn!(
                request_id = %command.request_id,
                "execute_command rejected for unsubscribed scope"
            );
            return Err(unknown());
        }
        // Detached commands run under their own token: no live connection is
        // attributable at inline-application time, so nothing cancels early.
        let cancellation = CancellationToken::new();
        if BUILTIN_COMMANDS.contains(&command.command_id.as_str()) {
            let runner = lock(&self.builtins).clone();
            return match runner {
                Some(runner) => runner(connection, command.clone(), cancellation).await,
                None => Err(unknown()),
            };
        }
        self.run_mod_command(command, cancellation).await
    }

    async fn run_mod_command(
        &self,
        command: &ExecuteCommandPayload,
        cancellation: CancellationToken,
    ) -> Result<String, String> {
        use lotta_extensions::mods::types::RegistrationName;
        let unknown = || format!("{COMMAND_UNKNOWN_PREFIX}{}", command.command_id);
        let registry = lock(&self.mod_commands).clone();
        let Some(registry) = registry else {
            return Err(unknown());
        };
        let runtime = registry.runtime().map_err(|_| unknown())?;
        let name = RegistrationName::new(command.command_id.clone()).map_err(|_| unknown())?;
        let handle = runtime.command(&name).map_err(|_| unknown())?;
        let value = handle
            .call(self.scoped_context(command), cancellation)
            .await
            .map_err(|_| unknown())?;
        Ok(match value {
            Value::String(text) => text,
            other => other.to_string(),
        })
    }

    /// Builds the scoped argument body handed to one mod command call:
    /// parsed args plus conversation/cwd/cancellation context, mirroring the
    /// pinned listener `ModCommandContext`.
    fn scoped_context(&self, command: &ExecuteCommandPayload) -> Value {
        let cwd = lock(&self.cwd_of).as_ref().map_or_else(
            || self.workspace_root.clone(),
            |resolve| resolve(&command.runtime),
        );
        json!({
            "args": command.args.clone().unwrap_or_default(),
            "command": command.command_id,
            "runtime": {
                "agent_id": command.runtime.agent_id.as_str(),
                "conversation_id": command.runtime.conversation_id.as_str(),
            },
            "conversation": {
                "agent_id": command.runtime.agent_id.as_str(),
                "id": command.runtime.conversation_id.as_str(),
            },
            "cwd": cwd.to_string_lossy(),
        })
    }

    async fn remove_queue_item(&self, connection: ConnectionId, command: &RemoveQueueItemPayload) {
        let removal = match NonEmptyString::new(command.item_id.clone()) {
            Ok(item_id) => self.remove_from_queue(&command.runtime, &item_id).await,
            Err(_) => None,
        };
        self.emit(
            connection,
            DeviceMessage::RemoveQueueItem(RemoveQueueItemResponseMessage {
                request_id: command.request_id.clone(),
                success: removal.is_some(),
                item_id: command.item_id.clone(),
            }),
        );
        if let Some((removed, snapshot)) = removal {
            self.broadcast_queue_update(&command.runtime, &snapshot, removed);
        }
    }

    async fn remove_from_queue(
        &self,
        runtime: &RuntimeScope,
        item_id: &NonEmptyString,
    ) -> Option<(Value, QueueSnapshot)> {
        let authority = lock(&self.queue_authority).clone()?;
        match authority.remove_queued(runtime, item_id).await {
            Ok(Some(mutation)) => Some((
                transition_json(mutation.event()),
                mutation.snapshot().clone(),
            )),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(
                    scope = %runtime.conversation_id.as_str(),
                    error = %error,
                    "authoritative queue removal failed"
                );
                None
            }
        }
    }

    /// Rebroadcasts one authoritative queue mutation to scope subscribers as
    /// a pinned-shape `update_queue` listener state message.
    fn broadcast_queue_update(
        &self,
        runtime: &RuntimeScope,
        snapshot: &QueueSnapshot,
        removed: Value,
    ) {
        let Some(sink) = lock(&self.event_sink).clone() else {
            return;
        };
        let queue = json!(snapshot.items());
        let (Ok(queue), Ok(removed)) = (
            lotta_domain::BoundedJsonValue::new(queue),
            lotta_domain::BoundedJsonValue::new(removed),
        ) else {
            tracing::warn!("bounded update_queue encoding failed");
            return;
        };
        if let Err(error) = sink.emit(runtime, RuntimeEvent::UpdateQueue { queue, removed }) {
            tracing::warn!(error = %error, "update_queue broadcast failed");
        }
    }

    async fn search_branches(&self, connection: ConnectionId, command: &SearchBranchesPayload) {
        let response = if command.query.len() > BRANCH_QUERY_BYTES_MAX {
            failed_search(&command.request_id, "branch query too long")
        } else {
            let max = command
                .max_results
                .unwrap_or(BRANCH_RESULTS_DEFAULT_MAX)
                .min(BRANCH_RESULTS_MAX);
            match run_git(
                &self.workspace_root,
                command.cwd.as_ref(),
                &["branch", "-a", "--format=%(refname:short)\t%(HEAD)"],
                BRANCH_SEARCH_TIMEOUT_MS,
            )
            .await
            {
                Ok(stdout) => SearchBranchesResponseMessage {
                    request_id: command.request_id.clone(),
                    branches: parse_branches(&stdout, &command.query, max),
                    success: true,
                    error: None,
                },
                Err(error) => failed_search(&command.request_id, &error),
            }
        };
        self.emit(connection, DeviceMessage::SearchBranches(response));
    }

    async fn checkout_branch(&self, connection: ConnectionId, command: &CheckoutBranchPayload) {
        let response = if valid_branch_name(&command.branch) {
            self.run_checkout(command).await
        } else {
            failed_checkout(&command.request_id, BRANCH_NAME_INVALID)
        };
        let success = response.success;
        self.emit(connection, DeviceMessage::CheckoutBranch(response));
        if success {
            self.refresh_device_status(connection);
        }
    }

    async fn run_checkout(&self, command: &CheckoutBranchPayload) -> CheckoutBranchResponseMessage {
        let create = command.create.unwrap_or(false);
        let args: Vec<&str> = if create {
            vec!["checkout", "-b", &command.branch]
        } else {
            vec!["checkout", &command.branch]
        };
        match run_git(
            &self.workspace_root,
            command.cwd.as_ref(),
            &args,
            BRANCH_CHECKOUT_TIMEOUT_MS,
        )
        .await
        {
            Ok(_) => self.checkout_answer(command).await,
            Err(error) => failed_checkout(
                &command.request_id,
                &non_empty_or(&error, BRANCH_FAILURE_CHECKOUT),
            ),
        }
    }

    /// Queries the actual checked-out branch from HEAD so the answer reports
    /// reality instead of echoing the request: a failed or empty HEAD query
    /// fails the checkout response rather than reporting the requested name.
    async fn checkout_answer(
        &self,
        command: &CheckoutBranchPayload,
    ) -> CheckoutBranchResponseMessage {
        let head = run_git(
            &self.workspace_root,
            command.cwd.as_ref(),
            &["rev-parse", "--abbrev-ref", "HEAD"],
            BRANCH_SEARCH_TIMEOUT_MS,
        )
        .await;
        let branch = head.ok().map(|stdout| stdout.trim().to_owned());
        match branch {
            Some(branch) if !branch.is_empty() => CheckoutBranchResponseMessage {
                request_id: command.request_id.clone(),
                branch,
                success: true,
                error: None,
            },
            _ => failed_checkout(&command.request_id, BRANCH_FAILURE_CHECKOUT),
        }
    }

    /// Re-advertises the device-status snapshot to one connection's scopes.
    ///
    /// Backs the pinned `/reload` behavior: refreshed registrations reach
    /// clients through a fresh `update_device_status` listener state message.
    pub fn refresh_status_for(&self, connection: ConnectionId) {
        self.refresh_device_status(connection);
    }

    /// Emits the complete scoped device-status snapshot as listener state to
    /// every runtime scope the requesting connection subscribes to.
    fn refresh_device_status(&self, connection: ConnectionId) {
        let Some(sink) = lock(&self.event_sink).clone() else {
            return;
        };
        let scopes = lock(&self.scopes)
            .as_ref()
            .map_or_else(Vec::new, |gate| gate(connection));
        if scopes.is_empty() {
            return;
        }
        let status = self.device_status_json();
        let Ok(status) = lotta_domain::BoundedJsonValue::new(status) else {
            tracing::warn!("bounded update_device_status encoding failed");
            return;
        };
        for scope in scopes {
            if let Err(error) = sink.emit(
                &scope,
                RuntimeEvent::UpdateDeviceStatus {
                    device_status: status.clone(),
                },
            ) {
                tracing::warn!(error = %error, "update_device_status broadcast failed");
            }
        }
    }

    /// Returns the current full device-status snapshot for authoritative sync.
    #[must_use]
    pub fn status_snapshot(&self) -> Value {
        self.device_status_json()
    }

    fn device_status_json(&self) -> Value {
        json!({
            "is_online": true,
            "current_working_directory": self.workspace_root.to_string_lossy(),
            "boot_working_directory": self.workspace_root.to_string_lossy(),
            "letta_code_version": env!("CARGO_PKG_VERSION"),
            "background_processes": lock(&self.background).snapshot(),
            "supported_commands": builtin_and_registered_commands(self),
            "pending_control_requests": [],
            "current_loaded_tools": [],
            "current_available_skills": [],
        })
    }

    fn secret_list(&self, connection: ConnectionId, command: &SecretListPayload) {
        let message = match self.secrets.entries(&command.agent_id) {
            Ok(entries) => SecretListResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                secrets: entries
                    .into_iter()
                    .map(|(key, value)| SecretEntry { key, value })
                    .collect(),
                error: None,
            },
            Err(error) => SecretListResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                secrets: Vec::new(),
                error: Some(error),
            },
        };
        self.emit(connection, DeviceMessage::SecretList(message));
    }

    fn secret_apply(&self, connection: ConnectionId, command: &SecretApplyPayload) {
        let message = match Self::validated_batch(command) {
            Ok((set, unset)) => self.applied_batch(command, set, &unset),
            Err(error) => failed_apply(&command.request_id, error),
        };
        self.emit(connection, DeviceMessage::SecretApply(message));
    }

    fn validated_batch(
        command: &SecretApplyPayload,
    ) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
        let mut set = BTreeMap::new();
        for (raw_key, value) in &command.set {
            let key = normalize_secret_name(raw_key);
            if !valid_secret_name(&key) {
                return Err(format!(
                    "{SECRET_NAME_INVALID_PREFIX}{raw_key}{SECRET_NAME_INVALID_SUFFIX}"
                ));
            }
            set.insert(key, value.clone());
        }
        let mut unset = Vec::with_capacity(command.unset.len());
        for raw_key in &command.unset {
            let key = normalize_secret_name(raw_key);
            if !valid_secret_name(&key) {
                return Err(format!(
                    "{SECRET_NAME_INVALID_PREFIX}{raw_key}{SECRET_NAME_INVALID_SUFFIX}"
                ));
            }
            unset.push(key);
        }
        Ok((set, unset))
    }

    fn applied_batch(
        &self,
        command: &SecretApplyPayload,
        set: BTreeMap<String, String>,
        unset: &[String],
    ) -> SecretApplyResponseMessage {
        match self.secrets.apply(&command.agent_id, set, unset) {
            Ok(names) => SecretApplyResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                names,
                error: None,
            },
            Err(error) => failed_apply(&command.request_id, error),
        }
    }

    fn emit(&self, connection: ConnectionId, message: DeviceMessage) {
        let _ = (self.forward)(connection, message);
    }
}

fn builtin_and_registered_commands(bridge: &DeviceBridge) -> Vec<String> {
    let mut commands: Vec<String> = BUILTIN_COMMANDS
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    if let Some(registries) = lock(&bridge.mod_commands).as_ref()
        && let Ok(snapshot) = registries.snapshot()
    {
        commands.extend(
            snapshot
                .commands
                .keys()
                .map(|name| name.as_str().to_owned()),
        );
    }
    commands.sort();
    commands.dedup();
    commands
}

fn failed_search(request_id: &str, error: &str) -> SearchBranchesResponseMessage {
    let error = non_empty_or(error, BRANCH_FAILURE_SEARCH);
    SearchBranchesResponseMessage {
        request_id: request_id.to_owned(),
        branches: Vec::new(),
        success: false,
        error: Some(error),
    }
}

fn failed_checkout(request_id: &str, error: &str) -> CheckoutBranchResponseMessage {
    let error = non_empty_or(error, BRANCH_FAILURE_CHECKOUT);
    CheckoutBranchResponseMessage {
        request_id: request_id.to_owned(),
        branch: String::new(),
        success: false,
        error: Some(error),
    }
}

fn failed_apply(request_id: &str, error: String) -> SecretApplyResponseMessage {
    SecretApplyResponseMessage {
        request_id: request_id.to_owned(),
        success: false,
        names: Vec::new(),
        error: Some(error),
    }
}

fn non_empty_or(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

fn transition_json(event: &QueueMutationEvent) -> Value {
    match event {
        QueueMutationEvent::Removed(item, disposition) => json!([{
            "client_message_id": item.client_message_id.as_str(),
            "disposition": disposition,
        }]),
        _ => json!([]),
    }
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known device commands,
/// including missing required fields or wrong-typed values.
pub fn decode(frame: &DecodedFrame) -> Result<Option<DeviceCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let parsed = match tag {
        Tag::ExecuteCommand => typed::<ExecuteCommandPayload>(frame)
            .map(Box::new)
            .map(DeviceCommand::ExecuteCommand),
        Tag::RemoveQueueItem => {
            typed::<RemoveQueueItemPayload>(frame).map(DeviceCommand::RemoveQueueItem)
        }
        Tag::SearchBranches => {
            typed::<SearchBranchesPayload>(frame).map(DeviceCommand::SearchBranches)
        }
        Tag::CheckoutBranch => {
            typed::<CheckoutBranchPayload>(frame).map(DeviceCommand::CheckoutBranch)
        }
        Tag::SecretList => typed::<SecretListPayload>(frame).map(DeviceCommand::SecretList),
        Tag::SecretApply => typed::<SecretApplyPayload>(frame).map(DeviceCommand::SecretApply),
        _ => return Ok(None),
    }?;
    Ok(Some(parsed))
}

fn typed<T: serde::de::DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ProtocolErrorEnvelope> {
    serde_json::from_value(frame.value.clone()).map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "device_command_invalid",
        "invalid device command",
        frame.request_id.clone(),
    )
}

#[cfg(test)]
#[path = "device_background_tests.rs"]
mod background_snapshot_is_state;
#[cfg(test)]
#[path = "device_commands_tests.rs"]
mod commands;
#[cfg(test)]
#[path = "device_fixture_round_trip_tests.rs"]
mod fixture_round_trip;
#[cfg(test)]
#[path = "device_remove_queue_item_tests.rs"]
mod remove_queue_item;
