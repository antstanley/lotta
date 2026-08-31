use std::{
    collections::HashMap,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use futures_util::FutureExt as _;
use lotta_domain::{Agent, ConversationId, RuntimeScope};
use tokio::{
    sync::{Mutex, OwnedMutexGuard, Semaphore},
    task::JoinSet,
};

use crate::ws::{DeferredInput, InputAdmissionWork};

use super::super::{chat, cursor};
use super::{
    input::PreparedRequest,
    output::ResponseOutcome,
    state::{ResponseCell, ResponseTurnSink},
};

/// Named maximum number of listener-owned Responses operations.
pub const OPENAI_RESPONSES_OWNERS_MAX: usize = 128;

struct SetupLock {
    gate: Arc<Mutex<()>>,
    refs: AtomicUsize,
}

/// Synchronous ownership of one setup-lock reference. The reference exists before
/// waiting on the async gate, so cancellation and task abort release it immediately.
/// Unwinding allocation drops the gate and prunes the exact map entry without
/// depending on an async destructor or another runtime poll.
struct SetupLease<'a> {
    locks: &'a StdMutex<HashMap<String, Arc<SetupLock>>>,
    key: String,
    entry: Arc<SetupLock>,
    gate: Option<OwnedMutexGuard<()>>,
}

impl Drop for SetupLease<'_> {
    fn drop(&mut self) {
        self.gate.take();
        if self.entry.refs.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if locks
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.entry))
            && self.entry.refs.load(Ordering::Acquire) == 0
        {
            locks.remove(&self.key);
        }
    }
}

/// Listener-owned Responses execution state sharing Task 75 runtime/repository adapters.
pub struct ResponsesState {
    /// Shared canonical Chat infrastructure and persistent key cache.
    pub chat: Arc<chat::ChatState>,
    owners: Mutex<JoinSet<()>>,
    owner_capacity: Arc<Semaphore>,
    setup_locks: StdMutex<HashMap<String, Arc<SetupLock>>>,
    conversations: Arc<dyn crate::ws::conversations::ConversationCommandRepository>,
    fork_supported: bool,
}

impl ResponsesState {
    /// Composes production Responses state over the canonical Chat adapters and backend capability.
    #[must_use]
    pub fn new(chat: Arc<chat::ChatState>) -> Self {
        let repository = chat.conversations.clone();
        Self::with_repository(chat, repository)
    }

    /// Composes state from an explicit conversation command repository.
    #[must_use]
    pub fn with_repository(
        chat: Arc<chat::ChatState>,
        repository: Arc<dyn crate::ws::conversations::ConversationCommandRepository>,
    ) -> Self {
        Self::with_repository_and_bound(chat, repository, OPENAI_RESPONSES_OWNERS_MAX)
    }

