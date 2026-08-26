use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::AtomicU64};

use lotta_domain::{
    AgentId, BoundedVec, Clock, ConversationId, DomainError, RuntimeScope, Timestamp,
};
use serde_json::Value;
use tokio::sync::mpsc;

use super::{ListenerState, SocketLimits, dispatch_output, event_sink};
use crate::auth::AuthPolicy;
use crate::observer::{RuntimeBroadcastObservation, RuntimeBroadcastObserver};
use crate::ws::{
    RandomEventIdGenerator, RouteOutput, RoutedEventBatch, RuntimeEvent, RuntimeRouter,
    UnsupportedRuntimeCommandService,
    event::{LoopState, LoopStatus},
};

const OBSERVATIONS_MAX: usize = 4;
type Observations = BoundedVec<RuntimeBroadcastObservation, OBSERVATIONS_MAX>;

struct RecordingObserver {
    observations: Mutex<Observations>,
}

impl RecordingObserver {
    fn new() -> Self {
        Self {
            observations: Mutex::new(BoundedVec::new(Vec::new()).unwrap()),
        }
    }

    fn values(&self) -> Observations {
        self.observations.lock().unwrap().clone()
    }
}

impl RuntimeBroadcastObserver for RecordingObserver {
    fn observe(&self, observation: RuntimeBroadcastObservation) {
        let mut observations = self.observations.lock().unwrap();
        let mut values = observations.as_slice().to_vec();
        values.push(observation);
        *observations = BoundedVec::new(values).unwrap();
    }
}

struct TestClock;
impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56Z").unwrap()
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

struct PanickingObserver;
impl RuntimeBroadcastObserver for PanickingObserver {
    fn observe(&self, _: RuntimeBroadcastObservation) {
        panic!("observer panic");
    }
}

fn scope(index: usize) -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept(format!("agent-{index}")).unwrap(),
        ConversationId::accept(format!("conversation-{index}")).unwrap(),
        None,
    )
}

fn event(name: &str) -> RuntimeEvent {
    let status = match name {
        "WAITING_ON_INPUT" => LoopStatus::WaitingOnInput,
        "WAITING_ON_APPROVAL" => LoopStatus::WaitingOnApproval,
        _ => LoopStatus::ProcessingApiResponse,
    };
    RuntimeEvent::UpdateLoopStatus {
        loop_status: LoopState {
            status,
            active_run_ids: Vec::new(),
            executing_tool_call_ids: Vec::new(),
        },
    }
}

fn state(
    observer: Arc<dyn RuntimeBroadcastObserver>,
    subscribed: bool,
) -> (Arc<ListenerState>, u64, mpsc::Receiver<Vec<String>>) {
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(TestClock);
    let mut router = RuntimeRouter::new(clock.clone(), Arc::new(RandomEventIdGenerator));
    let id = router.connections.open().unwrap();
    router.connections.initialize(id).unwrap();
    if subscribed {
        router.connections.subscribe(id, scope(1)).unwrap();
    }
    let (sender, receiver) = mpsc::channel(4);
    let mut outbound = HashMap::new();
    outbound.insert(id, sender);
    let group = compose_group_bridges();
    let state = Arc::new(ListenerState {
        auth: AuthPolicy::None,
        listener_instance: "test-listener".to_owned(),
        clock,
        shutdown: tokio_util::sync::CancellationToken::new(),
        limits: SocketLimits::default(),
        runtime_router: Arc::new(Mutex::new(router)),
        runtime_service: Arc::new(UnsupportedRuntimeCommandService),
        turn_controller: Arc::new(UnsupportedRuntimeCommandService),
        observer,
        external_tools: Arc::new(crate::ws::external_tools::ExternalToolBridge::new(
            crate::ws::external_tools::inert_forwarder(),
        )),
        teleports: Arc::new(crate::ws::teleport::TeleportBridge::new(
            crate::ws::teleport::inert_forwarder(),
        )),
        terminals: Arc::new(crate::ws::terminal::TerminalBridge::new(
            inert_terminal_forwarder(),
            Arc::new(TestClock),
        )),
        files: group.files,
        memories: group.memories,
        agents: group.agents,
        conversations: group.conversations,
        models: group.models,
        schedules: group.schedules,
        skills: Arc::new(crate::ws::skills::SkillsBridge::new(
            crate::ws::skills::inert_forwarder(),
            &files_workspace("observer-skills"),
            Arc::new(TestClock),
        )),
        settings: Arc::new(
            crate::ws::settings::SettingsBridge::new(
                crate::ws::settings::inert_forwarder(),
                &files_workspace("observer-settings"),
                &files_workspace("observer"),
            )
            .expect("settings bridge"),
        ),
        devices: Arc::new(
            crate::ws::device::DeviceBridge::new(
                crate::ws::device::inert_forwarder(),
                &files_workspace("observer-device-workspace"),
                &files_workspace("observer-device-storage"),
            )
            .expect("device bridge"),
        ),
        introspection: Arc::new(crate::ws::introspection::IntrospectionBridge::new(
            crate::ws::introspection::inert_forwarder(),
        )),
        next_observation: AtomicU64::new(1),
        outbound: Arc::new(Mutex::new(outbound)),
    });
    (state, id, receiver)
}

