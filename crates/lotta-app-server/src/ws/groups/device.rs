//! WebSocket device command group.
//!
//! Decodes and routes the pinned §WebSocket command groups Device row — the
//! `execute_command`, `remove_queue_item`, `search_branches`,
//! `checkout_branch`, `secret_list`, and `secret_apply` commands — mirroring
//! the pinned listener handlers. Routing rules this server keeps:
//!
//! * `remove_queue_item` never mutates queue storage directly: it resolves the
//!   scope's Task 18
//!   [`lotta_runtime::ConversationQueue`] and calls its removal operation, so
//!   every answer carries the wire disposition (`dequeued`/`cancelled`) and an
//!   authoritative post-mutation snapshot that is rebroadcast as an
//!   `update_queue` listener state message.
//! * `execute_command` resolves slash/mod command identifiers through the
//!   Task 45 mod command registry when one is registered; identifiers no mod
//!   published answer the pinned failure shape without side effects.
//! * Branch operations run bounded `git` invocations confined to this server's
//!   workspace root (see [`crate::ws::device_support`]).
//! * Secret list/apply persist through the Task 52 local-backend secrets file,
//!   and responses carry names only — plaintext is write-only.
//!
//! Background-process snapshots ride inside `update_device_status` listener
//! state messages (re-emitted after a successful checkout, like the pinned
//! baseline) and are deliberately not model-facing tools; see
//! [`crate::ws::device::BackgroundProcessSource`].

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lotta_domain::{NonEmptyString, RuntimeScope};
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::ConversationQueue;
use lotta_runtime::queue_snapshot::{QueueMutationEvent, QueueSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    error::AppServerError,
    errors::ProtocolErrorEnvelope,
    framing::DecodedFrame,
    ws::connection::ConnectionId,
    ws::device_support::{
        AgentSecretsStore, BRANCH_CHECKOUT_TIMEOUT_MS, BRANCH_QUERY_BYTES_MAX,
        BRANCH_RESULTS_DEFAULT_MAX, BRANCH_RESULTS_MAX, BRANCH_SEARCH_TIMEOUT_MS, GitBranchInfo,
        normalize_secret_name, parse_branches, run_git, valid_secret_name,
    },
};

/// Scrubbed failure detail for a rejected slash/mod dispatch.
const COMMAND_UNKNOWN: &str = "unknown command";
/// Scrubbed failure detail when git cannot complete a branch operation.
const BRANCH_FAILURE_SEARCH: &str = "Failed to search branches";
/// Scrubbed failure detail when git cannot complete a branch operation.
const BRANCH_FAILURE_CHECKOUT: &str = "Failed to checkout branch";
/// Pinned rejection text prefix for invalid secret names.
const SECRET_NAME_INVALID_PREFIX: &str = "Invalid secret name '";
/// Pinned rejection text suffix for invalid secret names.
const SECRET_NAME_INVALID_SUFFIX: &str = "'. Use uppercase letters, numbers, and underscores only.";

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
    /// Agent whose stored secret names to list.
    pub agent_id: String,
}

/// Pinned `secret_apply` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
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

/// The six concrete Device-row commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum DeviceCommand {
    /// Runs one slash or mod command for a runtime.
    #[serde(rename = "execute_command")]
    ExecuteCommand(Box<ExecuteCommandPayload>),
    /// Removes one queued input through the Task 18 queue.
    #[serde(rename = "remove_queue_item")]
    RemoveQueueItem(RemoveQueueItemPayload),
    /// Lists branches filtered by substring under the workspace root.
    #[serde(rename = "search_branches")]
    SearchBranches(SearchBranchesPayload),
    /// Checks out (optionally creating) one branch under the workspace root.
    #[serde(rename = "checkout_branch")]
    CheckoutBranch(CheckoutBranchPayload),
    /// Lists stored secret names for one agent.
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
    /// The requested branch now checked out.
    pub branch: String,
    /// Whether the checkout completed.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One name-only stored secret entry; values never cross the wire.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SecretEntry {
    /// Stored secret name.
    pub key: String,
}

/// Pinned-arity `secret_list_response` message carrying names only.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SecretListResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// Whether the listing completed.
    pub success: bool,
    /// Sorted stored secret names for the agent.
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

/// Authoritative queue state rebroadcast after a routed removal.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueueUpdateMessage {
    /// Owning agent identifier.
    pub agent_id: String,
    /// Owning conversation identifier.
    pub conversation_id: String,
    /// Authoritative snapshot: revision plus remaining items.
    pub queue: Value,
    /// Ordered explicit removal transitions with wire dispositions.
    pub removed: Value,
}

/// Listener device-status state refresh carrying background-process summaries.
///
/// Like the pinned baseline, background-process snapshots travel as listener
/// state inside `update_device_status`; they never become model tools.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeviceStatusUpdateMessage {
    /// Current background-process snapshot section.
    pub background_processes: Vec<BackgroundProcessSummary>,
}

