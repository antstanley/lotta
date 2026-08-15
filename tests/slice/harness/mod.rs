use lotta_app_server::config::ServerArgs;
use lotta_app_server::listener::start_listener_with_runtime_service_and_observer;
use lotta_app_server::observer::{RuntimeBroadcastObservation, RuntimeBroadcastObserver};
use lotta_app_server::ws::command::{
    AbortMessageCommand, ChangeDeviceStateCommand, InputCommand, RuntimeStartCommand, SyncCommand,
};
use lotta_app_server::ws::event::RuntimeEvent;
use lotta_app_server::ws::service::{
    AbortOutcome, DeviceStateOutcome, InputAdmission, RuntimeCommandService, RuntimeEventBatch,
    RuntimeEventSink, RuntimeStartOutcome, ServiceFuture, SyncOutcome,
};
use lotta_domain::{
    AgentId, BoundedJsonValue, BoundedVec, Clock, ConversationId, InputDisposition, LocalMessage,
    LocalMessageRole, MessageEntry, MessageEntryType, MessageId, ModelDescriptor, NonEmptyString,
    RunId, RuntimeScope, Timestamp, TranscriptEntry, TranscriptManifest,
};
use lotta_runtime::boundary::{ProviderEventText, ProviderText};
use lotta_runtime::ports::{
    ImagePolicy, PortFuture, ProviderContent, ProviderContentPart, ProviderDeadline, ProviderEvent,
    ProviderEventSink, ProviderMessage, ProviderMessageRole, ProviderMessages, ProviderPort,
    ProviderRequest, ProviderToolChoice, ReasoningControls, StopReason, TokenLimit, TranscriptItem,
    TranscriptStore,
};
use lotta_runtime::{
    ListenerRuntime, TurnEffectPort, TurnEvent, TurnPorts, TurnProjection, TurnToolCatalog,
    run_turn,
};
use lotta_testkit::contract::fixtures;
use lotta_testkit::fakes::{FakeProvider, FakeTool, FakeTranscriptStore};
use lotta_testkit::fixtures::traces::{
    FrameDirection, ReferenceTrace, TraceDriverProof, TraceFrame,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const HEAD: &str = "300f923f16cc8eee50656d7da732902c1dea2b65";
const TOKEN: &str = "task21-capability-token";
const TOKEN_SHA256: &str = "6542b8b241bf18de0dd4146d27c23d0f9730938ce4d66446322e7ca00ecf5d49";
const AGENT: &str = "00000000-0000-4000-8000-000000000001";
const CONVERSATION: &str = "00000000-0000-4000-8000-000000000002";
const TURN: &str = "00000000-0000-4000-8000-000000000090";
const RUN: &str = "00000000-0000-4000-8000-000000000091";
const CAUSE: &str = "client-message-slice";
const STDOUT_MAX: usize = 256 * 1024;
const STDERR_MAX: usize = 64 * 1024;
const PROVIDER_REQUESTS_MAX: usize = 1;
const TRANSCRIPT_ITEMS_MAX: usize = 3;

mod process;
pub use process::assert_failure_cleanup_and_timeout;
use process::run_client;

#[derive(Default)]
struct FixedClock(AtomicU64);
impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        let second = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Timestamp::parse_persisted_rfc3339(&format!("2000-01-01T00:00:{second:02}Z"))
            .expect("fixed increasing time")
    }
    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, lotta_domain::DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

#[derive(Default)]
pub struct Stats {
    pub runtime_resolutions: usize,
    pub retained_inputs: usize,
    pub provider_calls: usize,
    pub projections: usize,
    pub terminals: usize,
}

