//! WebSocket schedules command group.
//!
//! Decodes and routes the pinned `cron_list`, `cron_add`, `cron_get`,
//! `cron_runs`, `cron_trigger`, `cron_update`, `cron_delete`, and
//! `cron_delete_all` commands (the §WebSocket command groups Schedules row)
//! against one Task 60 [`SidePaths`](lotta_store::SidePaths) store rooted at
//! the canonical storage directory: `<root>/.letta/crons.json` CRUD through
//! [`ScheduleStore`](lotta_store::schedule::ScheduleStore) CAS, and
//! per-schedule run history through
//! [`RunLogStore`](lotta_store::schedule::RunLogStore).
//!
//! `cron_trigger` never starts a turn directly. It calls
//! [`ScheduleScheduler::trigger`](lotta_runtime::schedule::ScheduleScheduler::trigger),
//! which resolves or creates the target
//! runtime on this bridge's registry and enqueues exactly the same
//! `cron_prompt` admission as a scheduled fire — identical queue-item
//! identity, ordinary route, fired lifecycle transition, and run-log entry —
//! so dedup and history stay uniform across scheduled and manual fires. The
//! tick loop is deliberately not owned here: evaluation timing remains the
//! scheduler's concern.
//!
//! Add and update validate the cron or interval expression
//! ([`ScheduleExpression::parse`](lotta_runtime::schedule::ScheduleExpression::parse)
//! / [`parse_interval`](lotta_runtime::schedule::parse_interval)) and the IANA
//! timezone ([`IanaTimezone::new`](lotta_domain::IanaTimezone::new)) before
//! any store access, so invalid values are rejected without persisting
//! anything. Interval forms normalize to their
//! canonical five-field cron before persistence and surface the rounding note
//! as the add-response `warning`. Every mutating command emits a
//! `crons_updated` snapshot after its success response, mirroring the pinned
//! emit-after-response order; trigger emits none.
//!
//! Baseline degradations, kept honest: an omitted timezone defaults to UTC
//! rather than the host zone (the confined listener has no host identity),
//! first-ever creation of `crons.json` races unlocked where the baseline held
//! a file lock (subsequent writes stay CAS-guarded), stale-CAS conflicts and
//! blocking-pool exhaustion answer scrubbed failures rather than retrying the
//! pinned five-second lock loop, and run-history responses truncate entries
//! until they fit the transport frame ceiling.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use lotta_domain::{
    AgentId, Clock, ConversationId, IanaTimezone, NonEmptyString, Schedule, ScheduleStatus,
    Timestamp,
};
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{IdGenerator, PortFuture, SchedulePersistence};
use lotta_runtime::registry::ListenerRuntime;
use lotta_runtime::schedule::{
    JitterSource, RunLogEntry, ScheduleExpression, ScheduleFile, ScheduleScheduler, compute_jitter,
    parse_interval,
};
use lotta_store::schedule::{RunLogStore, ScheduleRevision, ScheduleService, ScheduleStore};
use lotta_store::{SidePaths, StoreErrorKind};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::{
    bounds::WS_FRAME_BYTES_MAX, error::AppServerError, errors::ProtocolErrorEnvelope,
    framing::DecodedFrame, ws::connection::ConnectionId,
};

/// Entries per `cron_runs_response` when the client omits `limit` (pinned parity).
pub const RUNS_PAGE_ENTRIES_DEFAULT: usize = 50;
/// Upper bound on run-log entries accepted for one page (pinned parity).
pub const RUNS_PAGE_ENTRIES_MAX: usize = 200;
/// Run-log records scanned to build one page before pagination (pinned parity).
pub const RUNS_SCAN_ENTRIES_MAX: usize = 5_000;
/// Active schedules permitted per agent before adds reject (pinned parity).
pub const ACTIVE_SCHEDULES_PER_AGENT_MAX: usize = 50;
/// Random bytes in one generated schedule identifier (eight hex characters).
pub const SCHEDULE_ID_BYTES_MAX: usize = 4;
/// Blocking-store permits shared by this bridge (mirrors the local pool bound).
pub const STORE_BLOCKING_PERMITS_MAX: usize = 8;

/// Scrubbed failure detail for rejected or failed listings.
const LIST_FAILURE: &str = "Failed to list crons";
/// Scrubbed failure detail for failed adds.
const ADD_FAILURE: &str = "Failed to add cron";
/// Scrubbed failure detail for failed fetches.
const GET_FAILURE: &str = "Failed to get cron";
/// Scrubbed failure detail for failed run-history reads.
const RUNS_FAILURE: &str = "Failed to list cron run history";
/// Scrubbed failure detail for failed triggers.
const TRIGGER_FAILURE: &str = "Failed to trigger cron";
/// Scrubbed failure detail for failed updates.
const UPDATE_FAILURE: &str = "Failed to update cron";
/// Scrubbed failure detail for failed deletes.
const DELETE_FAILURE: &str = "Failed to delete cron";
/// Scrubbed failure detail when the blocking store is unreachable.
const STORE_UNAVAILABLE: &str = "schedules store unavailable";
/// Pinned missing-schedule rejection text for updates.
const UPDATE_NOT_FOUND: &str = "Cron task not found";
/// Pinned missing-schedule rejection text for triggers.
const TRIGGER_NOT_FOUND: &str = "Schedule not found";
/// Pinned inactive-schedule rejection text for triggers.
const TRIGGER_INACTIVE: &str = "Schedule is not active";
/// Canonical timezone stored when a client omits one.
const DEFAULT_TIMEZONE: &str = "UTC";
/// Pinned conversation target storing fresh-conversation-per-fire semantics.
const NEW_CONVERSATION: &str = "new";
/// Rejection detail for an empty prompt.
const INVALID_PROMPT: &str = "Schedule prompt cannot be empty";
/// Rejection detail for an unusable conversation target.
const INVALID_CONVERSATION: &str = "Invalid conversation_id";
/// Rejection detail for an unknown IANA timezone.
const INVALID_TIMEZONE: &str = "Unknown IANA timezone";
/// Rejection detail for an unusable one-shot timestamp.
const INVALID_SCHEDULED_FOR: &str = "Invalid scheduled_for timestamp";
/// Rejection detail for an unusable agent identifier.
const INVALID_AGENT: &str = "Invalid agent_id";

