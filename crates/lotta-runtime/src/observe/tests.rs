use self::events::{BoundedEventSink, EventSinkError};
use self::metrics::{METRIC_FAMILY_NAMES, MetricValue};
use crate::boundary::ProviderText;
use crate::ports::{PortFuture, ProviderContent, ProviderContentPart, ProviderContext, ProviderContextOverflowDetail, ProviderError, ProviderErrorContext, ProviderEvent, ProviderMessage, ProviderMessageRole, ProviderRequest, StopReason, ToolExecutionRequest, ToolOutcome, ToolPort};
use crate::retry::{Clock, EventSink, ProviderRoute, RetryEvent, RetryExecutor, RetryPolicy, Sleeper};
use crate::turn::test_support::{RecordingEffects, ScriptedProvider, SequencingTool, call_events, catalog, outcome, request, text as provider_text};
use crate::turn::{CompactionPort, CompactionProgress, RequestRefreshPort, TurnProvider};
use crate::{AdmissionRequest, AdmissionRoute, RuntimeError, TurnPorts, TurnRunOutcome, run_turn};
use lotta_domain::{AgentId, BoundedJsonValue, BoundedVec, ConversationId, EntityExtras, QueueItem, QueueItemKind, QueueItemSource, RuntimeScope, Timestamp};
use serde_json::json;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const MARKERS: [&str; 5] = [
    "PROMPT_UNIQUE_MARKER",
    "BODY_UNIQUE_MARKER",
    "TOOL_INPUT_UNIQUE_MARKER",
    "CREDENTIAL_UNIQUE_MARKER",
    "SUBSTITUTED_COMMAND_UNIQUE_MARKER",
];

#[derive(Default)]
struct CaptureSink(Mutex<Vec<RuntimeEvent>>);

impl RuntimeEventSink for CaptureSink {
    fn try_emit(&self, event: RuntimeEvent) -> Result<(), EventSinkError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

impl CaptureSink {
    fn events(&self) -> Vec<RuntimeEvent> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct MarkerTool {
    inputs: Mutex<Vec<serde_json::Value>>,
}

impl ToolPort for MarkerTool {
    fn execute(&self, request: ToolExecutionRequest) -> crate::ports::PortFuture<'_, ToolOutcome> {
        self.inputs.lock().unwrap().push(request.input.as_value().clone());
        Box::pin(async { Ok(outcome("tool-ok")) })
    }
}

struct Evidence {
    sink: Arc<CaptureSink>,
    observer: RuntimeObserver,
    provider_requests: Vec<crate::ports::ProviderRequest>,
    tool_inputs: Vec<serde_json::Value>,
}

struct ContextSeam {
    compactions: AtomicUsize,
    refreshes: AtomicUsize,
    requests: Mutex<VecDeque<ProviderRequest>>,
}

impl ContextSeam {
    fn new(requests: Vec<ProviderRequest>) -> Self {
        Self {
            compactions: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
            requests: Mutex::new(requests.into()),
        }
    }
}

impl CompactionPort for ContextSeam {
    fn compact(
        &self,
        request: ProviderRequest,
        _: lotta_domain::TurnLease,
        _: ProviderContextOverflowDetail,
        _: crate::compaction::CompactionTrigger,
    ) -> PortFuture<'_, CompactionProgress> {
        self.compactions.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(CompactionProgress {
                tokens_before: request.normalized_wire_bytes()? as u64,
                tokens_after: 1,
                messages_before: request.messages.len(),
                messages_after: 0,
            })
        })
    }
}

impl RequestRefreshPort for ContextSeam {
    fn refresh(&self, _: ProviderRequest, _: lotta_domain::TurnLease) -> PortFuture<'static, ProviderRequest> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        let request = self.requests.lock().unwrap().pop_front();
        Box::pin(async move {
            request.ok_or_else(|| RuntimeError::InvalidData {
                context: "missing refreshed request".into(),
            })
        })
    }
}

#[derive(Default)]
struct RetryTime(AtomicUsize);

impl Clock for RetryTime {
    fn monotonic_ms(&self) -> u64 { self.0.load(Ordering::SeqCst) as u64 }
    fn unix_epoch_ms(&self) -> u64 { self.monotonic_ms() }
}

impl Sleeper for RetryTime {
    fn sleep<'a>(
        &'a self,
        duration: Duration,
        cancellation: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(RuntimeError::Cancelled { context: "retry evidence".into() });
            }
            self.0.fetch_add(duration.as_millis() as usize, Ordering::SeqCst);
            Ok(())
        })
    }
}

