//! Bounded approval and ask-user producer bridge.

use crate::{
    builtin::common,
    pipeline::{
        ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
    },
    registry::ToolRegistration,
};
use lotta_runtime::ports::{ToolApprovalPolicy, ToolOutcome, ToolOutcomeMessage};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Maximum queued interaction requests.
pub const INTERACTION_CHANNEL_ITEMS_MAX: usize = 32;
/// Maximum pending correlations.
pub const INTERACTION_PENDING_ITEMS_MAX: usize = 128;
/// Maximum questions in one ask-user request.
pub const INTERACTION_QUESTIONS_ITEMS_MAX: usize = 4;
/// Minimum options per question.
pub const INTERACTION_OPTIONS_ITEMS_MIN: usize = 2;
/// Maximum options per question.
pub const INTERACTION_OPTIONS_ITEMS_MAX: usize = 4;
/// Maximum text bytes per question field or answer.
pub const INTERACTION_TEXT_BYTES_MAX: usize = 16 * 1024;
/// Maximum wait in milliseconds.
pub const INTERACTION_WAIT_MS_MAX: u64 = 86_400_000;

/// Correlation identifier allocated by one conversation bridge.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CorrelationId(String);

/// One approval producer payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ApprovalRequest {
    /// Correlation identifier.
    pub correlation_id: CorrelationId,
    /// Tool call identifier.
    pub tool_call_id: String,
    /// Tool name.
    pub tool_name: String,
    /// Validated input.
    pub input: Value,
}

/// One ask-user option.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionOption {
    /// Option label.
    pub label: String,
    /// Option description.
    pub description: String,
}

/// One exact ask-user question.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Question {
    /// Full question.
    pub question: String,
    /// Short header.
    pub header: String,
    /// Two to four options.
    pub options: Vec<QuestionOption>,
    /// Whether multiple options may be selected.
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

/// One ask-user producer payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AskUserRequest {
    /// Correlation identifier.
    pub correlation_id: CorrelationId,
    /// Exact validated questions.
    pub questions: Vec<Question>,
}

/// Typed producer request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionRequest {
    /// Approval request.
    Approval(ApprovalRequest),
    /// Ask-user request.
    AskUser(AskUserRequest),
}

/// Typed interaction response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractionResponse {
    /// Approval resolution.
    Approval(ApprovalDecision),
    /// Ask-user answer map.
    Answers(Map<String, Value>),
}

/// Approval decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalDecision {
    /// Allow execution.
    Allow,
    /// Deny execution.
    Deny,
}

/// Fixed bridge errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionError {
    /// Invalid input or response kind.
    Invalid,
    /// Caller cancelled.
    Cancelled,
    /// Deadline elapsed.
    Timeout,
    /// Request channel or response side closed.
    Closed,
    /// Pending bound reached.
    Limit,
    /// Correlation unknown or already resolved.
    Unknown,
}

#[derive(Clone, Copy)]
enum PendingKind {
    Approval,
    AskUser,
}

struct Pending {
    kind: PendingKind,
    sender: oneshot::Sender<InteractionResponse>,
}