struct CountingProvider {
    inner: FakeProvider,
    calls: AtomicU64,
    requests: Mutex<BoundedVec<ProviderRequest, PROVIDER_REQUESTS_MAX>>,
}
impl CountingProvider {
    fn new() -> Self {
        let inner = FakeProvider::default();
        inner
            .configure(
                vec![
                    ProviderEvent::TextDelta {
                        text: ProviderEventText::new("hello from fake provider".into()).unwrap(),
                    },
                    ProviderEvent::Stop {
                        reason: StopReason::EndTurn,
                    },
                ],
                None,
            )
            .unwrap();
        Self {
            inner,
            calls: AtomicU64::new(0),
            requests: Mutex::new(BoundedVec::new(Vec::new()).expect("empty requests")),
        }
    }
}
impl ProviderPort for CountingProvider {
    fn stream(&self, request: ProviderRequest, events: ProviderEventSink) -> PortFuture<'_, ()> {
        let mut requests = self.requests.lock().unwrap();
        if requests.len() == PROVIDER_REQUESTS_MAX {
            return Box::pin(async {
                Err(lotta_runtime::RuntimeError::LimitExceeded {
                    context: "slice provider request observations".into(),
                })
            });
        }
        let mut retained = requests.as_slice().to_vec();
        if retained.try_reserve_exact(1).is_err() {
            return Box::pin(async {
                Err(lotta_runtime::RuntimeError::LimitExceeded {
                    context: "slice provider request observation allocation".into(),
                })
            });
        }
        retained.push(request.clone());
        *requests = BoundedVec::new(retained).expect("checked provider request bound");
        self.calls.fetch_add(1, Ordering::SeqCst);
        drop(requests);
        self.inner.stream(request, events)
    }
}

const AUTHORITY_OBSERVATIONS_MAX: usize = 7;
type AuthorityObservations = BoundedVec<AuthorityObservation, AUTHORITY_OBSERVATIONS_MAX>;
type ActiveObservation = (String, String, bool);

#[derive(Clone)]
enum AuthorityObservation {
    Lifecycle {
        kind: &'static str,
        cause: String,
        lease: String,
    },
    Broadcast {
        observation: RuntimeBroadcastObservation,
        active: Option<ActiveObservation>,
    },
}

struct ObservationLog {
    values: Mutex<AuthorityObservations>,
    failure: Mutex<Option<String>>,
    active: Mutex<Option<ActiveObservation>>,
}
impl Default for ObservationLog {
    fn default() -> Self {
        Self {
            values: Mutex::new(BoundedVec::new(Vec::new()).unwrap()),
            failure: Mutex::new(None),
            active: Mutex::new(None),
        }
    }
}
impl ObservationLog {
    fn fail(&self, message: &str) {
        if let Ok(mut failure) = self.failure.lock()
            && failure.is_none()
        {
            *failure = Some(message.to_owned());
        }
    }

    fn record(&self, value: AuthorityObservation) {
        let Ok(mut values) = self.values.lock() else {
            self.fail("authority observation lock poisoned");
            return;
        };
        if values.len() == AUTHORITY_OBSERVATIONS_MAX {
            drop(values);
            self.fail("authority observation bound exceeded");
            return;
        }
        let mut retained = values.as_slice().to_vec();
        if retained.try_reserve_exact(1).is_err() {
            drop(values);
            self.fail("authority observation allocation failed");
            return;
        }
        retained.push(value);
        let Ok(bounded) = BoundedVec::new(retained) else {
            drop(values);
            self.fail("authority observation bound exceeded");
            return;
        };
        *values = bounded;
    }
}
impl RuntimeBroadcastObserver for ObservationLog {
    fn observe(&self, observation: RuntimeBroadcastObservation) {
        let active = match self.active.lock() {
            Ok(value) => value.clone(),
            Err(_) => {
                self.fail("active observation lock poisoned");
                return;
            }
        };
        self.record(AuthorityObservation::Broadcast {
            observation,
            active,
        });
    }
}

struct Service {
    runtime: tokio::sync::Mutex<ListenerRuntime>,
    provider: CountingProvider,
    transcript: FakeTranscriptStore,
    tool: FakeTool,
    stats: Mutex<Stats>,
    observations: Arc<ObservationLog>,
    admitted: Mutex<Option<TranscriptEntry>>,
}
impl Service {
    fn new(observations: Arc<ObservationLog>) -> Arc<Self> {
        Arc::new(Self {
            runtime: tokio::sync::Mutex::new(ListenerRuntime::new()),
            provider: CountingProvider::new(),
            transcript: FakeTranscriptStore::default(),
            tool: FakeTool::default(),
            stats: Mutex::new(Stats::default()),
            observations,
            admitted: Mutex::new(None),
        })
    }
}

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept(AGENT).unwrap(),
        ConversationId::accept(CONVERSATION).unwrap(),
        None,
    )
}
fn events(values: Vec<RuntimeEvent>) -> RuntimeEventBatch {
    BoundedVec::new(values).unwrap()
}