struct RetryNotices;
impl EventSink for RetryNotices {
    fn emit(&self, _: RetryEvent) -> PortFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct CancellingProvider(CancellationToken);
impl crate::ports::ProviderPort for CancellingProvider {
    fn stream(
        &self,
        _: ProviderRequest,
        _: crate::ports::ProviderEventSink,
    ) -> PortFuture<'_, ()> {
        self.0.cancel();
        Box::pin(async { Ok(()) })
    }
}

fn observed_runtime(observer: RuntimeObserver) -> (crate::ListenerRuntime, crate::RuntimeHandle) {
    let mut runtime = crate::ListenerRuntime::with_observer(observer);
    let handle = runtime
        .get_or_create(&RuntimeScope::new(
            AgentId::accept("observe-agent").unwrap(),
            ConversationId::accept("observe-conversation").unwrap(),
            None,
        ), uuid::Uuid::from_u128(59))
        .unwrap();
    (runtime, handle)
}

fn queue_item(number: usize) -> QueueItem {
    QueueItem {
        id: NonEmptyString::new(format!("observe-item-{number}")).unwrap(),
        client_message_id: NonEmptyString::new(format!("observe-client-{number}")).unwrap(),
        kind: QueueItemKind::Message,
        source: QueueItemSource::User,
        content: BoundedJsonValue::new(json!({"body": BODY_UNIQUE_MARKER})).unwrap(),
        enqueued_at: Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56Z").unwrap(),
        extras: EntityExtras::default(),
    }
}

const BODY_UNIQUE_MARKER: &str = "BODY_UNIQUE_MARKER";

fn marked_request() -> crate::ports::ProviderRequest {
    let mut value = request();
    value.system_prompt = Some(ProviderText::new(
        "PROMPT_UNIQUE_MARKER config=CREDENTIAL_UNIQUE_MARKER command=SUBSTITUTED_COMMAND_UNIQUE_MARKER"
            .to_owned(),
    ).unwrap());
    value.model.model_settings = Some(serde_json::from_value(json!({
        "credential": "CREDENTIAL_UNIQUE_MARKER",
        "config": "test-config-CREDENTIAL_UNIQUE_MARKER"
    })).unwrap());
    value.messages = BoundedVec::new(vec![ProviderMessage {
        role: ProviderMessageRole::User,
        content: ProviderContent::new(vec![ProviderContentPart::Text(
            ProviderText::new(BODY_UNIQUE_MARKER.into()).unwrap(),
        )]).unwrap(),
        tool_call_id: None,
    }]).unwrap();
    value
}

fn pressured_request() -> ProviderRequest {
    let mut value = request();
    value.system_prompt = Some(ProviderText::new("x".repeat(12_000)).unwrap());
    value.context = Some(ProviderContext {
        server_max: Some(1_400),
        catalog_max: Some(1_300),
        agent_max: Some(1_200),
        conversation_max: Some(1_000),
        ..ProviderContext::default()
    });
    value
}

fn compacted_request() -> ProviderRequest {
    let mut value = request();
    value.output_tokens_max = crate::ports::TokenLimit::new(64).unwrap();
    value.context = Some(ProviderContext {
        server_max: Some(4_096),
        catalog_max: Some(4_096),
        ..ProviderContext::default()
    });
    value
}

fn successful_events() -> Vec<ProviderEvent> {
    vec![ProviderEvent::Stop { reason: StopReason::EndTurn }]
}

fn retryable_events() -> Vec<ProviderEvent> {
    vec![ProviderEvent::Error {
        error: ProviderError::Overloaded(ProviderErrorContext {
            retry_after: None,
            code: crate::boundary::ProviderName::new("retryable".into()).unwrap(),
            context: provider_text("temporary"),
        }),
    }]
}

fn begin_evidence_turn(
    runtime: &mut crate::ListenerRuntime,
    handle: &crate::RuntimeHandle,
    turn: &str,
    sequence: u64,
) -> lotta_domain::TurnLease {
    runtime.lifecycle_mut(handle).unwrap()
        .begin_turn(turn.into(), RunId::generate_sequence(sequence).unwrap()).unwrap()
}

async fn real_path_evidence() -> Evidence {
    let sink = Arc::new(CaptureSink::default());
    let observer = RuntimeObserver::new(sink.clone());
    let (mut runtime, handle) = observed_runtime(observer.clone());
    let (provider_requests, tool_inputs) = admission_and_tool_scenario(&mut runtime, &handle).await;
    stale_suppression_scenario(&mut runtime, &handle).await;
    compaction_scenario(&mut runtime, &handle).await;
    retry_scenario(&mut runtime, &handle).await;
    cancellation_scenario(&mut runtime, handle).await;
    observer.set_gauge(MetricFamily::QueueDepth, 0);

    Evidence {
        sink,
        observer,
        provider_requests,
        tool_inputs,
    }
}

