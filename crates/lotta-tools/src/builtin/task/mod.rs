//! Conversation-scoped task lifecycle tools.

use crate::{
    builtin::common,
    pipeline::{
        ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
    },
    registry::ToolRegistration,
};
use lotta_runtime::ports::{ToolApprovalPolicy, ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;

/// Maximum retained task records.
pub const TASKS_ITEMS_MAX: usize = 256;
/// Maximum bytes in one task text or identifier field.
pub const TASK_TEXT_BYTES_MAX: usize = 16 * 1024;
/// Maximum dependency edges per task.
pub const TASK_DEPENDENCIES_ITEMS_MAX: usize = 256;
/// Maximum metadata properties per task.
pub const TASK_METADATA_ITEMS_MAX: usize = 256;
/// Maximum serialized state bytes.
pub const TASK_STATE_BYTES_MAX: usize = 1024 * 1024;

/// Pinned task status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Not started.
    Pending,
    /// Started.
    InProgress,
    /// Completed.
    Completed,
    /// Deleted on update return.
    Deleted,
}

/// Pinned task lifecycle record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskRecord {
    /// Stable task identifier.
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// Brief subject.
    pub subject: String,
    /// Full description.
    pub description: String,
    /// Present-continuous description.
    #[serde(rename = "activeForm", skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// Current status.
    pub status: TaskStatus,
    /// Optional owner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// IDs this task blocks.
    pub blocks: Vec<String>,
    /// IDs blocking this task.
    #[serde(rename = "blockedBy")]
    pub blocked_by: Vec<String>,
    /// Free-form bounded metadata.
    pub metadata: Map<String, Value>,
    /// Deterministic creation sequence.
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    /// Deterministic update sequence.
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
}

#[derive(Clone, Default)]
struct TaskState {
    records: BTreeMap<String, TaskRecord>,
    order: Vec<String>,
    next_id: u64,
    revision: u64,
}

/// Typed explicit task lifecycle port.
pub struct TaskLifecyclePort {
    state: Mutex<TaskState>,
}

impl TaskLifecyclePort {
    /// Creates isolated conversation task state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(TaskState {
                next_id: 1,
                revision: 1,
                ..TaskState::default()
            }),
        }
    }

    async fn create(&self, input: CreateInput) -> Result<TaskRecord, TaskError> {
        validate_create(&input)?;
        let mut state = self.state.lock().await;
        let mut candidate = state.clone();
        if candidate.records.len() >= TASKS_ITEMS_MAX {
            return Err(TaskError::Limit);
        }
        let id = format!("task_{}", candidate.next_id);
        candidate.next_id = candidate.next_id.checked_add(1).ok_or(TaskError::Limit)?;
        let revision = next_revision(&mut candidate)?;
        let record = TaskRecord {
            task_id: id.clone(),
            subject: input.subject,
            description: input.description,
            active_form: input.active_form,
            status: TaskStatus::Pending,
            owner: None,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: input.metadata.unwrap_or_default(),
            created_at: revision,
            updated_at: revision,
        };
        candidate.order.push(id.clone());
        candidate.records.insert(id, record.clone());
        check_state(&candidate)?;
        *state = candidate;
        Ok(record)
    }

    async fn get(&self, task_id: &str) -> Result<TaskRecord, TaskError> {
        validate_text(task_id)?;
        self.state
            .lock()
            .await
            .records
            .get(task_id)
            .cloned()
            .ok_or(TaskError::Unknown)
    }

    async fn list(&self) -> Result<Vec<TaskRecord>, TaskError> {
        let state = self.state.lock().await;
        let mut output = Vec::new();
        output
            .try_reserve_exact(state.order.len())
            .map_err(|_| TaskError::Limit)?;
        for id in &state.order {
            output.push(state.records.get(id).cloned().ok_or(TaskError::Invalid)?);
        }
        Ok(output)
    }

    async fn update(&self, input: UpdateInput) -> Result<TaskRecord, TaskError> {
        validate_update(&input)?;
        let mut state = self.state.lock().await;
        let mut candidate = state.clone();
        validate_dependencies(&candidate, &input)?;
        let revision = next_revision(&mut candidate)?;
        let old = candidate
            .records
            .get(&input.task_id)
            .cloned()
            .ok_or(TaskError::Unknown)?;
        let mut output = old.clone();
        apply_update(&mut output, &input, revision)?;
        if output.status == TaskStatus::Deleted {
            delete_record(&mut candidate, &output);
        } else {
            candidate
                .records
                .insert(input.task_id.clone(), output.clone());
            sync_reciprocal_edges(&mut candidate, &old, &output, revision)?;
        }
        check_state(&candidate)?;
        *state = candidate;
        Ok(output)
    }
}