impl RuntimeCommandService for Service {
    fn runtime_start(
        &self,
        command: RuntimeStartCommand,
    ) -> ServiceFuture<'_, RuntimeStartOutcome> {
        Box::pin(async move {
            if command.agent_id.as_ref().map(NonEmptyString::as_str) != Some(AGENT)
                || command.conversation_id.as_ref().map(NonEmptyString::as_str)
                    != Some(CONVERSATION)
            {
                return Err(lotta_app_server::error::AppServerError::Malformed);
            }
            let scope = scope();
            self.runtime
                .lock()
                .await
                .get_or_create(&scope, Uuid::from_u128(21))
                .map_err(|_| lotta_app_server::error::AppServerError::Internal)?;
            let (manifest, session, _) = fixtures::transcript();
            self.transcript
                .initialize(&scope.agent_id, &scope.conversation_id, &manifest, &session)
                .await
                .map_err(|_| lotta_app_server::error::AppServerError::Internal)?;
            let mut stats = self.stats.lock().unwrap();
            stats.runtime_resolutions = stats
                .runtime_resolutions
                .checked_add(1)
                .expect("runtime resolution count");
            Ok(RuntimeStartOutcome {
                runtime: scope,
                created_agent: false,
                created_conversation: false,
                agent: None,
                conversation: None,
                broadcasts: events(vec![
                    RuntimeEvent::UpdateDeviceStatus {
                        device_status: BoundedJsonValue::new(
                            json!({"mode":"standard","cwd":"/synthetic/workspace"}),
                        )
                        .unwrap(),
                    },
                    RuntimeEvent::UpdateLoopStatus {
                        loop_status: BoundedJsonValue::new(json!({
                            "status":"IDLE", "active_run_ids":[],
                            "executing_tool_call_ids":[]
                        }))
                        .unwrap(),
                    },
                    RuntimeEvent::UpdateQueue {
                        queue: BoundedJsonValue::new(json!([])).unwrap(),
                        removed: BoundedJsonValue::new(json!([])).unwrap(),
                    },
                ]),
            })
        })
    }

    fn admit_input(&self, command: InputCommand) -> ServiceFuture<'_, InputAdmission> {
        Box::pin(async move {
            if command.runtime != scope() || continuation_text(command.payload.as_value()).is_err()
            {
                return Err(lotta_app_server::error::AppServerError::Malformed);
            }
            let appended = admitted_message(command.payload.as_value())
                .map_err(|()| lotta_app_server::error::AppServerError::Malformed)?;
            self.transcript
                .append(
                    &command.runtime.agent_id,
                    &command.runtime.conversation_id,
                    &appended,
                )
                .await
                .map_err(|_| lotta_app_server::error::AppServerError::Internal)?;
            *self.admitted.lock().unwrap() = Some(appended);
            Ok(InputAdmission {
                disposition: InputDisposition::Started,
                error: None,
                continuation: Some(command.payload),
                after_ack: events(Vec::new()),
            })
        })
    }

    fn continue_input(
        &self,
        scope: RuntimeScope,
        continuation: Option<BoundedJsonValue>,
        sink: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        Box::pin(async move {
            let continuation =
                continuation.ok_or(lotta_app_server::error::AppServerError::Malformed)?;
            let text = continuation_text(continuation.as_value())
                .map_err(|()| lotta_app_server::error::AppServerError::Malformed)?;
            let mut runtime = self.runtime.lock().await;
            let handle = runtime
                .get_or_create(&scope, Uuid::from_u128(21))
                .map_err(|_| lotta_app_server::error::AppServerError::Internal)?;
            let lease = runtime
                .lifecycle_mut(&handle)
                .map_err(|_| lotta_app_server::error::AppServerError::Internal)?
                .begin_turn(TURN.into(), RunId::accept(RUN).unwrap())
                .map_err(|_| lotta_app_server::error::AppServerError::Internal)?;
            let lease_label = if lease.generation() == 1 {
                "lease-slice".to_owned()
            } else {
                format!("lease-generation-{}", lease.generation())
            };
            let cause = continuation.as_value()["messages"][0]["client_message_id"]
                .as_str()
                .ok_or(lotta_app_server::error::AppServerError::Malformed)?
                .to_owned();
            self.observations.record(AuthorityObservation::Lifecycle {
                kind: "turn_started",
                cause: cause.clone(),
                lease: lease_label.clone(),
            });
            self.observations.record(AuthorityObservation::Lifecycle {
                kind: "lease_acquired",
                cause: cause.clone(),
                lease: lease_label.clone(),
            });
            *self.observations.active.lock().unwrap() = Some((cause, lease_label, true));
            let effects = Effects {
                scope: &scope,
                sink,
                stats: &self.stats,
            };
            let result = run_turn(
                &mut runtime,
                handle,
                lease,
                provider_request(&text),
                TurnPorts::new(
                    &self.provider,
                    &self.tool,
                    &TurnToolCatalog::new(Vec::new()).unwrap(),
                    &effects,
                ),
            )
            .await;
            *self.observations.active.lock().unwrap() = None;
            result
                .map(|_| ())
                .map_err(|_| lotta_app_server::error::AppServerError::Internal)
        })
    }

    fn sync(&self, _: SyncCommand) -> ServiceFuture<'_, SyncOutcome> {
        Box::pin(async {
            Ok(SyncOutcome {
                broadcasts: events(Vec::new()),
            })
        })
    }
    fn abort_message(&self, _: AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome> {
        Box::pin(async { Ok(AbortOutcome { aborted: false }) })
    }
    fn change_device_state(
        &self,
        _: ChangeDeviceStateCommand,
    ) -> ServiceFuture<'_, DeviceStateOutcome> {
        Box::pin(async {
            Ok(DeviceStateOutcome {
                broadcasts: events(Vec::new()),
            })
        })
    }
}

