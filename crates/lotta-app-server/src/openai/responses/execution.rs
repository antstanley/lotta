//! Responses conversation selection and supervised runtime ownership.

use std::{collections::HashMap, panic::AssertUnwindSafe, sync::Arc};

use futures_util::FutureExt as _;
use lotta_domain::{Agent, ConversationId, RuntimeScope};
use tokio::task::JoinSet;

use crate::ws::{DeferredInput, InputAdmissionWork};

use super::super::{chat, cursor};
use super::{
    input::PreparedRequest,
    output::ResponseOutcome,
    state::{ResponseCell, ResponseTurnSink},
};

/// Listener-owned Responses execution state sharing Task 75 runtime/repository adapters.
pub struct ResponsesState {
    /// Shared canonical Chat infrastructure and persistent key cache.
    pub chat: Arc<chat::ChatState>,
    owners: tokio::sync::Mutex<JoinSet<()>>,
    agent_owners: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    fork_supported: bool,
}

impl ResponsesState {
    /// Composes production Responses state over the canonical Chat adapters.
    #[must_use]
    pub fn new(chat: Arc<chat::ChatState>) -> Self {
        Self::with_fork_support(chat, true)
    }

    /// Composes state with an explicit canonical fork capability flag.
    #[must_use]
    pub fn with_fork_support(chat: Arc<chat::ChatState>, fork_supported: bool) -> Self {
        Self {
            chat,
            owners: tokio::sync::Mutex::new(JoinSet::new()),
            agent_owners: tokio::sync::Mutex::new(HashMap::new()),
            fork_supported,
        }
    }

