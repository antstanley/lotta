//! Authenticated OpenAI Chat Completions transport adapter.

use std::{
    collections::VecDeque,
    convert::Infallible,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{
    body::{Body, Bytes, to_bytes},
    extract::FromRequest,
    http::{HeaderMap, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::{FutureExt as _, stream};
use lotta_domain::{Agent, BoundedJsonValue, Clock, NonEmptyString, RuntimeScope};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    bounds::HTTP_BODY_BYTES_MAX,
    ws::{
        DeferredInput, RuntimeCommandService, RuntimeEvent, RuntimeEventSink, TurnController,
        command::{InputCommand, RuntimeMode, RuntimeStartCommand},
        event::StreamDelta,
        service::InputAdmissionWork,
    },
};

use super::{
    chat_keys::{ChatKeyCache, ChatKeyClaim, ChatScopeKey, OPENAI_CHAT_KEY_BYTES_MAX},
    errors,
    idempotency::{
        ChatIdentity, IDEMPOTENCY_KEY_BYTES_MAX, OutcomeCache, OutcomeCell, OutcomeClaim,
        OutcomeKey, TurnOutcome, Usage,
    },
    resolve,
};

/// Maximum decoded JSON nesting accepted by this route.
pub const OPENAI_CHAT_JSON_DEPTH_MAX: usize = 64;
/// Maximum model identifier length.
pub const OPENAI_CHAT_MODEL_BYTES_MAX: usize = 1_024;
/// Maximum messages accepted in one replay transcript.
pub const OPENAI_CHAT_MESSAGES_MAX: usize = 4_096;
/// Maximum content parts accepted in one message.
pub const OPENAI_CHAT_CONTENT_PARTS_MAX: usize = 4_096;
/// Maximum aggregate decoded content bytes.
pub const OPENAI_CHAT_CONTENT_BYTES_MAX: usize = HTTP_BODY_BYTES_MAX;
/// Bounded per-response SSE queue.
pub const OPENAI_CHAT_SSE_EVENTS_MAX: usize = 64;

const CHAT_KEY_HEADER: &str = "x-letta-chat-key";
const OPENWEBUI_HEADER: &str = "x-openwebui-chat-id";
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const X_IDEMPOTENCY_HEADER: &str = "x-idempotency-key";

/// Shared listener-local state for Chat Completions.
pub struct ChatState {
    /// Canonical agent repository bridge.
    pub agents: Arc<crate::ws::agents::AgentsBridge>,
    /// Canonical conversation repository bridge.
    pub conversations: Arc<crate::ws::conversations::ConversationsBridge>,
    /// Canonical admission service.
    pub runtime_service: Arc<dyn RuntimeCommandService>,
    /// Canonical production turn controller.
    pub turn_controller: Arc<dyn TurnController>,
    /// Listener clock.
    pub clock: Arc<dyn Clock + Send + Sync>,
    /// Listener lifetime cancellation.
    pub shutdown: CancellationToken,
    chat_keys: ChatKeyCache,
    outcomes: Arc<OutcomeCache>,
    owners: tokio::sync::Mutex<JoinSet<()>>,
}

impl ChatState {
    /// Composes state over the listener's existing repositories and runtime ports.
    #[must_use]
    pub fn new(
        agents: Arc<crate::ws::agents::AgentsBridge>,
        conversations: Arc<crate::ws::conversations::ConversationsBridge>,
        runtime_service: Arc<dyn RuntimeCommandService>,
        turn_controller: Arc<dyn TurnController>,
        clock: Arc<dyn Clock + Send + Sync>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            agents,
            conversations,
            runtime_service,
            turn_controller,
            clock,
            shutdown,
            chat_keys: ChatKeyCache::new(),
            outcomes: Arc::new(OutcomeCache::new()),
            owners: tokio::sync::Mutex::new(JoinSet::new()),
        }
    }

    /// Cancels and joins every listener-owned Chat execution.
    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        let mut owners = self.owners.lock().await;
        while owners.join_next().await.is_some() {}
    }
}