fn files_workspace(name: &str) -> std::path::PathBuf {
    static ORDINAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let ordinal = ORDINAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "lotta-listener-{name}-{}-{ordinal}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("workspace directory");
    path.canonicalize().expect("canonical workspace")
}

fn inert_terminal_forwarder() -> crate::ws::terminal::TerminalForwarder {
    Arc::new(|_, _| Ok(()))
}

fn output(state: &ListenerState, event: RuntimeEvent) -> RouteOutput {
    let scope = scope(1);
    let deliveries = state
        .runtime_router
        .lock()
        .unwrap()
        .broadcast(&scope, &event)
        .unwrap();
    RouteOutput {
        responses: BoundedVec::new(Vec::new()).unwrap(),
        event_batches: BoundedVec::new(vec![RoutedEventBatch {
            scope,
            event,
            deliveries,
        }])
        .unwrap(),
        response_after_events: false,
    }
}

fn wire(value: &str) -> Value {
    serde_json::from_str(value).unwrap()
}

#[tokio::test]
async fn initial_route_output_queues_exact_frame_and_observes_actual_batch() {
    let observer = Arc::new(RecordingObserver::new());
    let (state, id, mut receiver) = state(observer.clone(), true);
    let logical = event("INITIAL");
    dispatch_output(&state, id, &output(&state, logical.clone())).unwrap();
    let queued = wire(&receiver.recv().await.unwrap()[0]);
    let values = observer.values();
    let record = &values.as_slice()[0];
    assert_eq!(record.batch_ordinal, 1);
    assert_eq!(record.scope, scope(1));
    assert_eq!(
        serde_json::to_value(&record.event).unwrap(),
        serde_json::to_value(logical).unwrap()
    );
    assert_eq!(record.deliveries.len(), 1);
    let delivery = &record.deliveries.as_slice()[0];
    assert_eq!(delivery.connection_id, id);
    assert_eq!(delivery.ordinal, 1);
    assert_eq!(serde_json::to_value(&delivery.frame).unwrap(), queued);
}

#[tokio::test]
async fn continuation_sink_records_next_batch_and_queues_exact_frame() {
    let observer = Arc::new(RecordingObserver::new());
    let (state, id, mut receiver) = state(observer.clone(), true);
    dispatch_output(&state, id, &output(&state, event("INITIAL"))).unwrap();
    let _ = receiver.recv().await.unwrap();
    event_sink(&state)
        .emit(&scope(1), event("CONTINUATION"))
        .unwrap();
    let queued = wire(&receiver.recv().await.unwrap()[0]);
    let values = observer.values();
    assert_eq!(values.len(), 2);
    let record = &values.as_slice()[1];
    assert_eq!(record.batch_ordinal, 2);
    assert_eq!(record.deliveries.as_slice()[0].ordinal, 1);
    assert_eq!(
        serde_json::to_value(&record.deliveries.as_slice()[0].frame).unwrap(),
        queued
    );
}