/// All outbound device group messages, including listener state updates.
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
    /// Secret-name listing result.
    #[serde(rename = "secret_list_response")]
    SecretList(SecretListResponseMessage),
    /// Secret-batch result.
    #[serde(rename = "secret_apply_response")]
    SecretApply(SecretApplyResponseMessage),
    /// Authoritative queue state after a routed removal.
    #[serde(rename = "update_queue")]
    QueueUpdate(QueueUpdateMessage),
    /// Listener device-status refresh carrying background processes.
    #[serde(rename = "update_device_status")]
    StatusUpdate(DeviceStatusUpdateMessage),
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

/// Pinned monitor background-process summary.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MonitorBackgroundProcessSummary {
    /// Stable process identity.
    pub process_id: String,
    /// Human description.
    pub description: String,
    /// What opened the monitor.
    pub source: MonitorSource,
    /// Start instant in epoch milliseconds.
    pub started_at_ms: i64,
    /// Always running while listed.
    pub status: String,
    /// Whether the monitor survives its session.
    pub persistent: bool,
}

/// Origin of one monitor background process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorSource {
    /// Opened by a shell command.
    Command,
    /// Opened over the WebSocket control surface.
    Websocket,
}

/// Pinned agent-task background-process summary.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgentTaskBackgroundProcessSummary {
    /// Stable task identity.
    pub process_id: String,
    /// Display type of the delegated task.
    pub task_type: String,
    /// Human description.
    pub description: String,
    /// Start instant in epoch milliseconds.
    pub started_at_ms: i64,
    /// Lifecycle status.
    pub status: String,
    /// Originating subagent, when attributed.
    pub subagent_id: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `BackgroundProcessSummary` union.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackgroundProcessSummary {
    /// A launched shell process.
    Bash(BashBackgroundProcessSummary),
    /// A long-lived monitor.
    Monitor(MonitorBackgroundProcessSummary),
    /// A delegated agent task.
    AgentTask(AgentTaskBackgroundProcessSummary),
}