struct Effects<'a> {
    scope: &'a RuntimeScope,
    sink: Arc<dyn RuntimeEventSink>,
    stats: &'a Mutex<Stats>,
}
impl TurnEffectPort for Effects<'_> {
    fn persist_projection(&self, _: TurnProjection) -> Result<(), lotta_runtime::RuntimeError> {
        let mut stats = self.stats.lock().unwrap();
        stats.projections = stats.projections.checked_add(1).expect("projection count");
        Ok(())
    }
    fn emit(&self, event: TurnEvent) -> Result<(), lotta_runtime::RuntimeError> {
        let wire = match event {
            TurnEvent::StreamDelta(_) => RuntimeEvent::StreamDelta {
                delta: BoundedJsonValue::new(json!({
                    "id":"00000000-0000-4000-8000-000000000022",
                    "date":"2000-01-01T00:00:12.000Z",
                    "message_type":"message"
                }))
                .unwrap(),
                subagent_id: None,
            },
            TurnEvent::Finished { reason } => {
                let mut stats = self.stats.lock().unwrap();
                stats.terminals = stats.terminals.checked_add(1).expect("terminal count");
                RuntimeEvent::TurnFinished {
                    turn_id: NonEmptyString::new(TURN).unwrap(),
                    run_id: Some(RunId::accept(RUN).unwrap()),
                    stop_reason: NonEmptyString::new(stop_reason(reason)).unwrap(),
                    error: None,
                }
            }
            TurnEvent::ToolResult(_) => return Ok(()),
        };
        self.sink
            .emit(self.scope, wire)
            .map_err(|_| lotta_runtime::RuntimeError::AdapterFailure {
                code: "slice_event_sink",
                context: "runtime event sink".into(),
            })
    }
    fn append_tool_result(
        &self,
        _: lotta_runtime::ToolResultRecord,
    ) -> Result<(), lotta_runtime::RuntimeError> {
        Ok(())
    }
}

fn stop_reason(reason: StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::OutputLimit => "output_limit",
        StopReason::ToolUse => "tool_use",
        StopReason::ContentFilter => "content_filter",
        StopReason::Other => "other",
    }
    .into()
}