#[test]
fn zero_subscriber_dispatch_observes_actual_empty_batch() {
    let observer = Arc::new(RecordingObserver::new());
    let (state, id, _) = state(observer.clone(), false);
    let logical = event("EMPTY");
    dispatch_output(&state, id, &output(&state, logical.clone())).unwrap();
    let values = observer.values();
    let record = &values.as_slice()[0];
    assert_eq!(record.scope, scope(1));
    assert_eq!(
        serde_json::to_value(&record.event).unwrap(),
        serde_json::to_value(logical).unwrap()
    );
    assert!(record.deliveries.is_empty());
}

#[tokio::test]
async fn panicking_observer_cannot_change_dispatch_or_delivery() {
    let (state, id, mut receiver) = state(Arc::new(PanickingObserver), true);
    assert!(dispatch_output(&state, id, &output(&state, event("PANIC"))).is_ok());
    let queued = wire(&receiver.recv().await.unwrap()[0]);
    assert_eq!(queued["type"], "update_loop_status");
}

#[test]
fn exhausted_observation_ordinal_skips_observer_after_dispatch() {
    let observer = Arc::new(RecordingObserver::new());
    let (state, id, _) = state(observer.clone(), false);
    state
        .next_observation
        .store(u64::MAX, std::sync::atomic::Ordering::SeqCst);
    assert!(dispatch_output(&state, id, &output(&state, event("SATURATED"))).is_ok());
    assert!(observer.values().is_empty());
}

/// One storage-backed bridge bundle for observer fixtures.
struct GroupBridges {
    files: Arc<crate::ws::files::FilesBridge>,
    memories: Arc<crate::ws::memory::MemoryBridge>,
    agents: Arc<crate::ws::agents::AgentsBridge>,
    conversations: Arc<crate::ws::conversations::ConversationsBridge>,
    models: Arc<crate::ws::models::ModelsBridge>,
    schedules: Arc<crate::ws::schedules::SchedulesBridge>,
}

/// Composes the storage-backed bridges over unique temporary roots.
fn compose_group_bridges() -> GroupBridges {
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(TestClock);
    GroupBridges {
        files: Arc::new(
            crate::ws::files::FilesBridge::with_poll_interval(
                crate::ws::files::inert_forwarder(),
                &files_workspace("observer"),
                &files_workspace("observer-artifacts"),
                60_000,
            )
            .expect("files bridge"),
        ),
        memories: Arc::new(
            crate::ws::memory::MemoryBridge::new(
                crate::ws::memory::inert_forwarder(),
                &files_workspace("observer-memfs"),
                Arc::clone(&clock),
            )
            .expect("memory bridge"),
        ),
        agents: Arc::new(
            crate::ws::agents::AgentsBridge::new(
                crate::ws::agents::inert_forwarder(),
                &files_workspace("observer-agents"),
                Arc::clone(&clock),
            )
            .expect("agents bridge"),
        ),
        conversations: Arc::new(
            crate::ws::conversations::ConversationsBridge::new(
                crate::ws::conversations::inert_forwarder(),
                &files_workspace("observer-conversations"),
                Arc::clone(&clock),
                None,
            )
            .expect("conversations bridge"),
        ),
        models: Arc::new(
            crate::ws::models::ModelsBridge::new(
                crate::ws::models::inert_forwarder(),
                &files_workspace("observer-models"),
                Arc::clone(&clock),
            )
            .expect("models bridge"),
        ),
        schedules: Arc::new(
            crate::ws::schedules::SchedulesBridge::new(
                crate::ws::schedules::inert_forwarder(),
                &files_workspace("observer-schedules"),
                Arc::clone(&clock),
            )
            .expect("schedules bridge"),
        ),
    }
}