/// Explicit bounded per-conversation interaction port.
pub struct InteractionPort {
    sender: mpsc::Sender<InteractionRequest>,
    pending: Mutex<BTreeMap<CorrelationId, Pending>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl InteractionPort {
    /// Creates one port and its bounded consumer receiver.
    #[must_use]
    pub fn new() -> (Arc<Self>, mpsc::Receiver<InteractionRequest>) {
        let (sender, receiver) = mpsc::channel(INTERACTION_CHANNEL_ITEMS_MAX);
        let port = Arc::new(Self {
            sender,
            pending: Mutex::new(BTreeMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
        });
        (port, receiver)
    }

    async fn request(
        &self,
        kind: PendingKind,
        build: impl FnOnce(CorrelationId) -> InteractionRequest,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Result<InteractionResponse, InteractionError> {
        let id = self.allocate_id();
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= INTERACTION_PENDING_ITEMS_MAX {
                return Err(InteractionError::Limit);
            }
            pending.insert(id.clone(), Pending { kind, sender });
        }
        if self.sender.send(build(id.clone())).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(InteractionError::Closed);
        }
        let result = tokio::select! {
            response = receiver => response.map_err(|_| InteractionError::Closed),
            () = cancellation.cancelled() => Err(InteractionError::Cancelled),
            () = tokio::time::sleep(timeout) => Err(InteractionError::Timeout),
        };
        self.pending.lock().await.remove(&id);
        result
    }

    /// Emits an approval request and awaits exactly one typed resolution.
    ///
    /// # Errors
    /// Returns fixed validation, cancellation, timeout, closure, limit, or response errors.
    pub async fn request_approval(
        &self,
        tool_call_id: String,
        tool_name: String,
        input: Value,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Result<ApprovalDecision, InteractionError> {
        valid_interaction_text(&tool_call_id)?;
        valid_interaction_text(&tool_name)?;
        let response = self
            .request(
                PendingKind::Approval,
                |correlation_id| {
                    InteractionRequest::Approval(ApprovalRequest {
                        correlation_id,
                        tool_call_id,
                        tool_name,
                        input,
                    })
                },
                cancellation,
                timeout.min(Duration::from_millis(INTERACTION_WAIT_MS_MAX)),
            )
            .await?;
        match response {
            InteractionResponse::Approval(decision) => Ok(decision),
            InteractionResponse::Answers(_) => Err(InteractionError::Invalid),
        }
    }

    /// Resolves one pending request exactly once; late or duplicate responses are harmless.
    ///
    /// # Errors
    /// Returns [`InteractionError::Unknown`] for late, duplicate, or closed responses.
    pub async fn respond(
        &self,
        id: &CorrelationId,
        response: InteractionResponse,
    ) -> Result<(), InteractionError> {
        let mut pending = self.pending.lock().await;
        let kind = pending
            .get(id)
            .map(|entry| entry.kind)
            .ok_or(InteractionError::Unknown)?;
        if !response_matches_request(kind, &response) {
            return Err(InteractionError::Invalid);
        }
        let pending = pending.remove(id).ok_or(InteractionError::Unknown)?;
        pending
            .sender
            .send(response)
            .map_err(|_| InteractionError::Unknown)
    }

    fn allocate_id(&self) -> CorrelationId {
        use std::sync::atomic::Ordering;
        let value = self.next_id.fetch_add(1, Ordering::Relaxed);
        CorrelationId(format!("interaction_{}_{value}", std::process::id()))
    }
}

struct AskExecutor {
    port: Arc<InteractionPort>,
}

impl ToolExecutor for AskExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let port = Arc::clone(&self.port);
        let value = request.input.as_value().clone();
        Box::pin(async move {
            let questions = parse_questions(&value)?;
            let timeout = request
                .deadline
                .get()
                .min(Duration::from_millis(INTERACTION_WAIT_MS_MAX));
            let response = port
                .request(
                    PendingKind::AskUser,
                    |correlation_id| {
                        InteractionRequest::AskUser(AskUserRequest {
                            correlation_id,
                            questions: questions.clone(),
                        })
                    },
                    request.cancellation,
                    timeout,
                )
                .await;
            map_ask_response(&questions, response)
        })
    }
}

fn parse_questions(value: &Value) -> Result<Vec<Question>, ExecutorError> {
    let questions = value.get("questions").cloned().ok_or(ExecutorError)?;
    let questions: Vec<Question> = serde_json::from_value(questions).map_err(|_| ExecutorError)?;
    if questions.is_empty() || questions.len() > INTERACTION_QUESTIONS_ITEMS_MAX {
        return Err(ExecutorError);
    }
    for question in &questions {
        valid_text(&question.question)?;
        valid_text(&question.header)?;
        if !(INTERACTION_OPTIONS_ITEMS_MIN..=INTERACTION_OPTIONS_ITEMS_MAX)
            .contains(&question.options.len())
        {
            return Err(ExecutorError);
        }
        for option in &question.options {
            valid_text(&option.label)?;
            valid_text(&option.description)?;
        }
    }
    Ok(questions)
}