async fn admission_and_tool_scenario(
    runtime: &mut crate::ListenerRuntime,
    handle: &crate::RuntimeHandle,
) -> (Vec<ProviderRequest>, Vec<serde_json::Value>) {
    let start = runtime.admit(handle, AdmissionRequest {
        item: queue_item(1), route: AdmissionRoute::Ordinary,
    }).unwrap();
    assert!(matches!(start, crate::AdmissionOutcome::Start(_)));
    let lease = runtime.lifecycle_mut(handle).unwrap()
        .begin_turn("observe-turn".into(), RunId::generate_sequence(1).unwrap()).unwrap();
    let queued = runtime.admit(handle, AdmissionRequest {
        item: queue_item(2), route: AdmissionRoute::Ordinary,
    }).unwrap();
    assert!(matches!(queued, crate::AdmissionOutcome::Queued(_)));

    let provider = ScriptedProvider::new(vec![{
        let mut events = call_events("observe-call", "shell");
        if let ProviderEvent::ToolCallArgumentsDelta { chunk: value, .. } = &mut events[1] {
            *value = crate::boundary::ToolArgumentChunk::new(
                br#"{"input":"TOOL_INPUT_UNIQUE_MARKER","command":"SUBSTITUTED_COMMAND_UNIQUE_MARKER"}"#.to_vec()
            ).unwrap();
        }
        events.push(ProviderEvent::Stop { reason: StopReason::ToolUse });
        events
    }, vec![ProviderEvent::Stop { reason: StopReason::EndTurn }]]);
    let tool = MarkerTool::default();
    let effects = RecordingEffects::default();
    let tools = catalog(&["shell"]);
    let outcome_value = run_turn(runtime, handle.clone(), lease.clone(), marked_request(),
        TurnPorts::new(&provider, &tool, &tools, &effects)).await.unwrap();
    assert_eq!(outcome_value, TurnRunOutcome::Completed);
    (
        provider.requests.lock().unwrap().clone(),
        tool.inputs.lock().unwrap().clone(),
    )
}

async fn stale_suppression_scenario(
    runtime: &mut crate::ListenerRuntime,
    handle: &crate::RuntimeHandle,
) {
    let pumped = runtime.pump_queue(handle).unwrap().unwrap();
    let pumped_id = match pumped.mutation().event() {
        crate::QueueMutationEvent::Pumped(items) => items[0].id.clone(),
        other => panic!("unexpected pump: {other:?}"),
    };
    let stale_lease = runtime.lifecycle_mut(handle).unwrap()
        .begin_turn(pumped_id.as_str().into(), RunId::generate_sequence(2).unwrap()).unwrap();
    runtime.lifecycle_mut(handle).unwrap()
        .finish_turn(&stale_lease, lotta_domain::StopReason::new("superseded").unwrap()).unwrap();
    let current = runtime.lifecycle_mut(handle).unwrap()
        .begin_turn("replacement".into(), RunId::generate_sequence(3).unwrap()).unwrap();

    let stale_provider = ScriptedProvider::new(vec![vec![ProviderEvent::Stop {
        reason: StopReason::EndTurn,
    }]]);
    let stale_tool = SequencingTool::new(Vec::new());
    let stale_effects = RecordingEffects::default();
    let empty = catalog(&[]);
    let stale = run_turn(runtime, handle.clone(), stale_lease, request(),
        TurnPorts::new(&stale_provider, &stale_tool, &empty, &stale_effects)).await.unwrap();
    assert_eq!(stale, TurnRunOutcome::Suppressed);
    runtime.lifecycle_mut(handle).unwrap()
        .finish_turn(&current, lotta_domain::StopReason::new("replacement_done").unwrap()).unwrap();
}

async fn compaction_scenario(
    runtime: &mut crate::ListenerRuntime,
    handle: &crate::RuntimeHandle,
) {
    let empty = catalog(&[]);
    let compaction_lease = begin_evidence_turn(runtime, handle, "compaction-turn", 4);
    let compaction_provider = ScriptedProvider::new(vec![successful_events()]);
    let compaction_tool = SequencingTool::new(Vec::new());
    let compaction_effects = RecordingEffects::default();
    let seam = Arc::new(ContextSeam::new(vec![compacted_request()]));
    let compaction_ports = TurnPorts::new(
        &compaction_provider, &compaction_tool, &empty, &compaction_effects,
    ).with_context_ports(seam.as_ref(), Arc::clone(&seam) as _);
    assert_eq!(
        run_turn(runtime, handle.clone(), compaction_lease, pressured_request(), compaction_ports).await.unwrap(),
        TurnRunOutcome::Completed,
    );
    assert_eq!(seam.compactions.load(Ordering::SeqCst), 1);
    assert_eq!(seam.refreshes.load(Ordering::SeqCst), 1);
}

