//! Conversation-scoped planning and todo replacement tools.

use crate::{
    builtin::common,
    pipeline::{
        ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
    },
    registry::ToolRegistration,
};
use lotta_runtime::ports::ToolApprovalPolicy;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Maximum planning items retained in one conversation.
pub const PLANNING_ITEMS_MAX: usize = 256;
/// Maximum UTF-8 bytes in one planning text field.
pub const PLANNING_TEXT_BYTES_MAX: usize = 16 * 1024;
/// Maximum serialized planning-state bytes.
pub const PLANNING_STATE_BYTES_MAX: usize = 1024 * 1024;

/// One exact plan step.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanStep {
    /// Step text.
    pub step: String,
    /// Pinned status.
    pub status: PlanStatus,
}

/// Pinned plan status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// Not started.
    Pending,
    /// Currently executing.
    InProgress,
    /// Finished.
    Completed,
}

/// One exact todo item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TodoItem {
    /// Imperative description.
    pub content: String,
    /// Pinned status.
    pub status: PlanStatus,
    /// Present-continuous description.
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

#[derive(Default)]
struct PlanningState {
    plan: Vec<PlanStep>,
    explanation: Option<String>,
    todos: Vec<TodoItem>,
}

/// Explicit bounded per-conversation planning state port.
pub struct PlanningPort {
    state: Mutex<PlanningState>,
}

impl PlanningPort {
    /// Creates empty conversation planning state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(PlanningState::default()),
        }
    }

    fn replace_plan(&self, input: UpdatePlanInput) -> Result<(), PlanningError> {
        validate_plan(&input)?;
        let mut guard = self.state.lock().map_err(|_| PlanningError)?;
        guard.plan = input.plan;
        guard.explanation = input.explanation;
        Ok(())
    }

    fn replace_todos(&self, todos: Vec<TodoItem>) -> Result<(), PlanningError> {
        validate_todos(&todos)?;
        let mut guard = self.state.lock().map_err(|_| PlanningError)?;
        guard.todos = todos;
        Ok(())
    }

    /// Returns the exact current plan.
    ///
    /// # Errors
    /// Returns a fixed error if the conversation state lock is poisoned.
    pub fn plan(&self) -> Result<Vec<PlanStep>, PlanningError> {
        self.state
            .lock()
            .map(|state| state.plan.clone())
            .map_err(|_| PlanningError)
    }

    /// Returns the exact current todo list.
    ///
    /// # Errors
    /// Returns a fixed error if the conversation state lock is poisoned.
    pub fn todos(&self) -> Result<Vec<TodoItem>, PlanningError> {
        self.state
            .lock()
            .map(|state| state.todos.clone())
            .map_err(|_| PlanningError)
    }
}

impl Default for PlanningPort {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct UpdatePlanInput {
    #[serde(default)]
    explanation: Option<String>,
    plan: Vec<PlanStep>,
}

#[derive(Deserialize)]
struct TodoWriteInput {
    todos: Vec<TodoItem>,
}

/// Fixed planning failure without input retention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanningError;

struct PlanningExecutor {
    port: Arc<PlanningPort>,
}

impl ToolExecutor for PlanningExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let name = request.definition.internal_name.as_str().to_owned();
        let value = request.input.as_value().clone();
        let port = Arc::clone(&self.port);
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return interrupted();
            }
            let result = match name.as_str() {
                "update_plan" => serde_json::from_value::<UpdatePlanInput>(value)
                    .map_err(|_| PlanningError)
                    .and_then(|input| port.replace_plan(input))
                    .map(|()| "{\"message\":\"Plan updated\"}".to_owned()),
                "TodoWrite" => serde_json::from_value::<TodoWriteInput>(value)
                    .map_err(|_| PlanningError)
                    .and_then(|input| port.replace_todos(input.todos))
                    .map(|()| todo_message()),
                _ => Err(PlanningError),
            };
            result
                .map(RawToolOutcome::Success)
                .map_err(|_| ExecutorError)
        })
    }
}

fn interrupted() -> Result<RawToolOutcome, ExecutorError> {
    use lotta_runtime::ports::{ToolOutcome, ToolOutcomeMessage};
    let message = ToolOutcomeMessage::new("Planning tool interrupted.".to_owned())
        .map_err(|_| ExecutorError)?;
    Ok(RawToolOutcome::Failure(ToolOutcome::Interrupted {
        message,
    }))
}

fn validate_plan(input: &UpdatePlanInput) -> Result<(), PlanningError> {
    if input.plan.len() > PLANNING_ITEMS_MAX {
        return Err(PlanningError);
    }
    if input
        .explanation
        .as_ref()
        .is_some_and(|value| !valid_text(value))
        || input.plan.iter().any(|item| !valid_text(&item.step))
        || !one_active(input.plan.iter().map(|item| item.status))
    {
        return Err(PlanningError);
    }
    check_bytes(input)
}

fn validate_todos(items: &[TodoItem]) -> Result<(), PlanningError> {
    if items.len() > PLANNING_ITEMS_MAX
        || items
            .iter()
            .any(|item| !valid_text(&item.content) || !valid_text(&item.active_form))
        || !one_active(items.iter().map(|item| item.status))
    {
        return Err(PlanningError);
    }
    let bytes = serde_json::to_vec(items).map_err(|_| PlanningError)?.len();
    (bytes <= PLANNING_STATE_BYTES_MAX)
        .then_some(())
        .ok_or(PlanningError)
}

fn check_bytes(input: &UpdatePlanInput) -> Result<(), PlanningError> {
    let bytes = serde_json::to_vec(&(&input.explanation, &input.plan))
        .map_err(|_| PlanningError)?
        .len();
    (bytes <= PLANNING_STATE_BYTES_MAX)
        .then_some(())
        .ok_or(PlanningError)
}

fn one_active(statuses: impl Iterator<Item = PlanStatus>) -> bool {
    statuses
        .filter(|status| *status == PlanStatus::InProgress)
        .count()
        <= 1
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= PLANNING_TEXT_BYTES_MAX && !value.contains('\0')
}

fn todo_message() -> String {
    concat!(
        "{\"message\":\"Todos have been modified successfully. Ensure that you continue to ",
        "use the todo list to track your progress. Please proceed with the current tasks if ",
        "applicable\"}"
    )
    .to_owned()
}

/// Builds the canonical shared implementation for `update_plan`/`UpdatePlan` and `TodoWrite`.
///
/// # Errors
/// Rejects malformed pinned assets.
pub fn registrations(port: Arc<PlanningPort>) -> Result<Vec<ToolRegistration>, PlanningError> {
    let executor: Arc<dyn ToolExecutor> = Arc::new(PlanningExecutor { port });
    let plan = common::registration(
        "update_plan",
        include_str!("assets/schemas/UpdatePlan.json"),
        include_str!("assets/descriptions/UpdatePlan.md"),
        ToolApprovalPolicy::Never,
        "plan",
        Arc::clone(&executor),
    )
    .map_err(|()| PlanningError)?;
    let todos = common::registration(
        "write_todos",
        include_str!("assets/schemas/TodoWrite.json"),
        include_str!("assets/descriptions/TodoWrite.md"),
        ToolApprovalPolicy::Never,
        "plan",
        executor,
    )
    .map_err(|()| PlanningError)?;
    Ok(vec![plan, todos])
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[tokio::test]
async fn registered_names() {
    tests::registered_names_case().await;
}