/// Source of the background-process section of device-status snapshots.
///
/// This stays a protocol service: nothing registers these names as model tools.
pub trait BackgroundProcessSource: Send + Sync {
    /// Returns the current running-process summary list.
    fn snapshot(&self) -> Vec<BackgroundProcessSummary>;
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

/// Applies wire device commands against queues, the mod registry, git, and the
/// secrets store, emitting responses and listener state through the forwarder.
pub struct DeviceBridge {
    workspace_root: PathBuf,
    secrets: AgentSecretsStore,
    queues: Mutex<HashMap<RuntimeScope, ConversationQueue>>,
    mod_commands: Mutex<Option<Arc<lotta_extensions::mods::registry::ModRegistries>>>,
    background: Arc<dyn BackgroundProcessSource>,
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
            queues: Mutex::new(HashMap::new()),
            mod_commands: Mutex::new(None),
            background: Arc::new(NoRunningProcesses),
            forward,
        })
    }

    /// Registers the Task 45 mod command registry backing `execute_command`.
    pub fn register_mod_commands(
        &self,
        registries: Arc<lotta_extensions::mods::registry::ModRegistries>,
    ) {
        *lock(&self.mod_commands) = Some(registries);
    }

    /// Replaces the background-process source feeding status snapshots.
    pub fn register_background_processes(&mut self, source: Arc<dyn BackgroundProcessSource>) {
        self.background = source;
    }

    /// Routes one decoded command in a detached task, like the pinned listener.
    pub fn handle(self: &Arc<Self>, connection: ConnectionId, command: &DeviceCommand) {
        let this = Arc::clone(self);
        let command = command.clone();
        tokio::spawn(async move { this.apply(connection, &command).await });
    }

    /// Applies one command inline, emitting messages through the forwarder.
    pub async fn apply(&self, connection: ConnectionId, command: &DeviceCommand) {
        match command {
            DeviceCommand::ExecuteCommand(payload) => {
                self.execute_command(connection, payload).await;
            }
            DeviceCommand::RemoveQueueItem(payload) => {
                self.remove_queue_item(connection, payload);
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
        let outcome = self.run_mod_command(&command.command_id, command.args.as_deref());
        let (success, output) = match outcome.await {
            Ok(output) => (true, output),
            Err(output) => (false, output),
        };
        self.emit(
            connection,
            DeviceMessage::ExecuteCommand(ExecuteCommandResponseMessage {
                request_id: command.request_id.clone(),
                success,
                output,
            }),
        );
    }

    async fn run_mod_command(
        &self,
        command_id: &str,
        args: Option<&str>,
    ) -> Result<String, String> {
        use lotta_extensions::mods::types::RegistrationName;
        let registry = lock(&self.mod_commands).clone();
        let Some(registry) = registry else {
            return Err(COMMAND_UNKNOWN.to_owned());
        };
        let runtime = registry.runtime().map_err(|_| COMMAND_UNKNOWN.to_owned())?;
        let name =
            RegistrationName::new(command_id.to_owned()).map_err(|_| COMMAND_UNKNOWN.to_owned())?;
        let handle = runtime
            .command(&name)
            .map_err(|_| COMMAND_UNKNOWN.to_owned())?;
        let value = handle
            .call(
                json!({ "args": args.unwrap_or_default() }),
                CancellationToken::new(),
            )
            .await
            .map_err(|_| COMMAND_UNKNOWN.to_owned())?;
        Ok(match value {
            Value::String(text) => text,
            other => other.to_string(),
        })
    }

    fn remove_queue_item(&self, connection: ConnectionId, command: &RemoveQueueItemPayload) {
        let removal = NonEmptyString::new(command.item_id.clone())
            .ok()
            .and_then(|item_id| self.remove_from_queue(&command.runtime, &item_id));
        self.emit(
            connection,
            DeviceMessage::RemoveQueueItem(RemoveQueueItemResponseMessage {
                request_id: command.request_id.clone(),
                success: removal.is_some(),
                item_id: command.item_id.clone(),
            }),
        );
        if let Some((removed, snapshot)) = removal {
            self.emit(
                connection,
                DeviceMessage::QueueUpdate(QueueUpdateMessage {
                    agent_id: command.runtime.agent_id.as_str().to_owned(),
                    conversation_id: command.runtime.conversation_id.as_str().to_owned(),
                    queue: queue_snapshot_json(&snapshot),
                    removed,
                }),
            );
        }
    }

    fn remove_from_queue(
        &self,
        runtime: &RuntimeScope,
        item_id: &NonEmptyString,
    ) -> Option<(Value, QueueSnapshot)> {
        let mut queues = lock(&self.queues);
        let mutation = queues.get_mut(runtime)?.cancel(item_id).ok()??;
        Some((
            transition_json(mutation.event()),
            mutation.snapshot().clone(),
        ))
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
        let create = command.create.unwrap_or(false);
        let args: Vec<&str> = if create {
            vec!["checkout", "-b", &command.branch]
        } else {
            vec!["checkout", &command.branch]
        };
        let outcome = run_git(
            &self.workspace_root,
            command.cwd.as_ref(),
            &args,
            BRANCH_CHECKOUT_TIMEOUT_MS,
        )
        .await;
        let (success, error) = match outcome {
            Ok(_) => (true, None),
            Err(error) => (
                false,
                Some(if error.trim().is_empty() {
                    BRANCH_FAILURE_CHECKOUT.to_owned()
                } else {
                    error
                }),
            ),
        };
        self.emit(
            connection,
            DeviceMessage::CheckoutBranch(CheckoutBranchResponseMessage {
                request_id: command.request_id.clone(),
                branch: command.branch.clone(),
                success,
                error,
            }),
        );
        if success {
            self.emit(
                connection,
                DeviceMessage::StatusUpdate(DeviceStatusUpdateMessage {
                    background_processes: self.background.snapshot(),
                }),
            );
        }
    }

    fn secret_list(&self, connection: ConnectionId, command: &SecretListPayload) {
        let message = match self.secrets.names(&command.agent_id) {
            Ok(names) => SecretListResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                secrets: names.into_iter().map(|key| SecretEntry { key }).collect(),
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

    #[cfg(test)]
    pub(crate) fn enqueue_for_test(&self, scope: &RuntimeScope, item_id: &str) {
        let item = lotta_domain::QueueItem {
            id: NonEmptyString::new(item_id.to_owned()).expect("test item id"),
            client_message_id: NonEmptyString::new(format!("client-{item_id}"))
                .expect("test client id"),
            kind: lotta_domain::QueueItemKind::Message,
            source: lotta_domain::QueueItemSource::User,
            content: lotta_domain::BoundedJsonValue::new(json!({"text": "queued"}))
                .expect("test content"),
            enqueued_at: lotta_domain::Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z")
                .expect("test timestamp"),
            extras: lotta_domain::EntityExtras::default(),
        };
        lock(&self.queues)
            .entry(scope.clone())
            .or_default()
            .enqueue(item)
            .map(|_mutation| ())
            .expect("bounded queue");
    }
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

fn queue_snapshot_json(snapshot: &QueueSnapshot) -> Value {
    json!({
        "revision": snapshot.revision(),
        "items": snapshot.items(),
    })
}

fn transition_json(event: &QueueMutationEvent) -> Value {
    match event {
        QueueMutationEvent::Removed(item, disposition) => json!([{
            "item_id": item.id.as_str(),
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