/// Pinned `cron_list` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CronListCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Optional agent filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Optional conversation filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

/// Pinned `cron_add` payload.
#[allow(clippy::option_option)] // Three-state schema field requires absent/null/value.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CronAddCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Owning agent.
    pub agent_id: String,
    /// Conversation target; omitted or `"new"` fires into a fresh conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Five-field cron or interval form such as `10m`.
    pub cron: String,
    /// Optional IANA timezone; absence means UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Whether the schedule recurs.
    pub recurring: bool,
    /// Prompt enqueued on each fire.
    pub prompt: String,
    /// One-shot target timestamp.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "three_state::deserialize"
    )]
    pub scheduled_for: Option<Option<String>>,
}

/// Pinned `cron_get` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CronGetCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Requested schedule.
    pub task_id: String,
}

/// Pinned `cron_runs` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CronRunsCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Schedule whose history is read.
    pub task_id: String,
    /// Maximum entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Page offset into newest-first entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// Optional run identifier filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// Pinned `cron_trigger` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CronTriggerCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Schedule to fire immediately.
    pub task_id: String,
}

/// Pinned `cron_update` payload.
#[allow(clippy::option_option)] // Three-state schema field requires absent/null/value.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CronUpdateCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Schedule to modify.
    pub task_id: String,
    /// Replacement display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Replacement description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Replacement conversation target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Replacement cron or interval form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// Replacement IANA timezone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Replacement recurrence flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurring: Option<bool>,
    /// Replacement prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Replacement one-shot timestamp; explicit null clears it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "three_state::deserialize"
    )]
    pub scheduled_for: Option<Option<String>>,
}

/// Pinned `cron_delete` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CronDeleteCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Schedule to remove.
    pub task_id: String,
}

/// Pinned `cron_delete_all` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CronDeleteAllCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Agent whose schedules are removed.
    pub agent_id: String,
}

/// The eight concrete schedule group commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum SchedulesCommand {
    /// Filtered schedule listing.
    #[serde(rename = "cron_list")]
    List(CronListCommand),
    /// Validated schedule creation.
    #[serde(rename = "cron_add")]
    Add(CronAddCommand),
    /// Single schedule fetch.
    #[serde(rename = "cron_get")]
    Get(CronGetCommand),
    /// Bounded run-history page.
    #[serde(rename = "cron_runs")]
    Runs(CronRunsCommand),
    /// Immediate fire through the scheduler firing path.
    #[serde(rename = "cron_trigger")]
    Trigger(CronTriggerCommand),
    /// Partial validated update.
    #[serde(rename = "cron_update")]
    Update(CronUpdateCommand),
    /// Single schedule removal.
    #[serde(rename = "cron_delete")]
    Delete(CronDeleteCommand),
    /// Whole-agent removal.
    #[serde(rename = "cron_delete_all")]
    DeleteAll(CronDeleteAllCommand),
}

/// One page of `cron_runs_response`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronRunsPageMessage {
    /// Newest-first entries inside this page.
    pub entries: Vec<RunLogEntry>,
    /// Total filtered entry count across pages.
    pub total: usize,
    /// Requested page offset.
    pub offset: usize,
    /// Effective page limit after clamping.
    pub limit: usize,
    /// Whether further pages remain.
    pub has_more: bool,
    /// Offset of the next page, or null at the tail.
    pub next_offset: Option<usize>,
}