impl Default for TaskLifecyclePort {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct CreateInput {
    subject: String,
    description: String,
    #[serde(default, rename = "activeForm")]
    active_form: Option<String>,
    #[serde(default)]
    metadata: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
struct IdInput {
    #[serde(rename = "taskId")]
    task_id: String,
}

#[derive(Deserialize)]
struct UpdateInput {
    #[serde(rename = "taskId")]
    task_id: String,
    #[serde(default)]
    status: Option<TaskStatus>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "activeForm")]
    active_form: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default, rename = "addBlocks")]
    add_blocks: Vec<String>,
    #[serde(default, rename = "addBlockedBy")]
    add_blocked_by: Vec<String>,
    #[serde(default)]
    metadata: Option<Map<String, Value>>,
}

/// Fixed lifecycle error without retaining attacker payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskError {
    /// Invalid input or transition.
    Invalid,
    /// Unknown task identifier.
    Unknown,
    /// Named resource bound reached.
    Limit,
}

struct TaskExecutor {
    port: Arc<TaskLifecyclePort>,
}

impl ToolExecutor for TaskExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let name = request.definition.internal_name.as_str().to_owned();
        let value = request.input.as_value().clone();
        let port = Arc::clone(&self.port);
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return failure("interrupted", "Task operation interrupted.");
            }
            let result = execute_operation(&port, &name, value).await;
            match result {
                Ok(value) => Ok(RawToolOutcome::Success(value)),
                Err(TaskError::Unknown) => failure("unknown_task", "Task not found."),
                Err(TaskError::Invalid) => failure("invalid_task", "Task operation is invalid."),
                Err(TaskError::Limit) => failure("task_limit", "Task resource limit reached."),
            }
        })
    }
}

async fn execute_operation(
    port: &TaskLifecyclePort,
    name: &str,
    value: Value,
) -> Result<String, TaskError> {
    match name {
        "TaskCreate" => encode(&port.create(decode(value)?).await?),
        "TaskGet" => {
            let input: IdInput = decode(value)?;
            encode(&port.get(&input.task_id).await?)
        }
        "TaskList" => encode(&json!({"tasks": port.list().await?})),
        "TaskUpdate" => encode(&port.update(decode(value)?).await?),
        _ => Err(TaskError::Invalid),
    }
}

fn apply_update(
    record: &mut TaskRecord,
    input: &UpdateInput,
    revision: u64,
) -> Result<(), TaskError> {
    if let Some(status) = input.status {
        validate_transition(record.status, status)?;
        record.status = status;
    }
    replace_if_some(&mut record.subject, input.subject.as_ref());
    replace_if_some(&mut record.description, input.description.as_ref());
    if let Some(value) = &input.active_form {
        record.active_form = Some(value.clone());
    }
    if let Some(value) = &input.owner {
        record.owner = Some(value.clone());
    }
    append_unique(&mut record.blocks, &input.add_blocks)?;
    append_unique(&mut record.blocked_by, &input.add_blocked_by)?;
    if let Some(metadata) = &input.metadata {
        for (key, value) in metadata {
            if value.is_null() {
                record.metadata.remove(key);
            } else {
                record.metadata.insert(key.clone(), value.clone());
            }
        }
    }
    record.updated_at = revision;
    Ok(())
}

fn sync_reciprocal_edges(
    state: &mut TaskState,
    old: &TaskRecord,
    new: &TaskRecord,
    revision: u64,
) -> Result<(), TaskError> {
    sync_inverse(
        state,
        &new.task_id,
        &old.blocks,
        &new.blocks,
        |record| &mut record.blocked_by,
        revision,
    )?;
    sync_inverse(
        state,
        &new.task_id,
        &old.blocked_by,
        &new.blocked_by,
        |record| &mut record.blocks,
        revision,
    )
}

fn sync_inverse(
    state: &mut TaskState,
    source_id: &str,
    old: &[String],
    new: &[String],
    field: impl Fn(&mut TaskRecord) -> &mut Vec<String>,
    revision: u64,
) -> Result<(), TaskError> {
    for target_id in old.iter().filter(|id| !new.contains(id)) {
        let target = state.records.get_mut(target_id).ok_or(TaskError::Invalid)?;
        field(target).retain(|id| id != source_id);
        target.updated_at = revision;
    }
    for target_id in new.iter().filter(|id| !old.contains(id)) {
        let target = state.records.get_mut(target_id).ok_or(TaskError::Invalid)?;
        append_unique(field(target), &[source_id.to_owned()])?;
        target.updated_at = revision;
    }
    Ok(())
}