async fn retry_scenario(
    runtime: &mut crate::ListenerRuntime,
    handle: &crate::RuntimeHandle,
) {
    let empty = catalog(&[]);
    let retry_lease = begin_evidence_turn(runtime, handle, "retry-turn", 5);
    let retry_provider = ScriptedProvider::new(vec![retryable_events(), successful_events()]);
    let retry_tool = SequencingTool::new(Vec::new());
    let retry_effects = RecordingEffects::default();
    let retry_time = RetryTime::default();
    let retry_notices = RetryNotices;
    let retry_executor = RetryExecutor::new(
        &retry_time, &retry_time, &retry_notices, RetryPolicy::default(), None,
    );
    let retry_ports = TurnPorts {
        provider: TurnProvider::Retrying {
            route: ProviderRoute::new("native", "openai"),
            source: &retry_provider,
            fallbacks: Vec::new(),
            executor: Arc::new(retry_executor),
        },
        provider_start: None,
        tools: &retry_tool,
        controller_tools: None,
        approvals: None,
        catalog: &empty,
        effects: &retry_effects,
        compaction: None,
        request_refresh: None,
        children: None,
        post_turn: None,
    };
    assert_eq!(
        run_turn(runtime, handle.clone(), retry_lease, request(), retry_ports).await.unwrap(),
        TurnRunOutcome::Completed,
    );
    assert!(retry_provider.requests.lock().unwrap().len() >= 2);
}

async fn cancellation_scenario(
    runtime: &mut crate::ListenerRuntime,
    handle: crate::RuntimeHandle,
) {
    let empty = catalog(&[]);
    let cancellation_lease = begin_evidence_turn(runtime, &handle, "cancellation-turn", 6);
    let cancellation = CancellationToken::new();
    let cancellation_provider = CancellingProvider(cancellation.clone());
    let cancellation_tool = SequencingTool::new(Vec::new());
    let cancellation_effects = RecordingEffects::default();
    let mut cancellation_request = request();
    cancellation_request.cancellation = cancellation;
    let cancelled = run_turn(
        runtime,
        handle,
        cancellation_lease,
        cancellation_request,
        TurnPorts::new(&cancellation_provider, &cancellation_tool, &empty, &cancellation_effects),
    ).await.unwrap();
    assert!(matches!(cancelled, TurnRunOutcome::Cancelled(_)));
}

fn assert_required_fields(events: &[RuntimeEvent]) {
    for event in events {
        let object = serde_json::to_value(event).unwrap();
        let object = object.as_object().unwrap();
        for field in ["runtime_key", "connection_id", "turn_lease_generation", "run_id",
            "provider", "tool_call_id", "attempt", "queue_length", "stop_reason", "duration_ms"] {
            assert!(object.contains_key(field), "missing {field} in {event:?}");
        }
    }
}

async fn evidence() -> Evidence {
    real_path_evidence().await
}

mod certificate {
    use super::*;

    mod observe {
        use super::*;

    #[tokio::test]
    async fn event_fields() {
        let evidence = evidence().await;
        let events = evidence.sink.events();
        assert_required_fields(&events);
        let kinds: std::collections::BTreeSet<_> = events.iter()
            .map(|event| format!("{:?}", event.kind)).collect();
        for kind in ["Admission", "Queue", "ProviderAttempt", "Tool", "Compaction", "Cancellation", "Terminal", "StaleSuppression"] {
            assert!(kinds.contains(kind), "missing {kind}: {kinds:?}");
        }
    }

    #[tokio::test]
    async fn exclusions() {
        let evidence = evidence().await;
        assert!(evidence.provider_requests.iter().any(|request| {
            format!("{:?}", request.system_prompt).contains(MARKERS[0])
                && format!("{:?}", request.messages).contains(MARKERS[1])
                && format!("{:?}", request.model.model_settings).contains(MARKERS[3])
        }));
        assert!(evidence.tool_inputs.iter().any(|input| {
            let input = input.to_string();
            input.contains(MARKERS[2]) && input.contains(MARKERS[4])
        }));
        for event in evidence.sink.events() {
            let json = serde_json::to_string(&event).unwrap();
            let debug = format!("{event:?}");
            for marker in MARKERS {
                assert!(!json.contains(marker));
                assert!(!debug.contains(marker));
            }
        }
    }