#[derive(Deserialize)]
struct ChatRequest {
    model: Option<String>,
    messages: Option<Vec<ChatMessage>>,
    #[serde(default)]
    stream: Value,
}

#[derive(Deserialize)]
struct ChatMessage {
    role: Option<String>,
    content: Option<Value>,
}

#[derive(Clone)]
struct TurnMessage {
    role: &'static str,
    content: Vec<Value>,
    client_message_id: String,
}

struct ChatInputError(String);

impl ChatInputError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn response(self) -> Response {
        invalid(self.0)
    }
}

struct PreparedRequest {
    model: String,
    messages: Vec<TurnMessage>,
    streaming: bool,
    chat_key: Option<String>,
    idempotency_key: Option<String>,
    fingerprint: [u8; 32],
}

/// Route-local bounded JSON extractor with OpenAI-compatible rejections.
pub struct ChatJson(Value);

impl<S> FromRequest<S> for ChatJson
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(request: Request<Body>, _: &S) -> Result<Self, Self::Rejection> {
        let bytes = to_bytes(request.into_body(), HTTP_BODY_BYTES_MAX)
            .await
            .map_err(|_| {
                errors::response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    errors::invalid_request("request body too large"),
                )
            })?;
        let value = if bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&bytes).map_err(|_| invalid("invalid JSON body"))?
        };
        validate_json_shape(&value).map_err(ChatInputError::response)?;
        Ok(Self(value))
    }
}

/// Handles one authenticated Chat Completions request.
pub async fn complete(
    state: Arc<ChatState>,
    headers: HeaderMap,
    ChatJson(value): ChatJson,
) -> Response {
    let prepared = match prepare(value, &headers) {
        Ok(prepared) => prepared,
        Err(error) => return error.response(),
    };
    let Ok(agents) = resolve::visible_agents(&state.agents).await else {
        return server_failure("failed to list available agents");
    };
    let agent = match resolve::resolve(&agents, &prepared.model) {
        Ok(agent) => agent.clone(),
        Err(error) => return errors::response(StatusCode::NOT_FOUND, error),
    };
    dispatch(state, agent, prepared).await
}

fn prepare(value: Value, headers: &HeaderMap) -> Result<PreparedRequest, ChatInputError> {
    let request: ChatRequest =
        serde_json::from_value(value).map_err(|error| ChatInputError::new(error.to_string()))?;
    let model = request
        .model
        .filter(|model| !model.is_empty())
        .ok_or_else(|| ChatInputError::new("you must provide a model parameter"))?;
    if model.len() > OPENAI_CHAT_MODEL_BYTES_MAX {
        return Err(ChatInputError::new("model exceeds maximum length"));
    }
    let messages = request
        .messages
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| ChatInputError::new("you must provide a non-empty messages array"))?;
    if messages.len() > OPENAI_CHAT_MESSAGES_MAX {
        return Err(ChatInputError::new("messages exceeds maximum length"));
    }
    validate_all_content_arrays(&messages)?;
    let newest = newest_user(&messages).ok_or_else(|| {
        ChatInputError::new(
            "the messages array must include a user message with text or image content",
        )
    })?;
    let streaming = request.stream == Value::Bool(true);
    let chat_key = chat_key(headers, streaming)?;
    let selected = select_messages(&messages, newest, chat_key.is_some())?;
    let idempotency_key =
        normalized_header(headers, IDEMPOTENCY_HEADER, IDEMPOTENCY_KEY_BYTES_MAX)?.or(
            normalized_header(headers, X_IDEMPOTENCY_HEADER, IDEMPOTENCY_KEY_BYTES_MAX)?,
        );
    let fingerprint = request_fingerprint(&model, &selected)?;
    Ok(PreparedRequest {
        model,
        messages: selected,
        streaming,
        chat_key,
        idempotency_key,
        fingerprint,
    })
}

fn newest_user(messages: &[ChatMessage]) -> Option<usize> {
    messages.iter().rposition(|message| {
        message.role.as_deref() == Some("user") && !user_parts(message.content.as_ref()).is_empty()
    })
}