fn map_ask_response(
    questions: &[Question],
    response: Result<InteractionResponse, InteractionError>,
) -> Result<RawToolOutcome, ExecutorError> {
    match response {
        Ok(InteractionResponse::Answers(answers)) => {
            validate_answers(questions, &answers)?;
            let mut parts = Vec::new();
            for question in questions {
                let answer = answers
                    .get(&question.question)
                    .and_then(Value::as_str)
                    .ok_or(ExecutorError)?;
                parts.push(format!("\"{}\"=\"{}\"", question.question, answer));
            }
            let message = format!(
                "User has answered your questions: {}. You can now continue with the user's answers in mind.",
                parts.join(", ")
            );
            serde_json::to_string(&serde_json::json!({"message": message}))
                .map(RawToolOutcome::Success)
                .map_err(|_| ExecutorError)
        }
        Ok(_) => Err(ExecutorError),
        Err(error) => map_error(error),
    }
}

fn validate_answers(
    questions: &[Question],
    answers: &Map<String, Value>,
) -> Result<(), ExecutorError> {
    if answers.len() != questions.len() {
        return Err(ExecutorError);
    }
    for (key, value) in answers {
        if !questions.iter().any(|question| question.question == *key) {
            return Err(ExecutorError);
        }
        let answer = value.as_str().ok_or(ExecutorError)?;
        valid_text(answer)?;
    }
    Ok(())
}

fn response_matches_request(kind: PendingKind, response: &InteractionResponse) -> bool {
    match (kind, response) {
        (
            PendingKind::Approval,
            InteractionResponse::Approval(ApprovalDecision::Allow | ApprovalDecision::Deny),
        ) => true,
        (PendingKind::AskUser, InteractionResponse::Answers(answers)) => {
            answers.len() <= INTERACTION_QUESTIONS_ITEMS_MAX
        }
        _ => false,
    }
}

fn map_error(error: InteractionError) -> Result<RawToolOutcome, ExecutorError> {
    let message = match error {
        InteractionError::Cancelled => "Interaction cancelled.",
        InteractionError::Timeout => "Interaction timed out.",
        InteractionError::Closed => "Interaction channel closed.",
        InteractionError::Limit => "Interaction pending limit reached.",
        _ => "Interaction response was invalid.",
    };
    let message = ToolOutcomeMessage::new(message.to_owned()).map_err(|_| ExecutorError)?;
    let outcome = match error {
        InteractionError::Cancelled => ToolOutcome::Interrupted { message },
        InteractionError::Timeout => ToolOutcome::Timeout { message },
        _ => ToolOutcome::ToolDefinedError {
            code: lotta_runtime::ports::ToolOutcomeCode::new("interaction_error".to_owned())
                .map_err(|_| ExecutorError)?,
            message,
        },
    };
    Ok(RawToolOutcome::Failure(outcome))
}

fn valid_interaction_text(value: &str) -> Result<(), InteractionError> {
    if value.is_empty() || value.len() > INTERACTION_TEXT_BYTES_MAX || value.contains('\0') {
        Err(InteractionError::Invalid)
    } else {
        Ok(())
    }
}

fn valid_text(value: &str) -> Result<(), ExecutorError> {
    if value.is_empty() {
        return Err(ExecutorError);
    }
    valid_text_or_empty(value)
}
fn valid_text_or_empty(value: &str) -> Result<(), ExecutorError> {
    if value.len() > INTERACTION_TEXT_BYTES_MAX || value.contains('\0') {
        Err(ExecutorError)
    } else {
        Ok(())
    }
}

/// Builds the pinned `AskUserQuestion` registration.
///
/// Approval requests use [`InteractionPort::request_approval`] from the permission bridge and are
/// intentionally not an invented model tool.
///
/// # Errors
/// Rejects malformed pinned assets.
pub fn registration(port: Arc<InteractionPort>) -> Result<ToolRegistration, InteractionError> {
    let executor: Arc<dyn ToolExecutor> = Arc::new(AskExecutor { port });
    common::registration(
        "AskUserQuestion",
        include_str!("assets/schemas/AskUserQuestion.json"),
        include_str!("assets/descriptions/AskUserQuestion.md"),
        ToolApprovalPolicy::Never,
        "interact",
        executor,
    )
    .map_err(|()| InteractionError::Invalid)
}

#[cfg(test)]
mod tests;