fn continuation_text(value: &Value) -> Result<String, ()> {
    if value.get("kind").and_then(Value::as_str) != Some("create_message") {
        return Err(());
    }
    let messages = value.get("messages").and_then(Value::as_array).ok_or(())?;
    if messages.len() != 1
        || messages[0].get("role").and_then(Value::as_str) != Some("user")
        || messages[0].get("client_message_id").and_then(Value::as_str) != Some(CAUSE)
    {
        return Err(());
    }
    messages[0]
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or(())
}

fn admitted_message(value: &Value) -> Result<TranscriptEntry, ()> {
    let message = value
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.first())
        .ok_or(())?;
    let id = message
        .get("client_message_id")
        .and_then(Value::as_str)
        .ok_or(())?;
    let text = message.get("content").and_then(Value::as_str).ok_or(())?;
    let timestamp = Timestamp::parse_persisted_rfc3339("2000-01-01T00:00:10Z").map_err(|_| ())?;
    let message_id = MessageId::accept(id).map_err(|_| ())?;
    Ok(TranscriptEntry::Message(MessageEntry {
        entry_type: MessageEntryType::Message,
        id: NonEmptyString::new(id).map_err(|_| ())?,
        parent_id: Some("session".into()),
        timestamp,
        message: LocalMessage {
            id: message_id,
            role: LocalMessageRole::User,
            content: Some(BoundedJsonValue::new(json!(text)).map_err(|_| ())?),
            timestamp: 946_684_810_000.0,
            metadata: None,
        },
    }))
}

fn provider_request(text: &str) -> ProviderRequest {
    let message = ProviderMessage {
        role: ProviderMessageRole::User,
        content: ProviderContent::new(vec![ProviderContentPart::Text(
            ProviderText::new(text.into()).unwrap(),
        )])
        .unwrap(),
        tool_call_id: None,
    };
    ProviderRequest {
        model: ModelDescriptor {
            handle: NonEmptyString::new("fake").unwrap(),
            provider_id: NonEmptyString::new("openai").unwrap(),
            available: true,
            context_window: Some(4096),
            model_settings: None,
        },
        system_prompt: None,
        messages: ProviderMessages::new(vec![message]).unwrap(),
        tools: BoundedVec::new(Vec::new()).unwrap(),
        tool_choice: ProviderToolChoice::None,
        image_policy: ImagePolicy::Strict,
        context_tokens_max: TokenLimit::new(1024).unwrap(),
        output_tokens_max: TokenLimit::new(1024).unwrap(),
        reasoning: ReasoningControls {
            enabled: false,
            effort: None,
            tier: None,
        },
        cancellation: CancellationToken::new(),
        deadline: ProviderDeadline::new(Duration::from_secs(5)).unwrap(),
    }
}

#[derive(Deserialize)]
struct RawFrame {
    direction: String,
    wire: Value,
}

pub struct Capture {
    pub trace: ReferenceTrace,
    pub stats: Stats,
    pub tool_calls: u32,
    pub checkout: PathBuf,
    pub transcript: BoundedVec<TranscriptItem, TRANSCRIPT_ITEMS_MAX>,
    pub expected_manifest: TranscriptManifest,
    pub expected_session: TranscriptEntry,
    pub expected_appended: TranscriptEntry,
    pub provider_requests: BoundedVec<ProviderRequest, PROVIDER_REQUESTS_MAX>,
}

pub async fn capture() -> Capture {
    capture_result()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
}