/// Pinned `cron_list_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct CronListResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Matching canonical schedules.
    pub tasks: Vec<serde_json::Value>,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `cron_add_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct CronAddResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Created schedule, present on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<serde_json::Value>,
    /// Interval-rounding note, present when normalization adjusted the form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `cron_get_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct CronGetResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Whether the schedule exists.
    pub found: bool,
    /// Canonical schedule, or null when absent.
    pub task: Option<serde_json::Value>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `cron_runs_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct CronRunsResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Bounded page, present on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<CronRunsPageMessage>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `cron_trigger_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct CronTriggerResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Whether the enqueue succeeded.
    pub success: bool,
    /// Whether the schedule exists.
    pub found: bool,
    /// Re-read schedule, present when found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<serde_json::Value>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `cron_update_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct CronUpdateResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Updated schedule, present on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<serde_json::Value>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `cron_delete_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct CronDeleteResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Whether the schedule existed and was removed.
    pub found: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `cron_delete_all_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct CronDeleteAllResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Agent whose schedules were removed.
    pub agent_id: String,
    /// Number of removed schedules.
    pub deleted: u64,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `crons_updated` refresh notice.
#[allow(clippy::option_option)] // Absent/null/value mirror the pinned scope spread.
#[derive(Clone, Debug, Serialize)]
pub struct CronsUpdatedMessage {
    /// Notice instant in epoch milliseconds.
    pub timestamp: i64,
    /// Scope agent when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Scope conversation, including explicit null, when scoped by one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Option<String>>,
}

/// The ten outbound schedule group messages.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum SchedulesMessage {
    /// Listing result.
    #[serde(rename = "cron_list_response")]
    ListResponse(CronListResponseMessage),
    /// Creation result.
    #[serde(rename = "cron_add_response")]
    AddResponse(CronAddResponseMessage),
    /// Fetch result.
    #[serde(rename = "cron_get_response")]
    GetResponse(CronGetResponseMessage),
    /// Run-history page result.
    #[serde(rename = "cron_runs_response")]
    RunsResponse(CronRunsResponseMessage),
    /// Immediate-fire result.
    #[serde(rename = "cron_trigger_response")]
    TriggerResponse(CronTriggerResponseMessage),
    /// Update result.
    #[serde(rename = "cron_update_response")]
    UpdateResponse(CronUpdateResponseMessage),
    /// Removal result.
    #[serde(rename = "cron_delete_response")]
    DeleteResponse(CronDeleteResponseMessage),
    /// Whole-agent removal result.
    #[serde(rename = "cron_delete_all_response")]
    DeleteAllResponse(CronDeleteAllResponseMessage),
    /// Refresh notice emitted after every successful mutation.
    #[serde(rename = "crons_updated")]
    Updated(CronsUpdatedMessage),
}

/// Push callback delivering one outbound message to one connection.
pub type SchedulesForwarder =
    Arc<dyn Fn(ConnectionId, SchedulesMessage) -> Result<(), AppServerError> + Send + Sync>;

#[cfg(test)]
pub(crate) fn inert_forwarder() -> SchedulesForwarder {
    Arc::new(|_, _| Ok(()))
}

/// Absent/null/value wire decoding for pinned three-state fields.
///
/// Plain `Option<Option<T>>` cannot tell an explicit JSON `null` apart from
/// an omitted key; wrapping the inner option restores the third state.
mod three_state {
    use serde::{Deserialize, Deserializer};

    #[allow(clippy::option_option, reason = "three-state JSON presence contract")]
    pub(super) fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Ok(Some(Option::<T>::deserialize(deserializer)?))
    }
}

/// Canonical expression persisted in place of the raw command value.
#[derive(Clone)]
struct ExpressionResolution {
    cron: NonEmptyString,
    warning: Option<String>,
}

/// Validated wire values for one add or update, ready to persist.
#[allow(clippy::option_option)] // Three-state schema field requires absent/null/value.
#[derive(Clone)]
struct ValidatedFields {
    expression: Option<ExpressionResolution>,
    prompt: Option<NonEmptyString>,
    conversation_id: Option<ConversationId>,
    timezone: Option<IanaTimezone>,
    recurring: Option<bool>,
    scheduled_for: Option<Option<Timestamp>>,
}

/// Owned replacement fields for one update, applied under the store CAS.
struct UpdateFields {
    name: Option<String>,
    description: Option<String>,
    fields: ValidatedFields,
}

/// Rejection text for one add hitting the per-agent active hard limit.
fn active_limit_rejection(agent_id: &str, active: usize) -> String {
    format!(
        "Agent {agent_id} has {active} active schedules (max \
         {ACTIVE_SCHEDULES_PER_AGENT_MAX}). Delete some before adding more."
    )
}

/// Applies wire schedule commands to the Task 60 store and Task 61 firing path.
pub struct SchedulesBridge {
    paths: Arc<SidePaths>,
    pool: Arc<Semaphore>,
    #[cfg(test)]
    listener: Arc<Mutex<ListenerRuntime>>,
    scheduler: ScheduleScheduler,
    clock: Arc<dyn Clock + Send + Sync>,
    forward: SchedulesForwarder,
}

impl SchedulesBridge {
    /// Creates the production bridge over one canonical local storage root.
    ///
    /// # Errors
    /// Returns [`AppServerError::Config`] when the storage root cannot back a
    /// canonical side-store layout.
    pub fn new(
        forward: SchedulesForwarder,
        storage_dir: &Path,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, AppServerError> {
        let paths = SidePaths::new(storage_dir, None, [])
            .map_err(|_| AppServerError::Config("schedules store root invalid"))?;
        Self::compose(forward, Arc::new(paths), clock)
    }

    /// Composes a bridge from explicit parts (test seam).
    ///
    /// # Errors
    /// Propagates [`AppServerError::Config`] from the same preparation rules.
    pub fn compose(
        forward: SchedulesForwarder,
        paths: Arc<SidePaths>,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, AppServerError> {
        let listener = Arc::new(Mutex::new(ListenerRuntime::new()));
        let persistence: Arc<dyn SchedulePersistence> =
            Arc::new(ScheduleService::new(Arc::clone(&paths)));
        let scheduler = ScheduleScheduler::new(
            clock.clone(),
            persistence,
            Arc::clone(&listener),
            Arc::new(RandomIds),
        );
        Ok(Self {
            paths,
            pool: Arc::new(Semaphore::new(STORE_BLOCKING_PERMITS_MAX)),
            #[cfg(test)]
            listener,
            scheduler,
            clock,
            forward,
        })
    }

    /// Routes one decoded command in a detached task, like the pinned listener.
    pub fn handle(self: &Arc<Self>, connection: ConnectionId, command: &SchedulesCommand) {
        let this = Arc::clone(self);
        let command = command.clone();
        tokio::spawn(async move { this.apply(connection, &command).await });
    }

    /// Applies one command inline, emitting responses through the forwarder.
    pub async fn apply(&self, connection: ConnectionId, command: &SchedulesCommand) {
        match command {
            SchedulesCommand::List(payload) => self.list(connection, payload).await,
            SchedulesCommand::Add(payload) => self.add(connection, payload).await,
            SchedulesCommand::Get(payload) => self.get(connection, payload).await,
            SchedulesCommand::Runs(payload) => self.runs(connection, payload).await,
            SchedulesCommand::Trigger(payload) => self.trigger(connection, payload).await,
            SchedulesCommand::Update(payload) => self.update(connection, payload).await,
            SchedulesCommand::Delete(payload) => self.delete(connection, payload).await,
            SchedulesCommand::DeleteAll(payload) => self.delete_all(connection, payload).await,
        }
    }

    /// Registry this bridge resolves triggered schedules against (test seam).
    #[cfg(test)]
    pub(crate) fn listener(&self) -> &Arc<Mutex<ListenerRuntime>> {
        &self.listener
    }

    async fn list(&self, connection: ConnectionId, command: &CronListCommand) {
        let loaded = self
            .store(move |paths| loaded_or_empty(paths, LIST_FAILURE))
            .await;
        let Ok(loaded) = loaded else {
            return self.emit(connection, list_rejected(command, LIST_FAILURE.to_owned()));
        };
        let Some(file) = loaded.map(|(file, _)| file) else {
            return self.emit(
                connection,
                SchedulesMessage::ListResponse(CronListResponseMessage {
                    request_id: command.request_id.clone(),
                    tasks: Vec::new(),
                    success: true,
                    error: None,
                }),
            );
        };
        let tasks = file
            .tasks
            .iter()
            .filter(|task| {
                matches_filter(command.agent_id.as_deref(), Some(task.agent_id.as_str()))
            })
            .filter(|task| {
                matches_filter(
                    command.conversation_id.as_deref(),
                    Some(task.conversation_id.as_str()),
                )
            })
            .map(task_to_value)
            .collect();
        self.emit(
            connection,
            SchedulesMessage::ListResponse(CronListResponseMessage {
                request_id: command.request_id.clone(),
                tasks,
                success: true,
                error: None,
            }),
        );
    }

    async fn get(&self, connection: ConnectionId, command: &CronGetCommand) {
        let task_id = command.task_id.clone();
        let found = self
            .store(move |paths| find_task(paths, &task_id, GET_FAILURE))
            .await;
        match found {
            Ok(task) => self.emit(
                connection,
                SchedulesMessage::GetResponse(CronGetResponseMessage {
                    request_id: command.request_id.clone(),
                    success: true,
                    found: task.is_some(),
                    task: task.as_ref().map(task_to_value),
                    error: None,
                }),
            ),
            Err(message) => self.emit(
                connection,
                SchedulesMessage::GetResponse(CronGetResponseMessage {
                    request_id: command.request_id.clone(),
                    success: false,
                    found: false,
                    task: None,
                    error: Some(message),
                }),
            ),
        }
    }

    async fn runs(&self, connection: ConnectionId, command: &CronRunsCommand) {
        let task_id = command.task_id.clone();
        let entries = self
            .store(move |paths| read_run_log(paths, &task_id, RUNS_FAILURE))
            .await;
        let Ok(entries) = entries else {
            return self.emit(connection, runs_rejected(command, RUNS_FAILURE.to_owned()));
        };
        self.emit(
            connection,
            SchedulesMessage::RunsResponse(CronRunsResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                page: Some(bounded_page(entries, command)),
                error: None,
            }),
        );
    }
    async fn add(&self, connection: ConnectionId, command: &CronAddCommand) {
        let now = self.clock.now();
        let fields = match validate_fields(
            Some(command.cron.as_str()),
            Some(command.prompt.as_str()),
            command.conversation_id.clone(),
            command.timezone.as_deref(),
            None,
            command.scheduled_for.clone(),
        ) {
            Ok(fields) => fields,
            Err(message) => return self.emit(connection, add_rejected(command, message)),
        };
        let draft = match prepare_draft(command, &fields, now) {
            Ok(draft) => draft,
            Err(message) => return self.emit(connection, add_rejected(command, message)),
        };
        let mut draft = draft;
        draft.jitter_offset_ms = jitter_offset(&draft, now);
        let inserted = self
            .store(move |paths| insert_task(paths, draft, ADD_FAILURE))
            .await;
        match inserted {
            Ok(task) => {
                self.emit(
                    connection,
                    SchedulesMessage::AddResponse(CronAddResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        task: Some(task_to_value(&task)),
                        warning: fields.expression.and_then(|item| item.warning),
                        error: None,
                    }),
                );
                self.emit_snapshot(
                    connection,
                    task.agent_id.as_str().to_owned(),
                    Some(Some(task.conversation_id.as_str().to_owned())),
                );
            }
            Err(message) => self.emit(connection, add_rejected(command, message)),
        }
    }

    async fn update(&self, connection: ConnectionId, command: &CronUpdateCommand) {
        let fields = match validate_fields(
            command.cron.as_deref(),
            command.prompt.as_deref(),
            command.conversation_id.clone(),
            command.timezone.as_deref(),
            command.recurring,
            command.scheduled_for.clone(),
        ) {
            Ok(fields) => fields,
            Err(message) => {
                return self.emit(
                    connection,
                    SchedulesMessage::UpdateResponse(CronUpdateResponseMessage {
                        request_id: command.request_id.clone(),
                        success: false,
                        task: None,
                        error: Some(message),
                    }),
                );
            }
        };
        self.apply_update(connection, command, fields).await;
    }

    async fn apply_update(
        &self,
        connection: ConnectionId,
        command: &CronUpdateCommand,
        fields: ValidatedFields,
    ) {
        let update = UpdateFields {
            name: command.name.clone(),
            description: command.description.clone(),
            fields,
        };
        let task_id = command.task_id.clone();
        let updated = self
            .store(move |paths| {
                mutate_task(paths, &task_id, UPDATE_FAILURE, |task| {
                    apply_fields(task, &update);
                })
            })
            .await;
        match updated {
            Ok(Some(task)) => {
                self.emit(
                    connection,
                    SchedulesMessage::UpdateResponse(CronUpdateResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        task: Some(task_to_value(&task)),
                        error: None,
                    }),
                );
                self.emit_snapshot(
                    connection,
                    task.agent_id.as_str().to_owned(),
                    Some(Some(task.conversation_id.as_str().to_owned())),
                );
            }
            Ok(None) => self.emit(
                connection,
                SchedulesMessage::UpdateResponse(CronUpdateResponseMessage {
                    request_id: command.request_id.clone(),
                    success: false,
                    task: None,
                    error: Some(UPDATE_NOT_FOUND.to_owned()),
                }),
            ),
            Err(message) => self.emit(
                connection,
                SchedulesMessage::UpdateResponse(CronUpdateResponseMessage {
                    request_id: command.request_id.clone(),
                    success: false,
                    task: None,
                    error: Some(message),
                }),
            ),
        }
    }

    async fn delete(&self, connection: ConnectionId, command: &CronDeleteCommand) {
        let task_id = command.task_id.clone();
        let removed = self
            .store(move |paths| remove_task(paths, &task_id, DELETE_FAILURE))
            .await;
        match removed {
            Ok(Some(task)) => {
                self.emit(
                    connection,
                    SchedulesMessage::DeleteResponse(CronDeleteResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        found: true,
                        error: None,
                    }),
                );
                self.emit_snapshot(
                    connection,
                    task.agent_id.as_str().to_owned(),
                    Some(Some(task.conversation_id.as_str().to_owned())),
                );
            }
            Ok(None) => self.emit(
                connection,
                SchedulesMessage::DeleteResponse(CronDeleteResponseMessage {
                    request_id: command.request_id.clone(),
                    success: true,
                    found: false,
                    error: None,
                }),
            ),
            Err(message) => self.emit(
                connection,
                SchedulesMessage::DeleteResponse(CronDeleteResponseMessage {
                    request_id: command.request_id.clone(),
                    success: false,
                    found: false,
                    error: Some(message),
                }),
            ),
        }
    }

    async fn delete_all(&self, connection: ConnectionId, command: &CronDeleteAllCommand) {
        let agent_id = command.agent_id.clone();
        let deleted = self
            .store(move |paths| remove_agent_tasks(paths, &agent_id, DELETE_FAILURE))
            .await;
        let Ok(deleted) = deleted else {
            return self.emit(
                connection,
                SchedulesMessage::DeleteAllResponse(CronDeleteAllResponseMessage {
                    request_id: command.request_id.clone(),
                    success: false,
                    agent_id: command.agent_id.clone(),
                    deleted: 0,
                    error: Some(DELETE_FAILURE.to_owned()),
                }),
            );
        };
        self.emit(
            connection,
            SchedulesMessage::DeleteAllResponse(CronDeleteAllResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                agent_id: command.agent_id.clone(),
                deleted,
                error: None,
            }),
        );
        if deleted > 0 {
            self.emit_snapshot(connection, command.agent_id.clone(), None);
        }
    }

    async fn trigger(&self, connection: ConnectionId, command: &CronTriggerCommand) {
        let now = self.clock.now();
        let Ok(outcome) = self.scheduler.trigger(command.task_id.as_str(), now).await else {
            return self.emit(
                connection,
                trigger_answer(
                    command,
                    false,
                    false,
                    None,
                    Some(TRIGGER_FAILURE.to_owned()),
                ),
            );
        };
        if !outcome.found {
            return self.emit(
                connection,
                trigger_answer(
                    command,
                    false,
                    false,
                    None,
                    Some(TRIGGER_NOT_FOUND.to_owned()),
                ),
            );
        }
        let task = self.reload(command.task_id.as_str()).await;
        if !outcome.active {
            return self.emit(
                connection,
                trigger_answer(
                    command,
                    false,
                    true,
                    task,
                    Some(TRIGGER_INACTIVE.to_owned()),
                ),
            );
        }
        self.emit(connection, trigger_answer(command, true, true, task, None));
    }

    async fn reload(&self, task_id: &str) -> Option<serde_json::Value> {
        let owned = task_id.to_owned();
        let found = self
            .store(move |paths| find_task(paths, &owned, TRIGGER_FAILURE))
            .await
            .ok()
            .flatten();
        found.as_ref().map(task_to_value)
    }

    #[allow(clippy::option_option)] // Pinned scope spread distinguishes absent from null.
    fn emit_snapshot(
        &self,
        connection: ConnectionId,
        agent_id: String,
        conversation_id: Option<Option<String>>,
    ) {
        self.emit(
            connection,
            SchedulesMessage::Updated(CronsUpdatedMessage {
                timestamp: self.clock.now().as_utc().timestamp_millis(),
                agent_id: Some(agent_id),
                conversation_id,
            }),
        );
    }

    fn emit(&self, connection: ConnectionId, message: SchedulesMessage) {
        match serde_json::to_string(&message) {
            Ok(body) if body.len() <= WS_FRAME_BYTES_MAX => {
                let _ = (self.forward)(connection, message);
            }
            Ok(_) => tracing::warn!("schedules response exceeded the frame bound and was dropped"),
            Err(_) => tracing::warn!("schedules response failed to encode"),
        }
    }

    /// Runs one blocking store operation under the bounded local pool.
    async fn store<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&SidePaths) -> Result<T, String> + Send + 'static,
    {
        let Ok(permit) = Arc::clone(&self.pool).acquire_owned().await else {
            return Err(STORE_UNAVAILABLE.to_owned());
        };
        let paths = Arc::clone(&self.paths);
        let joined = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(&paths)
        })
        .await;
        match joined {
            Ok(result) => result,
            Err(_) => Err(STORE_UNAVAILABLE.to_owned()),
        }
    }
}

/// Serializes one canonical schedule for the wire.
fn task_to_value(task: &Schedule) -> serde_json::Value {
    serde_json::to_value(task).unwrap_or(serde_json::Value::Null)
}

/// Returns whether an unset filter matches everything or the exact value.
fn matches_filter(filter: Option<&str>, value: Option<&str>) -> bool {
    filter.is_none_or(|wanted| value.is_some_and(|found| found == wanted))
}

/// Builds the newest-first bounded page for one runs request.
///
/// Entries past the frame ceiling are truncated away so the encoded response
/// always forwards, mirroring the files-group transport discipline.
fn bounded_page(entries: Vec<RunLogEntry>, command: &CronRunsCommand) -> CronRunsPageMessage {
    let limit = command
        .limit
        .unwrap_or(RUNS_PAGE_ENTRIES_DEFAULT)
        .clamp(1, RUNS_PAGE_ENTRIES_MAX);
    let offset = command.offset.unwrap_or(0);
    let mut filtered: Vec<RunLogEntry> = entries
        .into_iter()
        .filter(|entry| matches_filter(command.run_id.as_deref(), entry.run_id.as_deref()))
        .collect();
    if filtered.len() > RUNS_SCAN_ENTRIES_MAX {
        filtered.truncate(RUNS_SCAN_ENTRIES_MAX);
    }
    filtered.sort_by_key(|entry| std::cmp::Reverse(entry.ts));
    let total = filtered.len();
    let paged: Vec<RunLogEntry> = filtered.into_iter().skip(offset).take(limit).collect();
    fit_frame(&paged, total, offset, limit)
}

/// Shrinks one page until its encoded response respects the frame ceiling.
fn fit_frame(
    entries: &[RunLogEntry],
    total: usize,
    offset: usize,
    limit: usize,
) -> CronRunsPageMessage {
    let mut kept = entries.len();
    loop {
        let next_offset = offset.saturating_add(kept);
        let has_more = next_offset < total;
        let page = CronRunsPageMessage {
            entries: entries[..kept].to_vec(),
            total,
            offset,
            limit,
            has_more,
            next_offset: has_more.then_some(next_offset),
        };
        let encoded = match serde_json::to_string(&page) {
            Ok(body) => body.len(),
            Err(_) => 0,
        };
        if encoded <= WS_FRAME_BYTES_MAX || kept == 0 {
            return page;
        }
        kept -= 1;
    }
}

/// Validates every supplied mutation field before any store access.
#[allow(clippy::option_option)] // Three-state schema field requires absent/null/value.
fn validate_fields(
    cron: Option<&str>,
    prompt: Option<&str>,
    conversation_id: Option<String>,
    timezone: Option<&str>,
    recurring: Option<bool>,
    scheduled_for: Option<Option<String>>,
) -> Result<ValidatedFields, String> {
    let expression = match cron {
        Some(raw) => Some(validate_expression(raw)?),
        None => None,
    };
    let prompt = match prompt {
        Some(text) => Some(NonEmptyString::new(text).map_err(|_| INVALID_PROMPT.to_owned())?),
        None => None,
    };
    let conversation_id = match conversation_id {
        Some(value) => {
            Some(ConversationId::accept(value).map_err(|_| INVALID_CONVERSATION.to_owned())?)
        }
        None => None,
    };
    let timezone = match timezone {
        Some(value) => Some(IanaTimezone::new(value).map_err(|_| INVALID_TIMEZONE.to_owned())?),
        None => None,
    };
    let scheduled_for = match scheduled_for {
        None => None,
        Some(inner) => Some(parse_scheduled_for(inner)?),
    };
    Ok(ValidatedFields {
        expression,
        prompt,
        conversation_id,
        timezone,
        recurring,
        scheduled_for,
    })
}

/// Parses one three-state one-shot timestamp field.
fn parse_scheduled_for(raw: Option<String>) -> Result<Option<Timestamp>, String> {
    match raw {
        None => Ok(None),
        Some(text) => Timestamp::parse_persisted_rfc3339(text.as_str())
            .map(Some)
            .map_err(|_| INVALID_SCHEDULED_FOR.to_owned()),
    }
}

/// Accepts a five-field cron verbatim or normalizes an interval form.
fn validate_expression(raw: &str) -> Result<ExpressionResolution, String> {
    if ScheduleExpression::parse(raw).is_ok() {
        let cron = NonEmptyString::new(raw).map_err(|_| invalid_expression(raw))?;
        return Ok(ExpressionResolution {
            cron,
            warning: None,
        });
    }
    let interval = parse_interval(raw).ok_or_else(|| invalid_expression(raw))?;
    let cron = NonEmptyString::new(interval.cron).map_err(|_| invalid_expression(raw))?;
    Ok(ExpressionResolution {
        cron,
        warning: interval.note,
    })
}

/// Builds the pinned invalid-expression rejection text.
fn invalid_expression(raw: &str) -> String {
    format!("Invalid cron expression \"{raw}\". Schedule was not saved.")
}

/// Assembles a validated active schedule draft for one add.
fn prepare_draft(
    command: &CronAddCommand,
    fields: &ValidatedFields,
    now: Timestamp,
) -> Result<Schedule, String> {
    let agent = AgentId::accept(command.agent_id.clone()).map_err(|_| INVALID_AGENT.to_owned())?;
    let conversation = fields
        .conversation_id
        .clone()
        .or_else(|| ConversationId::accept(NEW_CONVERSATION).ok())
        .ok_or_else(|| INVALID_CONVERSATION.to_owned())?;
    let prompt = fields.prompt.clone().ok_or(INVALID_PROMPT.to_owned())?;
    let expression = fields
        .expression
        .clone()
        .ok_or_else(|| invalid_expression(command.cron.as_str()))?;
    let timezone = fields
        .timezone
        .clone()
        .or_else(|| IanaTimezone::new(DEFAULT_TIMEZONE).ok())
        .ok_or_else(|| INVALID_TIMEZONE.to_owned())?
        .to_string();
    let scheduled_for = fields
        .scheduled_for
        .flatten()
        .map(|instant| instant.to_string());
    let inputs = DraftInputs {
        agent,
        conversation,
        prompt,
        expression,
        timezone,
        scheduled_for,
    };
    build_draft(command, inputs, now)
}

/// Every resolved input of one draft schedule, ready for construction.
struct DraftInputs {
    agent: AgentId,
    conversation: ConversationId,
    prompt: NonEmptyString,
    expression: ExpressionResolution,
    /// Validated canonical IANA identifier.
    timezone: String,
    /// Serialized one-shot instant, or none.
    scheduled_for: Option<String>,
}

/// Constructs the canonical draft entity once every input is typed.
fn build_draft(
    command: &CronAddCommand,
    inputs: DraftInputs,
    now: Timestamp,
) -> Result<Schedule, String> {
    let DraftInputs {
        agent,
        conversation,
        prompt,
        expression,
        timezone,
        scheduled_for,
    } = inputs;
    let Ok(identifier) = random_schedule_id() else {
        return Err(ADD_FAILURE.to_owned());
    };
    serde_json::from_value(serde_json::json!({
        "id": identifier,
        "agent_id": agent.as_str(),
        "conversation_id": conversation.as_str(),
        "name": command.name,
        "description": command.description,
        "cron": expression.cron.as_str(),
        "timezone": timezone,
        "recurring": command.recurring,
        "prompt": prompt.as_str(),
        "status": "active",
        "created_at": now.to_string(),
        "expires_at": null,
        "last_fired_at": null,
        "fire_count": 0,
        "cancel_reason": null,
        "jitter_offset_ms": 0,
        "last_run_at": null,
        "last_run_outcome": null,
        "last_run_reason": null,
        "last_run_error": null,
        "last_missed_at": null,
        "missed_count": 0,
        "failed_count": 0,
        "scheduled_for": scheduled_for,
        "fired_at": null,
        "missed_at": null,
    }))
    .map_err(|_| ADD_FAILURE.to_owned())
}

/// Applies validated replacement fields onto one live schedule record.
fn apply_fields(task: &mut Schedule, update: &UpdateFields) {
    if let Some(name) = &update.name {
        task.name.clone_from(name);
    }
    if let Some(description) = &update.description {
        task.description.clone_from(description);
    }
    if let Some(conversation_id) = &update.fields.conversation_id {
        task.conversation_id = conversation_id.clone();
    }
    if let Some(expression) = &update.fields.expression {
        task.cron = expression.cron.clone();
    }
    if let Some(timezone) = &update.fields.timezone {
        task.timezone = timezone.clone();
    }
    if let Some(recurring) = update.fields.recurring {
        task.recurring = recurring;
    }
    if let Some(prompt) = &update.fields.prompt {
        task.prompt = prompt.clone();
    }
    if let Some(scheduled_for) = update.fields.scheduled_for {
        task.scheduled_for = scheduled_for;
    }
}

/// Draws one pinned-length hexadecimal schedule identifier.
fn random_schedule_id() -> Result<String, AppServerError> {
    const HEX_DIGITS: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    let mut bytes = [0_u8; SCHEDULE_ID_BYTES_MAX];
    getrandom::fill(&mut bytes).map_err(|_| AppServerError::Internal)?;
    let mut identifier = String::with_capacity(SCHEDULE_ID_BYTES_MAX * 2);
    for byte in bytes {
        identifier.push(HEX_DIGITS[usize::from(byte >> 4)]);
        identifier.push(HEX_DIGITS[usize::from(byte & 0x0f)]);
    }
    Ok(identifier)
}

/// Operating-system entropy backing schedule jitter.
struct OsEntropy;

impl JitterSource for OsEntropy {
    fn next_u64(&mut self) -> Result<u64, RuntimeError> {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes).map_err(|_| RuntimeError::Unsupported {
            context: "schedule jitter entropy".into(),
        })?;
        Ok(u64::from_le_bytes(bytes))
    }
}

/// Computes the persisted jitter offset for one draft, degrading to zero.
fn jitter_offset(schedule: &Schedule, now: Timestamp) -> i64 {
    let fire_time = match schedule.scheduled_for {
        Some(instant) => *instant.as_utc(),
        None => *now.as_utc(),
    };
    match compute_jitter(schedule, fire_time, &mut OsEntropy) {
        Ok(offset) => offset,
        Err(error) => {
            tracing::warn!(error = %error, "schedule jitter unavailable; storing zero");
            0
        }
    }
}

/// Registry identity source supplying only turn-lifecycle owner UUIDs.
struct RandomIds;

impl IdGenerator for RandomIds {
    fn agent_id(&self) -> PortFuture<'_, AgentId> {
        unsupported_id()
    }

    fn conversation_id(&self) -> PortFuture<'_, ConversationId> {
        unsupported_id()
    }

    fn message_id(&self) -> PortFuture<'_, lotta_domain::MessageId> {
        unsupported_id()
    }

    fn run_id(&self) -> PortFuture<'_, lotta_domain::RunId> {
        unsupported_id()
    }

    fn incident_id(&self) -> PortFuture<'_, uuid::Uuid> {
        unsupported_id()
    }

    fn turn_lifecycle_owner_id(&self) -> PortFuture<'_, uuid::Uuid> {
        Box::pin(async {
            let mut bytes = [0_u8; 16];
            getrandom::fill(&mut bytes).map_err(|_| RuntimeError::Unsupported {
                context: "turn lifecycle owner entropy".into(),
            })?;
            bytes[6] = (bytes[6] & 0x0f) | UUID_VERSION_4_BITS;
            bytes[8] = (bytes[8] & 0x3f) | UUID_VARIANT_BITS;
            Ok(uuid::Uuid::from_bytes(bytes))
        })
    }
}

/// RFC 4122 version nibble marking a random UUID.
const UUID_VERSION_4_BITS: u8 = 0x40;
/// RFC 4122 variant bits marking an RFC-compatible UUID.
const UUID_VARIANT_BITS: u8 = 0x80;

fn unsupported_id<T>() -> PortFuture<'static, T> {
    Box::pin(async {
        Err(RuntimeError::Unsupported {
            context: "schedules bridge".into(),
        })
    })
}

/// Loads the canonical file, creating an empty one when absent.
fn canonical_loaded(
    paths: &SidePaths,
    failure: &str,
) -> Result<(ScheduleFile, ScheduleRevision), String> {
    if let Some(loaded) = loaded_or_empty(paths, failure)? {
        return Ok(loaded);
    }
    // ponytail: first creation races unlocked where the baseline held
    // crons.lock; later writers stay CAS-guarded, add per-root locking
    // if multi-writer creation ever matters.
    let bytes = empty_canonical().encode().map_err(|_| failure.to_owned())?;
    lotta_store::side::crons::write(paths, &bytes).map_err(|_| failure.to_owned())?;
    let store = ScheduleStore::new(paths);
    let created = store.load().map_err(|_| failure.to_owned())?;
    Ok((created.file, created.revision))
}

/// Loads the canonical file without creating it; `None` marks absence.
fn loaded_or_empty(
    paths: &SidePaths,
    failure: &str,
) -> Result<Option<(ScheduleFile, ScheduleRevision)>, String> {
    let store = ScheduleStore::new(paths);
    match store.load() {
        Ok(loaded) => Ok(Some((loaded.file, loaded.revision))),
        Err(error) if error.kind() == StoreErrorKind::NotFound => Ok(None),
        Err(_) => Err(failure.to_owned()),
    }
}

/// Empty canonical `crons.json` contents.
fn empty_canonical() -> ScheduleFile {
    ScheduleFile {
        version: 1,
        scheduler_owner: None,
        tasks: Vec::new(),
        extras: serde_json::Map::new(),
    }
}

/// Finds one canonical schedule without creating anything.
fn find_task(paths: &SidePaths, task_id: &str, failure: &str) -> Result<Option<Schedule>, String> {
    let store = ScheduleStore::new(paths);
    match store.load() {
        Ok(loaded) => Ok(loaded
            .file
            .tasks
            .into_iter()
            .find(|task| task.id.as_str() == task_id)),
        Err(error) if error.kind() == StoreErrorKind::NotFound => Ok(None),
        Err(_) => Err(failure.to_owned()),
    }
}

/// Reads one bounded per-schedule run history.
fn read_run_log(
    paths: &SidePaths,
    task_id: &str,
    failure: &str,
) -> Result<Vec<RunLogEntry>, String> {
    RunLogStore::new(paths)
        .read(task_id)
        .map_err(|_| failure.to_owned())
}

/// Inserts one validated draft under the per-agent active hard limit.
fn insert_task(paths: &SidePaths, task: Schedule, failure: &str) -> Result<Schedule, String> {
    let (mut file, revision) = canonical_loaded(paths, failure)?;
    let agent_id = task.agent_id.as_str();
    let active = file
        .tasks
        .iter()
        .filter(|existing| existing.agent_id == task.agent_id)
        .filter(|existing| existing.status == ScheduleStatus::Active)
        .count();
    if active >= ACTIVE_SCHEDULES_PER_AGENT_MAX {
        return Err(active_limit_rejection(agent_id, active));
    }
    file.tasks.push(task.clone());
    ScheduleStore::new(paths)
        .save(&file, &revision)
        .map_err(|_| failure.to_owned())?;
    Ok(task)
}

/// Applies one change to exactly one schedule and persists it with CAS.
fn mutate_task(
    paths: &SidePaths,
    task_id: &str,
    failure: &str,
    change: impl FnOnce(&mut Schedule),
) -> Result<Option<Schedule>, String> {
    let Some((mut file, revision)) = loaded_or_empty(paths, failure)? else {
        return Ok(None);
    };
    let Some(position) = file
        .tasks
        .iter()
        .position(|task| task.id.as_str() == task_id)
    else {
        return Ok(None);
    };
    change(&mut file.tasks[position]);
    ScheduleStore::new(paths)
        .save(&file, &revision)
        .map_err(|_| failure.to_owned())?;
    Ok(file.tasks.into_iter().nth(position))
}

/// Removes exactly one schedule, returning the removed record.
fn remove_task(
    paths: &SidePaths,
    task_id: &str,
    failure: &str,
) -> Result<Option<Schedule>, String> {
    let Some((mut file, revision)) = loaded_or_empty(paths, failure)? else {
        return Ok(None);
    };
    let Some(position) = file
        .tasks
        .iter()
        .position(|task| task.id.as_str() == task_id)
    else {
        return Ok(None);
    };
    let removed = file.tasks.remove(position);
    ScheduleStore::new(paths)
        .save(&file, &revision)
        .map_err(|_| failure.to_owned())?;
    Ok(Some(removed))
}

/// Removes every schedule owned by one agent, writing only on removal.
fn remove_agent_tasks(paths: &SidePaths, agent_id: &str, failure: &str) -> Result<u64, String> {
    let agent = AgentId::accept(agent_id).map_err(|_| failure.to_owned())?;
    let Some((mut file, revision)) = loaded_or_empty(paths, failure)? else {
        return Ok(0);
    };
    let before = file.tasks.len();
    file.tasks.retain(|task| task.agent_id != agent);
    let removed = u64::try_from(before - file.tasks.len()).map_err(|_| failure.to_owned())?;
    if removed == 0 {
        return Ok(0);
    }
    ScheduleStore::new(paths)
        .save(&file, &revision)
        .map_err(|_| failure.to_owned())?;
    Ok(removed)
}

fn list_rejected(command: &CronListCommand, message: String) -> SchedulesMessage {
    SchedulesMessage::ListResponse(CronListResponseMessage {
        request_id: command.request_id.clone(),
        tasks: Vec::new(),
        success: false,
        error: Some(message),
    })
}

fn add_rejected(command: &CronAddCommand, message: String) -> SchedulesMessage {
    SchedulesMessage::AddResponse(CronAddResponseMessage {
        request_id: command.request_id.clone(),
        success: false,
        task: None,
        warning: None,
        error: Some(message),
    })
}

fn runs_rejected(command: &CronRunsCommand, message: String) -> SchedulesMessage {
    SchedulesMessage::RunsResponse(CronRunsResponseMessage {
        request_id: command.request_id.clone(),
        success: false,
        page: None,
        error: Some(message),
    })
}

fn trigger_answer(
    command: &CronTriggerCommand,
    success: bool,
    found: bool,
    task: Option<serde_json::Value>,
    error: Option<String>,
) -> SchedulesMessage {
    SchedulesMessage::TriggerResponse(CronTriggerResponseMessage {
        request_id: command.request_id.clone(),
        success,
        found,
        task,
        error,
    })
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known schedule commands,
/// including missing required fields or wrong-typed values.
pub fn decode(frame: &DecodedFrame) -> Result<Option<SchedulesCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let parsed = match tag {
        Tag::CronList => typed::<CronListCommand>(frame).map(SchedulesCommand::List),
        Tag::CronAdd => typed::<CronAddCommand>(frame).map(SchedulesCommand::Add),
        Tag::CronGet => typed::<CronGetCommand>(frame).map(SchedulesCommand::Get),
        Tag::CronRuns => typed::<CronRunsCommand>(frame).map(SchedulesCommand::Runs),
        Tag::CronTrigger => typed::<CronTriggerCommand>(frame).map(SchedulesCommand::Trigger),
        Tag::CronUpdate => typed::<CronUpdateCommand>(frame).map(SchedulesCommand::Update),
        Tag::CronDelete => typed::<CronDeleteCommand>(frame).map(SchedulesCommand::Delete),
        Tag::CronDeleteAll => typed::<CronDeleteAllCommand>(frame).map(SchedulesCommand::DeleteAll),
        _ => return Ok(None),
    }?;
    Ok(Some(parsed))
}

fn typed<T: DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ProtocolErrorEnvelope> {
    serde_json::from_value(frame.value.clone()).map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "schedules_command_invalid",
        "invalid schedules command",
        frame.request_id.clone(),
    )
}

#[cfg(test)]
#[path = "schedules_commands_tests.rs"]
mod commands;
#[cfg(test)]
#[path = "schedules_fixture_round_trip_tests.rs"]
mod fixture_round_trip;
#[cfg(test)]
#[path = "schedules_runs_and_snapshots_tests.rs"]
mod runs_and_snapshots;
#[cfg(test)]
#[path = "schedules_support.rs"]
mod support;
#[cfg(test)]
#[path = "schedules_trigger_enqueues_tests.rs"]
mod trigger_enqueues;
#[cfg(test)]
#[path = "schedules_validation_tests.rs"]
mod validation;