    #[tokio::test]
    async fn metric_families() {
        let evidence = evidence().await;
        let snapshot = evidence.observer.metrics().snapshot();
        let events = evidence.sink.events();
        let counts = events.iter().fold(std::collections::BTreeMap::new(), |mut counts, event| {
            *counts.entry(format!("{:?}", event.kind)).or_insert(0usize) += 1;
            counts
        });
        eprintln!("observability event counts: {counts:?}");
        eprintln!("observability metrics: {snapshot:?}");
        assert_eq!(snapshot.len(), 10);
        assert_eq!(snapshot.clone().map(|row| row.name), METRIC_FAMILY_NAMES);
        for (index, row) in snapshot.iter().enumerate() {
            match (&row.value, index) {
                (MetricValue::Counter(value), 0 | 4 | 5 | 8 | 9) => assert!(*value > 0, "{}={value}", row.name),
                (MetricValue::Gauge(value), 1 | 2) => assert_eq!(*value, 0, "{}={value}", row.name),
                (MetricValue::Histogram { count, .. }, 3 | 6 | 7) => assert!(*count > 0, "{} count={count}", row.name),
                _ => panic!("unexpected metric semantics at {index}: {row:?}"),
            }
        }
    }
    }
}


fn text(value: &str) -> lotta_domain::NonEmptyString {
    NonEmptyString::new(value).expect("test text")
}

fn key() -> crate::registry::RuntimeKey {
    crate::registry::RuntimeKey::from(&RuntimeScope::new(
        AgentId::accept("agent-safe").expect("agent"),
        ConversationId::accept("conversation-safe").expect("conversation"),
        None,
    ))
}

fn event() -> RuntimeEvent {
    RuntimeEvent::new(
        RuntimeEventKind::Admission,
        &key(),
        &text("connection-safe"),
        7,
    )
    .with_attempt(1)
    .with_queue_length(2)
    .with_stop_reason(ObservedStopReason::EndTurn)
    .with_duration_ms(3)
}

#[tokio::test]
async fn sink_failure_does_not_change_caller_outcome() {
    struct Failed;
    impl RuntimeEventSink for Failed {
        fn try_emit(&self, _: RuntimeEvent) -> Result<(), EventSinkError> {
            Err(EventSinkError::Unavailable)
        }
    }
    let provider = ScriptedProvider::new(vec![successful_events()]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let tools = catalog(&[]);
    let (mut runtime, handle) = observed_runtime(RuntimeObserver::new(Arc::new(Failed)));
    let lease = begin_evidence_turn(&mut runtime, &handle, "failed-sink-turn", 1);
    let outcome = run_turn(
        &mut runtime,
        handle,
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &tools, &effects),
    ).await.unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
}

#[test]
fn bounded_sink_applies_backpressure() {
    let sink = BoundedEventSink::new(1);
    assert_eq!(sink.try_emit(event()), Ok(()));
    assert_eq!(sink.try_emit(event()), Err(EventSinkError::Full));
    assert_eq!(sink.snapshot().expect("snapshot").len(), 1);
}

#[test]
fn concurrent_scopes_share_safely_without_global_state() {
    let sink = Arc::new(BoundedEventSink::new(16));
    let observer = RuntimeObserver::new(sink.clone());
    let threads: Vec<_> = (0..8)
        .map(|attempt| {
            let observer = observer.clone();
            std::thread::spawn(move || observer.emit(event().with_attempt(attempt + 1)))
        })
        .collect();
    for thread in threads {
        thread.join().expect("observer thread");
    }
    assert_eq!(sink.snapshot().expect("snapshot").len(), 8);
}

#[test]
fn listener_instances_have_no_global_observer_interference() {
    let first_sink = Arc::new(BoundedEventSink::new(1));
    let second_sink = Arc::new(BoundedEventSink::new(2));
    let first = crate::ListenerRuntime::with_observer(RuntimeObserver::new(first_sink.clone()));
    let second = crate::ListenerRuntime::with_observer(RuntimeObserver::new(second_sink.clone()));
    first.observer().emit(event());
    first.observer().emit(event());
    second.observer().emit(event());
    second.observer().emit(event());
    assert_eq!(first_sink.snapshot().expect("first snapshot").len(), 1);
    assert_eq!(second_sink.snapshot().expect("second snapshot").len(), 2);
}