pub async fn capture_result() -> Result<Capture, String> {
    let checkout = checkout();
    verify_checkout(&checkout);
    let observations = Arc::new(ObservationLog::default());
    let service = Service::new(observations.clone());
    let prepared = ServerArgs {
        listen_enabled: true,
        listen: Some("ws://127.0.0.1:0/ws".into()),
        ws_auth: Some("capability-token".into()),
        ws_token_sha256: Some(TOKEN_SHA256.into()),
        ..ServerArgs::default()
    }
    .prepare()
    .unwrap();
    let mut listener = start_listener_with_runtime_service_and_observer(
        prepared,
        Arc::new(FixedClock::default()),
        service.clone(),
        observations.clone(),
    )
    .await
    .map_err(|error| error.to_string())?;
    let client_checkout = checkout.clone();
    let client_url = listener.websocket_url().to_owned();
    let client_result =
        tokio::task::spawn_blocking(move || run_client(&client_checkout, &client_url)).await;
    listener.shutdown();
    let listener_result = listener.wait().await;
    let raw = client_result
        .map_err(|_| "join Bun client".to_owned())?
        .map_err(|error| format!("client failed after listener wait: {error}"))?;
    listener_result.map_err(|error| format!("listener wait failed: {error}"))?;
    verify_checkout(&checkout);
    let transcript = collect_transcript(&service.transcript).await;
    let expected_appended = service
        .admitted
        .lock()
        .unwrap()
        .clone()
        .expect("actual admitted transcript entry");
    let (expected_manifest, expected_session, _) = fixtures::transcript();
    let mut stats = std::mem::take(&mut *service.stats.lock().unwrap());
    stats.provider_calls = usize::try_from(service.provider.calls.load(Ordering::SeqCst)).unwrap();
    stats.retained_inputs = transcript
        .as_slice()
        .iter()
        .filter(|item| matches!(item, TranscriptItem::Entry(TranscriptEntry::Message(_))))
        .count();
    let provider_requests = service.provider.requests.lock().unwrap().clone();
    if let Some(error) = observations.failure.lock().unwrap().clone() {
        return Err(error);
    }
    let authority = observations.values.lock().unwrap().clone();
    Ok(Capture {
        trace: trace(raw, authority.as_slice())?,
        stats,
        tool_calls: service.tool.completed_calls(),
        checkout,
        transcript,
        expected_manifest,
        expected_session,
        expected_appended,
        provider_requests,
    })
}

async fn collect_transcript(
    store: &FakeTranscriptStore,
) -> BoundedVec<TranscriptItem, TRANSCRIPT_ITEMS_MAX> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(TRANSCRIPT_ITEMS_MAX);
    let scope = scope();
    let load = store.load(
        &scope.agent_id,
        &scope.conversation_id,
        sender,
        CancellationToken::new(),
    );
    let collect = async move {
        let mut values = Vec::new();
        values
            .try_reserve_exact(TRANSCRIPT_ITEMS_MAX)
            .expect("bounded transcript observation allocation");
        while let Some(value) = receiver.recv().await {
            assert!(
                values.len() < TRANSCRIPT_ITEMS_MAX,
                "transcript observation bound"
            );
            values.push(value);
        }
        BoundedVec::new(values).expect("checked transcript observation bound")
    };
    let (result, values) = tokio::join!(load, collect);
    result.unwrap();
    values
}