fn delete_record(state: &mut TaskState, record: &TaskRecord) {
    state.records.remove(&record.task_id);
    state.order.retain(|id| id != &record.task_id);
    for other in state.records.values_mut() {
        let before = (other.blocks.len(), other.blocked_by.len());
        other.blocks.retain(|id| id != &record.task_id);
        other.blocked_by.retain(|id| id != &record.task_id);
        if before != (other.blocks.len(), other.blocked_by.len()) {
            other.updated_at = record.updated_at;
        }
    }
}

fn validate_transition(current: TaskStatus, target: TaskStatus) -> Result<(), TaskError> {
    let valid = current == target
        || matches!(
            (current, target),
            (
                TaskStatus::Pending,
                TaskStatus::InProgress | TaskStatus::Deleted
            ) | (
                TaskStatus::InProgress,
                TaskStatus::Completed | TaskStatus::Deleted
            ) | (TaskStatus::Completed, TaskStatus::Deleted)
        );
    valid.then_some(()).ok_or(TaskError::Invalid)
}

fn validate_create(input: &CreateInput) -> Result<(), TaskError> {
    validate_text(&input.subject)?;
    validate_text(&input.description)?;
    if let Some(value) = &input.active_form {
        validate_text(value)?;
    }
    if let Some(metadata) = &input.metadata {
        validate_metadata(metadata, true)?;
    }
    Ok(())
}

fn validate_update(input: &UpdateInput) -> Result<(), TaskError> {
    validate_text(&input.task_id)?;
    for value in [
        &input.subject,
        &input.description,
        &input.active_form,
        &input.owner,
    ]
    .into_iter()
    .flatten()
    {
        validate_text(value)?;
    }
    validate_ids(&input.add_blocks)?;
    validate_ids(&input.add_blocked_by)?;
    if let Some(metadata) = &input.metadata {
        validate_metadata(metadata, false)?;
    }
    Ok(())
}

fn validate_dependencies(state: &TaskState, input: &UpdateInput) -> Result<(), TaskError> {
    for id in input.add_blocks.iter().chain(&input.add_blocked_by) {
        if id == &input.task_id || !state.records.contains_key(id) {
            return Err(TaskError::Invalid);
        }
    }
    Ok(())
}

fn validate_ids(values: &[String]) -> Result<(), TaskError> {
    if values.len() > TASK_DEPENDENCIES_ITEMS_MAX {
        return Err(TaskError::Limit);
    }
    for value in values {
        validate_text(value)?;
    }
    Ok(())
}

fn validate_metadata(values: &Map<String, Value>, strings_only: bool) -> Result<(), TaskError> {
    if values.len() > TASK_METADATA_ITEMS_MAX {
        return Err(TaskError::Limit);
    }
    for (key, value) in values {
        validate_text(key)?;
        if strings_only && !value.is_string() {
            return Err(TaskError::Invalid);
        }
    }
    Ok(())
}

fn validate_text(value: &str) -> Result<(), TaskError> {
    if value.is_empty() || value.len() > TASK_TEXT_BYTES_MAX || value.contains('\0') {
        Err(TaskError::Invalid)
    } else {
        Ok(())
    }
}

fn append_unique(target: &mut Vec<String>, additions: &[String]) -> Result<(), TaskError> {
    for value in additions {
        if !target.contains(value) {
            if target.len() == TASK_DEPENDENCIES_ITEMS_MAX {
                return Err(TaskError::Limit);
            }
            target.push(value.clone());
        }
    }
    Ok(())
}

fn replace_if_some(target: &mut String, source: Option<&String>) {
    if let Some(value) = source {
        target.clone_from(value);
    }
}

fn next_revision(state: &mut TaskState) -> Result<u64, TaskError> {
    let current = state.revision;
    state.revision = state.revision.checked_add(1).ok_or(TaskError::Limit)?;
    Ok(current)
}

fn check_state(state: &TaskState) -> Result<(), TaskError> {
    if state.records.len() > TASKS_ITEMS_MAX || state.order.len() != state.records.len() {
        return Err(TaskError::Limit);
    }
    for (id, record) in &state.records {
        validate_complete_record(state, id, record)?;
    }
    if has_cycle(state) {
        return Err(TaskError::Invalid);
    }
    let bytes = serde_json::to_vec(&state.records)
        .map_err(|_| TaskError::Limit)?
        .len();
    (bytes <= TASK_STATE_BYTES_MAX)
        .then_some(())
        .ok_or(TaskError::Limit)
}