    fn with_repository_and_bound(
        chat: Arc<chat::ChatState>,
        repository: Arc<dyn crate::ws::conversations::ConversationCommandRepository>,
        owner_capacity: usize,
    ) -> Self {
        let fork_supported = repository.supports_hidden_fork();
        Self {
            chat,
            owners: Mutex::new(JoinSet::new()),
            owner_capacity: Arc::new(Semaphore::new(owner_capacity)),
            setup_locks: StdMutex::new(HashMap::new()),
            conversations: repository,
            fork_supported,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_repository_and_capacity(
        chat: Arc<chat::ChatState>,
        repository: Arc<dyn crate::ws::conversations::ConversationCommandRepository>,
        owner_capacity: usize,
    ) -> Self {
        Self::with_repository_and_bound(chat, repository, owner_capacity)
    }

    async fn acquire_setup(&self, key: &str) -> SetupLease<'_> {
        let key = key.to_owned();
        let entry = {
            let mut locks = self
                .setup_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = Arc::clone(locks.entry(key.clone()).or_insert_with(|| {
                Arc::new(SetupLock {
                    gate: Arc::new(Mutex::new(())),
                    refs: AtomicUsize::new(0),
                })
            }));
            entry.refs.fetch_add(1, Ordering::AcqRel);
            entry
        };
        let mut lease = SetupLease {
            locks: &self.setup_locks,
            key,
            entry,
            gate: None,
        };
        lease.gate = Some(Arc::clone(&lease.entry.gate).lock_owned().await);
        lease
    }

    /// Cancels through the shared listener token and joins every Responses owner.
    pub async fn shutdown(&self) {
        let mut owners = self.owners.lock().await;
        while owners.join_next().await.is_some() {}
    }

    #[cfg(test)]
    pub(crate) async fn owner_count(&self) -> usize {
        let mut owners = self.owners.lock().await;
        while owners.try_join_next().is_some() {}
        owners.len()
    }

    #[cfg(test)]
    pub(crate) fn setup_lock_count(&self) -> usize {
        self.setup_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
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
    NotFound,
    WrongModel,
    Unsupported,
    Failed,
}

/// Strictly resolves one optional previous-response cursor before allocation.
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
    // Capability is checked before existence lookup, allocation, or fork work.
    if !state.fork_supported {
        return Err(PreviousError::Unsupported);
    }
    let exists = state
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

/// Owner registration failure returned before a response is exposed.
pub enum SpawnError {
    Backpressure,
    Shutdown,
}

/// Registers one independent supervised owner within the named bound.
pub async fn spawn_owner(
    state: Arc<ResponsesState>,
    agent: Agent,
    request: PreparedRequest,
    previous: Option<PreviousState>,
    cell: Arc<ResponseCell>,
) -> Result<(), SpawnError> {
    if state.chat.shutdown.is_cancelled() {
        return Err(SpawnError::Shutdown);
    }
    let permit = Arc::clone(&state.owner_capacity)
        .try_acquire_owned()
        .map_err(|_| SpawnError::Backpressure)?;
    let task_state = Arc::clone(&state);
    let task_cell = Arc::clone(&cell);
    let mut owners = state.owners.lock().await;
    while owners.try_join_next().is_some() {}
    if state.chat.shutdown.is_cancelled() {
        return Err(SpawnError::Shutdown);
    }
    owners.spawn(async move {
        let _permit = permit;
        let operation = AssertUnwindSafe(run_owner(
            &task_state,
            &agent,
            request,
            previous,
            Arc::clone(&task_cell),
        ))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| ResponseOutcome::failed());
        task_cell.settle(operation);
    });
    Ok(())
}

async fn run_owner(
    state: &ResponsesState,
    agent: &Agent,
    request: PreparedRequest,
    previous: Option<PreviousState>,
    cell: Arc<ResponseCell>,
) -> ResponseOutcome {
    let allocated = tokio::select! {
        () = state.chat.shutdown.cancelled() => Err(()),
        allocated = allocate_serialized(state, agent, &request, previous.as_ref()) => allocated,
    };
    let Ok((conversation, owned, created_fork)) = allocated else {
        cell.start(None);
        return ResponseOutcome::failed();
    };
    cell.start(Some(conversation.clone()));
    let mut outcome = tokio::select! {
        () = state.chat.shutdown.cancelled() => ResponseOutcome::failed(),
        outcome = execute_turn(state, agent, &conversation, &request, Arc::clone(&cell)) => outcome,
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

fn setup_key(agent: &Agent) -> String {
    // The local conversation repository's create/fork + prompt compilation setup is
    // agent-scoped. Only that short setup is serialized; admission and the provider turn are not.
    agent.id.as_str().to_owned()
}

async fn allocate_serialized(
    state: &ResponsesState,
    agent: &Agent,
    request: &PreparedRequest,
    previous: Option<&PreviousState>,
) -> Result<(ConversationId, bool, bool), ()> {
    let key = setup_key(agent);
    let _lease = state.acquire_setup(&key).await;
    allocate(state, agent, request, previous).await
}

async fn allocate(
    state: &ResponsesState,
    agent: &Agent,
    request: &PreparedRequest,
    previous: Option<&PreviousState>,
) -> Result<(ConversationId, bool, bool), ()> {
    if let Some(previous) = previous {
        return state
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
    cell: Arc<ResponseCell>,
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
    submit_turn(state, command, scope, continuation, cell).await
}

async fn submit_turn(
    state: &ResponsesState,
    command: crate::ws::command::InputCommand,
    scope: RuntimeScope,
    continuation: lotta_domain::BoundedJsonValue,
    cell: Arc<ResponseCell>,
) -> ResponseOutcome {
    let sink = Arc::new(ResponseTurnSink::new(cell));
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
    // Local repository artifact deletion shares only the short setup critical section,
    // preventing it from racing prompt compilation for another conversation.
    let key = setup_key(agent);
    let _lease = state.acquire_setup(&key).await;
    state
        .conversations
        .delete_for_openai(&agent.id, conversation)
        .await
        .map_err(|_| ())
}