fn checkout() -> PathBuf {
    if let Some(candidate) = std::env::var_os("LOTTA_LETTA_CODE_CHECKOUT") {
        return PathBuf::from(candidate)
            .canonicalize()
            .expect("canonical configured pinned checkout");
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    ["../letta-code", "../../letta-code"]
        .into_iter()
        .map(|relative| manifest.join(relative))
        .find(|candidate| candidate.join("src/app-server-client.ts").is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
        .expect("canonical pinned checkout beside main or jj workspace")
}
fn verify_checkout(path: &Path) {
    let head = Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git head");
    assert!(head.status.success());
    assert_eq!(String::from_utf8(head.stdout).unwrap().trim(), HEAD);
    let status = Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["status", "--porcelain"])
        .output()
        .expect("git status");
    assert!(status.status.success());
    assert!(
        status.stdout.is_empty(),
        "pinned checkout must remain clean"
    );
}

fn raw_frame(raw: RawFrame, cause: Option<String>) -> Result<TraceFrame, String> {
    let direction = match raw.direction.as_str() {
        "lifecycle" => FrameDirection::Lifecycle,
        "client_to_server" => FrameDirection::ClientToServer,
        "server_to_client" => FrameDirection::ServerToClient,
        _ => return Err("unknown client observation direction".into()),
    };
    Ok(TraceFrame {
        frame_index: 0,
        direction,
        connection_ordinal: 1,
        caused_by: cause,
        lease_id: None,
        lease_current: None,
        broadcast_emission: None,
        wire: BoundedJsonValue::new(raw.wire).map_err(|_| "raw frame bound".to_owned())?,
    })
}

fn broadcast_frames(
    observation: &RuntimeBroadcastObservation,
    active: &Option<ActiveObservation>,
) -> Result<Vec<TraceFrame>, String> {
    let (cause, lease, current) = active
        .as_ref()
        .map_or((None, None, None), |(cause, lease, current)| {
            (Some(cause.clone()), Some(lease.clone()), Some(*current))
        });
    let emission = format!("emission-{}", observation.batch_ordinal);
    let logical = serde_json::to_value(&observation.event)
        .map_err(|_| "serialize logical RuntimeEvent".to_owned())?;
    let subscribers = observation
        .deliveries
        .as_slice()
        .iter()
        .map(|delivery| delivery.ordinal)
        .collect::<Vec<_>>();
    let mut frames = vec![TraceFrame {
        frame_index: 0,
        direction: FrameDirection::Lifecycle,
        connection_ordinal: subscribers.first().copied().unwrap_or(1) as u32,
        caused_by: cause.clone(),
        lease_id: lease.clone(),
        lease_current: None,
        broadcast_emission: None,
        wire: BoundedJsonValue::new(json!({
            "type":"broadcast_begin",
            "emission":emission,
            "message_type":observation.event.discriminant(),
            "subscriber_ordinals":subscribers,
            "payload_sha256":logical_payload_sha256(&logical),
        }))
        .map_err(|_| "broadcast observation bound".to_owned())?,
    }];
    for delivery in observation.deliveries.as_slice() {
        frames.push(TraceFrame {
            frame_index: 0,
            direction: FrameDirection::ServerToClient,
            connection_ordinal: u32::try_from(delivery.ordinal)
                .map_err(|_| "connection ordinal bound".to_owned())?,
            caused_by: cause.clone(),
            lease_id: lease.clone(),
            lease_current: current,
            broadcast_emission: Some(emission.clone()),
            wire: BoundedJsonValue::new(
                serde_json::to_value(&delivery.frame)
                    .map_err(|_| "serialize observed delivery frame".to_owned())?,
            )
            .map_err(|_| "delivery observation bound".to_owned())?,
        });
    }
    Ok(frames)
}

fn trace_frames(
    raw: Vec<RawFrame>,
    authority: &[AuthorityObservation],
) -> Result<Vec<TraceFrame>, String> {
    let mut authority_frames = Vec::new();
    for observation in authority {
        match observation {
            AuthorityObservation::Lifecycle { kind, cause, lease } => {
                authority_frames.push(lifecycle_frame(kind, cause, lease))
            }
            AuthorityObservation::Broadcast {
                observation,
                active,
            } => {
                let is_initial = observation.batch_ordinal <= 3;
                if is_initial != active.is_none() {
                    return Err("invalid captured causal context for observer batch".into());
                }
                if !is_initial && !active.as_ref().is_some_and(|value| value.2) {
                    return Err("stream or terminal batch lacks current lease context".into());
                }
                authority_frames.extend(broadcast_frames(observation, active)?);
            }
        }
    }
    let ordinals = authority
        .iter()
        .filter_map(|value| match value {
            AuthorityObservation::Broadcast { observation, .. } => Some(observation.batch_ordinal),
            AuthorityObservation::Lifecycle { .. } => None,
        })
        .collect::<Vec<_>>();
    if ordinals != [1, 2, 3, 4, 5] {
        return Err(format!("observer batch ordinals mismatch: {ordinals:?}"));
    }
    let mut raw_messages = raw.iter().filter(|frame| {
        frame.direction == "server_to_client" && frame.wire.get("event_seq").is_some()
    });
    for frame in authority_frames
        .iter()
        .filter(|frame| frame.direction == FrameDirection::ServerToClient)
    {
        let observed = raw_messages
            .next()
            .ok_or("missing client onMessage frame")?;
        if observed.wire != *frame.wire.as_value() {
            return Err(format!(
                "observer delivery/client onMessage JSON mismatch: observed={} authority={}",
                observed.wire,
                frame.wire.as_value()
            ));
        }
    }
    if raw_messages.next().is_some() {
        return Err("extra client onMessage frame".into());
    }
    let mut frames = Vec::new();
    let mut authority = authority_frames.into_iter().peekable();
    let mut request_causes = std::collections::VecDeque::with_capacity(8);
    for raw in raw {
        if raw.direction == "server_to_client" && raw.wire.get("event_seq").is_some() {
            while authority
                .peek()
                .is_some_and(|frame| frame.direction != FrameDirection::ServerToClient)
            {
                frames.push(authority.next().ok_or("authority ordering")?);
            }
            frames.push(authority.next().ok_or("missing authority delivery")?);
        } else {
            let kind = raw.wire["type"].as_str().unwrap_or_default();
            let cause = if kind == "input" {
                let request_id = raw.wire["request_id"]
                    .as_str()
                    .ok_or("input request id missing")?;
                let cause = raw.wire["payload"]["messages"][0]["client_message_id"]
                    .as_str()
                    .ok_or("input client message id missing")?
                    .to_owned();
                if request_causes.len() == 8 {
                    request_causes.pop_front();
                }
                request_causes.push_back((request_id.to_owned(), cause.clone()));
                Some(cause)
            } else if kind == "input_accepted" {
                let request_id = raw.wire["request_id"]
                    .as_str()
                    .ok_or("input accepted request id missing")?;
                request_causes
                    .iter()
                    .find(|(observed, _)| observed == request_id)
                    .map(|(_, cause)| cause.clone())
                    .ok_or("input accepted cause correlation missing")?
                    .into()
            } else {
                None
            };
            frames.push(raw_frame(raw, cause)?);
        }
    }
    frames.extend(authority);
    Ok(frames)
}

fn trace(raw: Vec<RawFrame>, authority: &[AuthorityObservation]) -> Result<ReferenceTrace, String> {
    let expected = lotta_testkit::fixtures::traces::load_trace("vertical-slice")
        .map_err(|error| error.to_string())?;
    let mut frames = trace_frames(raw, authority)?;
    for (index, frame) in frames.iter_mut().enumerate() {
        frame.frame_index = index;
    }
    let commands = frames
        .iter()
        .filter(|frame| frame.direction == FrameDirection::ClientToServer)
        .filter_map(|frame| frame.wire.as_value()["type"].as_str().map(str::to_owned))
        .collect();
    let messages = frames
        .iter()
        .filter(|frame| frame.direction == FrameDirection::ServerToClient)
        .filter_map(|frame| frame.wire.as_value()["type"].as_str().map(str::to_owned))
        .collect();
    Ok(ReferenceTrace {
        schema_version: 1,
        name: "slice_happy_turn".into(),
        kind: "vertical_slice".into(),
        provenance: expected.provenance,
        supporting_provenance: expected.supporting_provenance,
        driver_proof: TraceDriverProof {
            command_types: BoundedVec::new(commands).map_err(|_| "command proof bound")?,
            message_types: BoundedVec::new(messages).map_err(|_| "message proof bound")?,
        },
        frames: BoundedVec::new(frames).map_err(|_| "trace frame bound")?,
    })
}

fn lifecycle_frame(kind: &str, cause: &str, lease: &str) -> TraceFrame {
    TraceFrame {
        frame_index: 0,
        direction: FrameDirection::Lifecycle,
        connection_ordinal: 1,
        caused_by: Some(cause.into()),
        lease_id: Some(lease.into()),
        lease_current: None,
        broadcast_emission: None,
        wire: BoundedJsonValue::new(json!({
            "type":kind, "caused_by":cause, "lease_id":lease,
        }))
        .expect("bounded lifecycle observation"),
    }
}

fn logical_payload_sha256(value: &Value) -> String {
    let mut value = value.clone();
    sort_value(&mut value);
    let canonical = serde_json::to_vec(&value).expect("bounded logical event");
    format!("{:x}", Sha256::digest(canonical))
}
fn sort_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let old = std::mem::take(map);
            let mut entries = old.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, mut child) in entries {
                sort_value(&mut child);
                map.insert(key, child);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(sort_value),
        _ => {}
    }
}