    async fn agent_owner(&self, agent_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let owner = {
            let mut owners = self.agent_owners.lock().await;
            Arc::clone(
                owners
                    .entry(agent_id.to_owned())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        owner.lock_owned().await
    }

    /// Cancels through the shared listener token and joins every Responses owner.
    pub async fn shutdown(&self) {
        let mut owners = self.owners.lock().await;
        while owners.join_next().await.is_some() {}
    }
}

/// Validated prior-response source.
#[derive(Clone)]
pub struct PreviousState {
    /// Source conversation.
    pub conversation_id: ConversationId,
}

/// Prior-response resolution failure.
#[derive(Clone, Copy)]
pub enum PreviousError {
    /// Cursor or durable source is absent for this model.
    NotFound,
    /// Cursor belongs to a different model.
    WrongModel,
    /// Canonical fork support is unavailable.
    Unsupported,
    /// Repository lookup failed.
    Failed,
}

/// Strictly resolves one optional previous-response cursor before allocation.
///
/// # Errors
/// Distinguishes pinned not-found, unsupported-backend, and scrubbed server failures.
pub async fn resolve_previous(
    state: &ResponsesState,
    agent: &Agent,
    request: &PreparedRequest,
) -> Result<Option<PreviousState>, PreviousError> {
    let Some(response_id) = request.previous_response_id.as_deref() else {
        return Ok(None);
    };
    let parsed = cursor::parse(response_id).map_err(|_| PreviousError::NotFound)?;
    if parsed.agent_id != agent.id {
        return Err(PreviousError::WrongModel);
    }
    if !state.fork_supported {
        return Err(PreviousError::Unsupported);
    }
    let exists = state
        .chat
        .conversations
        .contains_for_openai(&agent.id, &parsed.conversation_id)
        .await
        .map_err(|_| PreviousError::Failed)?;
    if !exists {
        return Err(PreviousError::NotFound);
    }
    Ok(Some(PreviousState {
        conversation_id: parsed.conversation_id,
    }))
}

/// Spawns one independent supervised owner; no idempotency header is consulted.
pub async fn spawn_owner(
    state: Arc<ResponsesState>,
    agent: Agent,
    request: PreparedRequest,
    previous: Option<PreviousState>,
    cell: Arc<ResponseCell>,
) {
    let task_state = Arc::clone(&state);
    let mut owners = state.owners.lock().await;
    while owners.try_join_next().is_some() {}
    if state.chat.shutdown.is_cancelled() {
        drop(owners);
        cell.settle(ResponseOutcome::failed()).await;
        return;
    }
    owners.spawn(async move {
        let operation = AssertUnwindSafe(run_owner(&task_state, &agent, request, previous))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| ResponseOutcome::failed());
        cell.settle(operation).await;
    });
}

async fn run_owner(
    state: &ResponsesState,
    agent: &Agent,
    request: PreparedRequest,
    previous: Option<PreviousState>,
) -> ResponseOutcome {
    let _agent_owner = state.agent_owner(agent.id.as_str()).await;
    let allocated = allocate(state, agent, &request, previous.as_ref()).await;
    let Ok((conversation, owned, created_fork)) = allocated else {
        return ResponseOutcome::failed();
    };
    let mut outcome = tokio::select! {
        () = state.chat.shutdown.cancelled() => ResponseOutcome::failed(),
        outcome = execute_turn(state, agent, &conversation, &request) => outcome,
    };
    let success = outcome.error.is_none();
    let retained = success && (request.store || request.chat_key.is_some());
    if success
        && created_fork
        && let Some(chat_key) = request.chat_key.as_deref()
        && state
            .chat
            .remember_conversation(agent, chat_key, conversation.clone())
            .await
            .is_err()
    {
        outcome = ResponseOutcome::failed();
    }
    if !retained || outcome.error.is_some() {
        if owned && let Some(chat_key) = request.chat_key.as_deref() {
            state
                .chat
                .forget_conversation(agent, chat_key, &conversation)
                .await;
        }
        if cleanup(state, agent, &conversation, owned).await.is_err() {
            return ResponseOutcome::failed();
        }
    } else {
        outcome.conversation_id = Some(conversation);
    }
    outcome
}

async fn allocate(
    state: &ResponsesState,
    agent: &Agent,
    request: &PreparedRequest,
    previous: Option<&PreviousState>,
) -> Result<(ConversationId, bool, bool), ()> {
    if let Some(previous) = previous {
        return state
            .chat
            .conversations
            .fork_for_openai(&agent.id, &previous.conversation_id)
            .await
            .map(|conversation| (conversation, true, true))
            .map_err(|_| ());
    }
    chat::resolve_conversation_status(&state.chat, agent, request.chat_key.as_deref())
        .await
        .map(|resolved| (resolved.id, resolved.newly_created, false))
}

async fn execute_turn(
    state: &ResponsesState,
    agent: &Agent,
    conversation: &ConversationId,
    request: &PreparedRequest,
) -> ResponseOutcome {
    let scope = RuntimeScope::new(agent.id.clone(), conversation.clone(), None);
    if chat::start_runtime(&state.chat, &scope).await.is_err() {
        return ResponseOutcome::failed();
    }
    let Ok(command) = chat::input_command(scope.clone(), &request.messages) else {
        return ResponseOutcome::failed();
    };
    let Ok(admission) = state
        .chat
        .runtime_service
        .admit_input(command.clone())
        .await
    else {
        return ResponseOutcome::failed();
    };
    let InputAdmissionWork::NewStarted(continuation) = admission.work else {
        return ResponseOutcome::failed();
    };
    submit_turn(state, command, scope, continuation).await
}

async fn submit_turn(
    state: &ResponsesState,
    command: crate::ws::command::InputCommand,
    scope: RuntimeScope,
    continuation: lotta_domain::BoundedJsonValue,
) -> ResponseOutcome {
    let sink = Arc::new(ResponseTurnSink::new());
    let deferred = DeferredInput {
        scope,
        disposition: lotta_domain::InputDisposition::Started,
        continuation: Some(continuation),
    };
    let result = state
        .chat
        .turn_controller
        .submit_turn(
            command,
            deferred,
            state.chat.shutdown.child_token(),
            sink.clone(),
        )
        .await;
    sink.outcome(result.is_err())
}

async fn cleanup(
    state: &ResponsesState,
    agent: &Agent,
    conversation: &ConversationId,
    owned: bool,
) -> Result<(), ()> {
    if !owned {
        return Ok(());
    }
    let scope = RuntimeScope::new(agent.id.clone(), conversation.clone(), None);
    state
        .chat
        .runtime_service
        .teardown_ephemeral_runtime(scope)
        .await
        .map_err(|_| ())?;
    state
        .chat
        .conversations
        .delete_for_openai(&agent.id, conversation)
        .await
        .map_err(|_| ())
}