fn validate_all_content_arrays(messages: &[ChatMessage]) -> Result<(), ChatInputError> {
    for message in messages {
        if let Some(Value::Array(parts)) = message.content.as_ref()
            && parts.len() > OPENAI_CHAT_CONTENT_PARTS_MAX
        {
            return Err(ChatInputError::new(
                "message content parts exceeds maximum length",
            ));
        }
    }
    Ok(())
}

fn request_fingerprint(model: &str, messages: &[TurnMessage]) -> Result<[u8; 32], ChatInputError> {
    let canonical = messages
        .iter()
        .map(|message| json!({"role":message.role, "content":message.content}))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&json!({"model":model, "messages":canonical}))
        .map_err(|_| ChatInputError::new("failed to canonicalize request"))?;
    Ok(Sha256::digest(bytes).into())
}

fn select_messages(
    messages: &[ChatMessage],
    newest: usize,
    stateful: bool,
) -> Result<Vec<TurnMessage>, ChatInputError> {
    let selected = if stateful {
        vec![turn_message(
            "user",
            user_parts(messages[newest].content.as_ref()),
        )]
    } else {
        messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| {
                match message.role.as_deref() {
                    Some("user") => nonempty_turn("user", user_parts(message.content.as_ref())),
                    Some("assistant") => {
                        nonempty_turn("assistant", assistant_parts(message.content.as_ref()))
                    }
                    _ => None,
                }
                .map(|mut item| {
                    if index == newest {
                        item.content = user_parts(messages[newest].content.as_ref());
                    }
                    item
                })
            })
            .collect()
    };
    let content_bytes = selected
        .iter()
        .flat_map(|message| &message.content)
        .map(|part| part.to_string().len())
        .sum::<usize>();
    if content_bytes > OPENAI_CHAT_CONTENT_BYTES_MAX {
        return Err(ChatInputError::new(
            "message content exceeds maximum length",
        ));
    }
    if selected.is_empty() {
        return Err(ChatInputError::new(
            "the messages array must include usable content",
        ));
    }
    Ok(selected)
}

fn nonempty_turn(role: &'static str, parts: Vec<Value>) -> Option<TurnMessage> {
    (!parts.is_empty()).then(|| turn_message(role, parts))
}

fn turn_message(role: &'static str, content: Vec<Value>) -> TurnMessage {
    TurnMessage {
        role,
        content,
        client_message_id: fresh_uuid().to_string(),
    }
}

fn user_parts(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) if !text.is_empty() => vec![json!({"type":"text", "text":text})],
        Some(Value::Array(parts)) if parts.len() <= OPENAI_CHAT_CONTENT_PARTS_MAX => {
            parts.iter().filter_map(user_part).collect()
        }
        _ => Vec::new(),
    }
}

fn user_part(part: &Value) -> Option<Value> {
    let kind = part.get("type")?.as_str()?;
    if matches!(kind, "text" | "input_text") {
        let text = part.get("text")?.as_str()?;
        return (!text.is_empty()).then(|| json!({"type":"text", "text":text}));
    }
    if !matches!(kind, "image_url" | "input_image") {
        return None;
    }
    let image = part.get("image_url")?;
    let raw = image
        .as_str()
        .or_else(|| image.get("url").and_then(Value::as_str))?;
    image_part(raw)
}

fn image_part(raw: &str) -> Option<Value> {
    if let Some(rest) = raw.strip_prefix("data:") {
        let (media_type, data) = rest.split_once(";base64,")?;
        if media_type.is_empty() || data.is_empty() {
            return None;
        }
        return Some(json!({"type":"image", "source":{
            "type":"base64", "media_type":media_type, "data":data
        }}));
    }
    (raw.starts_with("http://") || raw.starts_with("https://"))
        .then(|| json!({"type":"image", "source":{"type":"url", "url":raw}}))
}