fn validate_complete_record(
    state: &TaskState,
    id: &str,
    record: &TaskRecord,
) -> Result<(), TaskError> {
    if id != record.task_id
        || record.blocks.len() > TASK_DEPENDENCIES_ITEMS_MAX
        || record.blocked_by.len() > TASK_DEPENDENCIES_ITEMS_MAX
    {
        return Err(TaskError::Limit);
    }
    for blocked in &record.blocks {
        let target = state.records.get(blocked).ok_or(TaskError::Invalid)?;
        if blocked == id || !target.blocked_by.contains(&record.task_id) {
            return Err(TaskError::Invalid);
        }
    }
    for blocker in &record.blocked_by {
        let target = state.records.get(blocker).ok_or(TaskError::Invalid)?;
        if blocker == id || !target.blocks.contains(&record.task_id) {
            return Err(TaskError::Invalid);
        }
    }
    Ok(())
}

fn has_cycle(state: &TaskState) -> bool {
    fn visit(state: &TaskState, id: &str, marks: &mut BTreeMap<String, u8>) -> bool {
        match marks.get(id) {
            Some(1) => return true,
            Some(2) => return false,
            _ => {}
        }
        marks.insert(id.to_owned(), 1);
        let cyclic = state
            .records
            .get(id)
            .is_some_and(|record| record.blocks.iter().any(|next| visit(state, next, marks)));
        marks.insert(id.to_owned(), 2);
        cyclic
    }
    let mut marks = BTreeMap::new();
    state.records.keys().any(|id| visit(state, id, &mut marks))
}

fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, TaskError> {
    serde_json::from_value(value).map_err(|_| TaskError::Invalid)
}
fn encode<T: Serialize>(value: &T) -> Result<String, TaskError> {
    serde_json::to_string(value).map_err(|_| TaskError::Limit)
}
fn failure(code: &str, message: &str) -> Result<RawToolOutcome, ExecutorError> {
    Ok(RawToolOutcome::Failure(ToolOutcome::ToolDefinedError {
        code: ToolOutcomeCode::new(code.to_owned()).map_err(|_| ExecutorError)?,
        message: ToolOutcomeMessage::new(message.to_owned()).map_err(|_| ExecutorError)?,
    }))
}

/// Builds the four task-record lifecycle registrations.
///
/// `TaskOutput` and `TaskStop` remain the canonical Task 38 registrations to avoid collisions.
///
/// # Errors
/// Rejects malformed pinned assets.
pub fn registrations(port: Arc<TaskLifecyclePort>) -> Result<Vec<ToolRegistration>, TaskError> {
    let executor: Arc<dyn ToolExecutor> = Arc::new(TaskExecutor { port });
    let mut output = Vec::new();
    for name in ["TaskCreate", "TaskGet", "TaskList", "TaskUpdate"] {
        output.push(
            common::registration(
                name,
                schema(name),
                description(name),
                ToolApprovalPolicy::Never,
                "task",
                Arc::clone(&executor),
            )
            .map_err(|()| TaskError::Invalid)?,
        );
    }
    output.push(common::registration(
        "Task",
        r#"{"type":"object","properties":{"description":{"type":"string"},"prompt":{"type":"string"}},"required":["description","prompt"],"additionalProperties":true}"#,
        "Start a bounded subagent task.",
        ToolApprovalPolicy::Never,
        "task",
        executor,
    ).map_err(|()| TaskError::Invalid)?);
    Ok(output)
}

fn schema(name: &str) -> &'static str {
    match name {
        "TaskCreate" => include_str!("assets/schemas/TaskCreate.json"),
        "TaskGet" => include_str!("assets/schemas/TaskGet.json"),
        "TaskList" => include_str!("assets/schemas/TaskList.json"),
        "TaskUpdate" => include_str!("assets/schemas/TaskUpdate.json"),
        _ => "",
    }
}
fn description(name: &str) -> &'static str {
    match name {
        "TaskCreate" => include_str!("assets/descriptions/TaskCreate.md"),
        "TaskGet" => include_str!("assets/descriptions/TaskGet.md"),
        "TaskList" => include_str!("assets/descriptions/TaskList.md"),
        "TaskUpdate" => include_str!("assets/descriptions/TaskUpdate.md"),
        _ => "",
    }
}

#[cfg(test)]
mod tests;