fn assistant_parts(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) if !text.is_empty() => vec![json!({"type":"text", "text":text})],
        Some(Value::Array(parts)) if parts.len() <= OPENAI_CHAT_CONTENT_PARTS_MAX => {
            let text = parts
                .iter()
                .filter_map(|part| {
                    matches!(
                        part.get("type").and_then(Value::as_str),
                        Some("text" | "input_text" | "output_text")
                    )
                    .then(|| part.get("text").and_then(Value::as_str))
                    .flatten()
                    .filter(|text| !text.is_empty())
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty())
                .then(|| json!({"type":"text", "text":text}))
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

fn chat_key(headers: &HeaderMap, streaming: bool) -> Result<Option<String>, ChatInputError> {
    if let Some(value) = normalized_header(headers, CHAT_KEY_HEADER, OPENAI_CHAT_KEY_BYTES_MAX)? {
        return Ok(Some(value));
    }
    if streaming {
        return normalized_header(headers, OPENWEBUI_HEADER, OPENAI_CHAT_KEY_BYTES_MAX);
    }
    Ok(None)
}

fn normalized_header(
    headers: &HeaderMap,
    name: &'static str,
    bytes_max: usize,
) -> Result<Option<String>, ChatInputError> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let text = value
        .to_str()
        .map_err(|_| ChatInputError::new("invalid request header"))?
        .trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.len() > bytes_max {
        return Err(ChatInputError::new("request header exceeds maximum length"));
    }
    Ok(Some(text.to_owned()))
}

async fn dispatch(state: Arc<ChatState>, agent: Agent, request: PreparedRequest) -> Response {
    let cache_key = request.idempotency_key.as_ref().map(|key| OutcomeKey {
        agent_id: agent.id.as_str().to_owned(),
        chat: request
            .chat_key
            .as_ref()
            .map_or(ChatIdentity::Ephemeral(request.fingerprint), |chat| {
                ChatIdentity::Persistent(chat.clone())
            }),
        idempotency_key: key.clone(),
    });
    let (cell, owner) = match &cache_key {
        Some(key) => match state.outcomes.claim(key.clone()).await {
            OutcomeClaim::Owner(cell) => (cell, true),
            OutcomeClaim::Existing(cell) => (cell, false),
            OutcomeClaim::Full => return capacity_failure("idempotency cache is at capacity"),
        },
        None => (Arc::new(OutcomeCell::new()), true),
    };
    if owner {
        spawn_owner(
            Arc::clone(&state),
            agent,
            request.messages.clone(),
            request.chat_key.clone(),
            cache_key,
            Arc::clone(&cell),
        )
        .await;
    }
    render(
        cell,
        owner,
        &request.model,
        request.streaming,
        state.clock.now().as_utc().timestamp(),
    )
    .await
}

async fn resolve_conversation(
    state: &ChatState,
    agent: &Agent,
    chat_key: Option<&str>,
) -> Result<lotta_domain::ConversationId, ()> {
    let Some(chat_key) = chat_key else {
        return state
            .conversations
            .create_for_openai(&agent.id)
            .await
            .map_err(|_| ());
    };
    let key = ChatScopeKey {
        agent_id: agent.id.as_str().to_owned(),
        chat_id: chat_key.to_owned(),
    };
    match state.chat_keys.claim(key.clone()).await {
        ChatKeyClaim::Existing(slot) => slot.wait().await,
        ChatKeyClaim::Full => Err(()),
        ChatKeyClaim::Owner(slot) => {
            // The slot owner contains every abnormal exit from the repository
            // await. Shutdown cancels the allocation future, panic is caught,
            // and the exact slot is removed before existing waiters are woken.
            let created = AssertUnwindSafe(async {
                tokio::select! {
                    () = state.shutdown.cancelled() => Err(()),
                    result = state.conversations.create_for_openai(&agent.id) => {
                        result.map_err(|_| ())
                    }
                }
            })
            .catch_unwind()
            .await
            .unwrap_or(Err(()));
            if let Ok(conversation) = created {
                slot.settle(Ok(conversation.clone())).await;
                Ok(conversation)
            } else {
                state.chat_keys.remove_failed_and_settle(&key, &slot).await;
                Err(())
            }
        }
    }
}

async fn spawn_owner(
    state: Arc<ChatState>,
    agent: Agent,
    messages: Vec<TurnMessage>,
    chat_key: Option<String>,
    cache_key: Option<OutcomeKey>,
    cell: Arc<OutcomeCell>,
) {
    let task_state = Arc::clone(&state);
    let mut owners = state.owners.lock().await;
    while owners.try_join_next().is_some() {}
    if state.shutdown.is_cancelled() {
        drop(owners);
        if let Some(key) = cache_key {
            state
                .outcomes
                .evict_failed_and_settle(&key, &cell, failed_outcome())
                .await;
        } else {
            cell.settle(failed_outcome()).await;
        }
        return;
    }
    owners.spawn(async move {
        let state = task_state;
        let resolved = AssertUnwindSafe(resolve_conversation(&state, &agent, chat_key.as_deref()))
            .catch_unwind()
            .await;
        let mut outcome = if let Ok(Ok(conversation)) = resolved {
            let scope = RuntimeScope::new(agent.id.clone(), conversation.clone(), None);
            let execution = AssertUnwindSafe(async {
                tokio::select! {
                    () = state.shutdown.cancelled() => failed_outcome(),
                    outcome = execute_turn(
                        &state,
                        &agent,
                        &conversation,
                        messages,
                        Arc::clone(&cell),
                    ) => outcome,
                }
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|_| failed_outcome());
            let cleanup = if chat_key.is_none() {
                AssertUnwindSafe(async {
                    // Durable artifacts are removed only after the canonical
                    // runtime owner confirms lifecycle/queue/approval/subscription
                    // quiescence and removes the registry entry.
                    state
                        .runtime_service
                        .teardown_ephemeral_runtime(scope)
                        .await?;
                    state
                        .conversations
                        .delete_for_openai(&agent.id, &conversation)
                        .await
                })
                .catch_unwind()
                .await
                .is_ok_and(|result| result.is_ok())
            } else {
                true
            };
            if cleanup { execution } else { failed_outcome() }
        } else {
            failed_outcome()
        };
        // No provider/runtime detail is retained in cache or sent on the wire.
        if outcome.error.is_some() {
            outcome = failed_outcome();
        }
        let failed = outcome.error.is_some();
        if failed && let Some(key) = cache_key {
            state
                .outcomes
                .evict_failed_and_settle(&key, &cell, outcome)
                .await;
        } else {
            cell.settle(outcome).await;
        }
    });
}

async fn execute_turn(
    state: &ChatState,
    agent: &Agent,
    conversation: &lotta_domain::ConversationId,
    messages: Vec<TurnMessage>,
    cell: Arc<OutcomeCell>,
) -> TurnOutcome {
    let scope = RuntimeScope::new(agent.id.clone(), conversation.clone(), None);
    if start_runtime(state, &scope).await.is_err() {
        return failed_outcome();
    }
    let Ok(command) = input_command(scope.clone(), &messages) else {
        return failed_outcome();
    };
    let Ok(admission) = state.runtime_service.admit_input(command.clone()).await else {
        return failed_outcome();
    };
    let InputAdmissionWork::NewStarted(continuation) = admission.work else {
        return failed_outcome();
    };
    let sink = Arc::new(ChatTurnSink::new(Arc::clone(&cell)));
    let deferred = DeferredInput {
        scope,
        disposition: lotta_domain::InputDisposition::Started,
        continuation: Some(continuation),
    };
    let result = state
        .turn_controller
        .submit_turn(
            command,
            deferred,
            state.shutdown.child_token(),
            sink.clone(),
        )
        .await;
    sink.outcome(result.is_err())
}

async fn start_runtime(state: &ChatState, scope: &RuntimeScope) -> Result<(), ()> {
    let request_id =
        NonEmptyString::new(format!("openai-start-{}", fresh_uuid())).map_err(|_| ())?;
    let command = RuntimeStartCommand {
        request_id,
        agent_id: NonEmptyString::new(scope.agent_id.as_str().to_owned()).ok(),
        create_agent: None,
        conversation_id: NonEmptyString::new(scope.conversation_id.as_str().to_owned()).ok(),
        create_conversation: None,
        cwd: None,
        mode: Some(RuntimeMode::Unrestricted),
        conversation_source_tags: None,
        workspace_sandbox: None,
        skill_sources: None,
        preserve_skill_sources: None,
        client_info: None,
        recover_approvals: false,
        force_device_status: None,
        wait_for_replay: None,
        external_tools: None,
    };
    state
        .runtime_service
        .runtime_start(0, command)
        .await
        .map(|_| ())
        .map_err(|_| ())
}

fn input_command(scope: RuntimeScope, messages: &[TurnMessage]) -> Result<InputCommand, ()> {
    let values = messages
        .iter()
        .map(|message| {
            json!({
                "client_message_id": message.client_message_id,
                "role": message.role,
                "content": message.content,
                "otid": message.client_message_id,
            })
        })
        .collect::<Vec<_>>();
    let request_id = messages
        .last()
        .map(|message| message.client_message_id.clone())
        .ok_or(())?;
    let payload = BoundedJsonValue::new(json!({"kind":"create_message", "messages":values}))
        .map_err(|_| ())?;
    Ok(InputCommand {
        request_id: NonEmptyString::new(request_id).ok(),
        runtime: scope,
        payload,
    })
}

struct ChatTurnSink {
    cell: Arc<OutcomeCell>,
    text: Mutex<String>,
    usage: Mutex<Usage>,
    error: Mutex<Option<String>>,
}

impl ChatTurnSink {
    fn new(cell: Arc<OutcomeCell>) -> Self {
        Self {
            cell,
            text: Mutex::new(String::new()),
            usage: Mutex::new(Usage::default()),
            error: Mutex::new(None),
        }
    }

    fn outcome(&self, controller_failed: bool) -> TurnOutcome {
        let text = self
            .text
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default();
        let usage = self
            .usage
            .lock()
            .map(|usage| usage.clone())
            .unwrap_or_default();
        let mut error = self.error.lock().ok().and_then(|error| error.clone());
        if controller_failed && error.is_none() {
            error = Some("failed to run agent turn".to_owned());
        }
        TurnOutcome { text, usage, error }
    }

    fn apply_other(&self, value: &Value) {
        let kind = value.get("kind").and_then(Value::as_str);
        let message_type = value.get("message_type").and_then(Value::as_str);
        if matches!(kind, Some("assistant" | "text")) || message_type == Some("assistant_message") {
            if let Some(piece) = delta_text(value) {
                if let Ok(mut text) = self.text.lock() {
                    text.push_str(&piece);
                }
                self.cell.publish(piece);
            }
        } else if message_type == Some("usage_statistics") {
            if let Ok(mut usage) = self.usage.lock() {
                usage.prompt_tokens = number(value, "prompt_tokens");
                usage.completion_tokens = number(value, "completion_tokens");
                usage.total_tokens = number(value, "total_tokens");
                usage.reasoning_tokens = value.get("reasoning_tokens").and_then(Value::as_u64);
            }
        } else if matches!(message_type, Some("loop_error" | "error_message"))
            && let Ok(mut error) = self.error.lock()
        {
            *error = Some("failed to run agent turn".to_owned());
        }
    }
}

impl RuntimeEventSink for ChatTurnSink {
    fn emit(
        &self,
        _: &RuntimeScope,
        event: RuntimeEvent,
    ) -> Result<(), crate::error::AppServerError> {
        match event {
            RuntimeEvent::StreamDelta {
                delta: StreamDelta::Other(value),
                subagent_id: None,
            } => {
                self.apply_other(value.as_value());
            }
            RuntimeEvent::TurnFinished {
                stop_reason, error, ..
            } => {
                let terminal = error.map_or_else(
                    || {
                        matches!(stop_reason.as_str(), "cancelled" | "user_cancellation")
                            .then(|| "failed to run agent turn".to_owned())
                    },
                    |_| Some("failed to run agent turn".to_owned()),
                );
                if terminal.is_some()
                    && let Ok(mut current) = self.error.lock()
                {
                    *current = terminal;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn delta_text(value: &Value) -> Option<String> {
    value
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            value
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn failed_outcome() -> TurnOutcome {
    TurnOutcome {
        text: String::new(),
        usage: Usage::default(),
        error: Some("failed to run agent turn".to_owned()),
    }
}

async fn render(
    cell: Arc<OutcomeCell>,
    owner: bool,
    model: &str,
    streaming: bool,
    created: i64,
) -> Response {
    let (completion_id, created) = cell
        .response_identity(|| (format!("chatcmpl-{}", fresh_uuid()), created))
        .await;
    if streaming {
        return sse_response(cell, owner, completion_id, created, model.to_owned());
    }
    let outcome = cell.wait().await;
    if let Some(error) = &outcome.error {
        return server_failure(error);
    }
    json_completion(&completion_id, created, model, &outcome)
}

fn json_completion(id: &str, created: i64, model: &str, outcome: &TurnOutcome) -> Response {
    let mut usage = json!({
        "prompt_tokens":outcome.usage.prompt_tokens,
        "completion_tokens":outcome.usage.completion_tokens,
        "total_tokens":outcome.usage.total_tokens,
    });
    if let Some(reasoning) = outcome.usage.reasoning_tokens {
        usage["completion_tokens_details"] = json!({"reasoning_tokens":reasoning});
    }
    axum::Json(json!({
        "id":id, "object":"chat.completion", "created":created, "model":model,
        "choices":[{"index":0, "message":{"role":"assistant", "content":outcome.text},
            "finish_reason":"stop"}],
        "usage":usage,
    }))
    .into_response()
}

fn sse_response(
    cell: Arc<OutcomeCell>,
    _owner: bool,
    id: String,
    created: i64,
    model: String,
) -> Response {
    // The body itself owns the outcome cursor. No detached writer exists: if
    // Hyper drops an unread body, the broadcast receiver and all polling stop
    // immediately without a producer blocked on a bounded response queue.
    let body_stream = stream::unfold(
        SseCursor::new(cell, id, created, model),
        |mut cursor| async move {
            cursor
                .next()
                .await
                .map(|bytes| (Ok::<Bytes, Infallible>(bytes), cursor))
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| server_failure("failed to create stream"))
}

struct SseCursor {
    cell: Arc<OutcomeCell>,
    deltas: tokio::sync::broadcast::Receiver<(u64, String)>,
    pending: VecDeque<Bytes>,
    id: String,
    created: i64,
    model: String,
    last_sequence: u64,
    sent: String,
    finished: bool,
}

impl SseCursor {
    fn new(cell: Arc<OutcomeCell>, id: String, created: i64, model: String) -> Self {
        // Subscribe before snapshotting replay so publication cannot fall between
        // those operations. Sequence filtering removes snapshot/live overlap.
        let deltas = cell.subscribe();
        let replay = cell.replay();
        let mut cursor = Self {
            cell,
            deltas,
            pending: VecDeque::from([Bytes::from(chunk(
                &id,
                created,
                &model,
                &json!({"role":"assistant", "content":""}),
                &Value::Null,
            ))]),
            id,
            created,
            model,
            last_sequence: 0,
            sent: String::new(),
            finished: false,
        };
        cursor.enqueue_replay(replay);
        cursor
    }

    async fn next(&mut self) -> Option<Bytes> {
        loop {
            if let Some(bytes) = self.pending.pop_front() {
                return Some(bytes);
            }
            if self.finished {
                return None;
            }
            if !self.cell.is_active() {
                let outcome = self.cell.wait().await;
                self.enqueue_finish(&outcome);
                continue;
            }
            tokio::select! {
                outcome = self.cell.wait() => self.enqueue_finish(&outcome),
                delta = self.deltas.recv() => match delta {
                    Ok((sequence, piece)) => self.enqueue_delta(sequence, &piece),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        self.enqueue_replay(self.cell.replay());
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let outcome = self.cell.wait().await;
                        self.enqueue_finish(&outcome);
                    }
                }
            }
        }
    }

    fn enqueue_delta(&mut self, sequence: u64, piece: &str) {
        if sequence <= self.last_sequence {
            return;
        }
        self.last_sequence = sequence;
        self.sent.push_str(piece);
        self.pending.push_back(Bytes::from(chunk(
            &self.id,
            self.created,
            &self.model,
            &json!({"content":piece}),
            &Value::Null,
        )));
    }

    fn enqueue_replay(&mut self, replay: Vec<(u64, String)>) {
        for (sequence, piece) in replay {
            self.enqueue_delta(sequence, &piece);
        }
    }

    fn enqueue_finish(&mut self, outcome: &TurnOutcome) {
        if self.finished {
            return;
        }
        // Reconcile the retained ordered log before terminal publication. This
        // preserves exact chunks even if settlement wins the select race.
        self.enqueue_replay(self.cell.replay());
        if outcome.error.is_none() {
            if let Some(remaining) = outcome.text.strip_prefix(&self.sent)
                && !remaining.is_empty()
            {
                self.enqueue_delta(self.last_sequence.saturating_add(1), remaining);
            }
            self.pending.push_back(Bytes::from(terminal_chunk(
                &self.id,
                self.created,
                &self.model,
                &outcome.usage,
            )));
        } else if let Some(error) = &outcome.error {
            self.pending.push_back(Bytes::from(format!(
                "data: {}\n\n",
                json!({"error":{"message":error, "type":"server_error"}})
            )));
        }
        self.pending
            .push_back(Bytes::from_static(b"data: [DONE]\n\n"));
        self.finished = true;
    }
}

fn terminal_chunk(id: &str, created: i64, model: &str, usage: &Usage) -> String {
    format!(
        "data: {}\n\n",
        json!({
            "id":id, "object":"chat.completion.chunk", "created":created, "model":model,
            "choices":[{"index":0, "delta":{}, "finish_reason":"stop"}],
            "usage":{
                "prompt_tokens":usage.prompt_tokens,
                "completion_tokens":usage.completion_tokens,
                "total_tokens":usage.total_tokens
            }
        })
    )
}

fn chunk(id: &str, created: i64, model: &str, delta: &Value, finish_reason: &Value) -> String {
    format!(
        "data: {}\n\n",
        json!({
            "id":id, "object":"chat.completion.chunk", "created":created, "model":model,
            "choices":[{"index":0, "delta":delta, "finish_reason":finish_reason}]
        })
    )
}

fn fresh_uuid() -> Uuid {
    static FALLBACK: AtomicU64 = AtomicU64::new(1);
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_ok() {
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    } else {
        Uuid::from_u128(u128::from(FALLBACK.fetch_add(1, Ordering::Relaxed)))
    }
}

fn invalid(message: impl Into<String>) -> Response {
    errors::response(StatusCode::BAD_REQUEST, errors::invalid_request(message))
}

fn server_failure(_: impl Into<String>) -> Response {
    errors::response(
        StatusCode::INTERNAL_SERVER_ERROR,
        errors::server_error("internal server error"),
    )
}

fn capacity_failure(message: &'static str) -> Response {
    errors::response(
        StatusCode::SERVICE_UNAVAILABLE,
        errors::server_error(message),
    )
}

fn validate_json_shape(value: &Value) -> Result<(), ChatInputError> {
    let mut stack = vec![(value, 1_usize)];
    let mut work = 0_usize;
    while let Some((current, depth)) = stack.pop() {
        work = work
            .checked_add(1)
            .ok_or_else(|| ChatInputError::new("request JSON exceeds maximum work"))?;
        if work > HTTP_BODY_BYTES_MAX {
            return Err(ChatInputError::new("request JSON exceeds maximum work"));
        }
        if depth > OPENAI_CHAT_JSON_DEPTH_MAX {
            return Err(ChatInputError::new("request JSON exceeds maximum depth"));
        }
        match current {
            Value::Array(values) => {
                stack.extend(values.iter().map(|child| (child, depth + 1)));
            }
            Value::Object(values) => {
                stack.extend(values.values().map(|child| (child, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
include!("chat_tests.rs");
