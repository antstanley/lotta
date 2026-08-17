use super::snapshot::{StatusError, StatusPort, StreamEvent, TaskState};
use super::spawn::*;
use super::types::*;
use crate::sidecar::SidecarOwnerIdentity;
use lotta_domain::{AgentId, ConversationId};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Status(Mutex<Vec<Vec<u8>>>);
impl StatusPort for Status {
    type Update<'a> = Pin<Box<dyn Future<Output = Result<(), StatusError>> + Send + 'a>>;
    fn update_subagent_state(&self, payload: Vec<u8>) -> Self::Update<'_> {
        Box::pin(async move {
            self.0.lock().unwrap().push(payload);
            Ok(())
        })
    }
}

struct Launcher {
    active: AtomicUsize,
    peak: AtomicUsize,
    order: Mutex<Vec<u64>>,
    gates: Mutex<BTreeMap<u64, CancellationToken>>,
    gate: CancellationToken,
    silent: bool,
}
impl Launcher {
    fn new(silent: bool) -> Self {
        Self {
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            order: Mutex::new(Vec::new()),
            gates: Mutex::new(BTreeMap::new()),
            gate: CancellationToken::new(),
            silent,
        }
    }
}
impl SubagentLauncher for Launcher {
    type Run<'a> = Pin<Box<dyn Future<Output = Result<SubagentResult, SpawnError>> + Send + 'a>>;
    fn run(
        &self,
        task_id: u64,
        _: SubagentRequest,
        _: SidecarOwnerIdentity,
        cancellation: CancellationToken,
        events: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Self::Run<'_> {
        Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            self.order.lock().unwrap().push(task_id);
            let identity_gate = self.gates.lock().unwrap().get(&task_id).cloned();
            if !self.silent {
                events
                    .send(StreamEvent {
                        task_id,
                        sequence: 1,
                        bytes: b"event".to_vec(),
                    })
                    .await
                    .unwrap();
            }
            if let Some(identity_gate) = identity_gate {
                tokio::select! {
                    () = identity_gate.cancelled() => {},
                    () = self.gate.cancelled() => {},
                    () = cancellation.cancelled() => {}
                }
            } else {
                tokio::select! { () = self.gate.cancelled() => {}, () = cancellation.cancelled() => {} }
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            if cancellation.is_cancelled() {
                Err(SpawnError::Cancelled)
            } else {
                Ok(SubagentResult {
                    report: String::new(),
                    success: true,
                    diagnostic: String::new(),
                })
            }
        })
    }
}

fn request() -> SubagentRequest {
    SubagentRequest {
        subagent_type: SubagentType::GeneralPurpose,
        description: "description".into(),
        prompt: "prompt".into(),
        resolved_context: None,
        model: ModelPolicy::Inherit("parent-model".into()),
        background: true,
        silent: false,
        filesystem_roots: Vec::new(),
        reflection_worktree: None,
        max_turns: None,
        tools: ToolPolicy::All,
        memory_scope: None,
        parent_scope: ParentScope {
            agent_id: AgentId::accept("parent").unwrap(),
            conversation_id: ConversationId::accept("conversation").unwrap(),
            runtime_id: "runtime".into(),
        },
        existing_agent_id: None,
        existing_conversation_id: None,
    }
}

fn manager(launcher: Arc<Launcher>, status: Arc<Status>) -> SubagentManager<Launcher, Status> {
    SubagentManager::new(
        launcher,
        status,
        SidecarOwnerIdentity::new("parent", "runtime", "conversation").unwrap(),
        "parent".into(),
    )
}

async fn spawn_count(
    manager: &SubagentManager<Launcher, Status>,
    count: usize,
) -> Vec<Result<SpawnOutcome, SpawnError>> {
    let mut output = Vec::new();
    for _ in 0..count {
        output.push(manager.spawn(request()).await);
    }
    output
}

#[tokio::test]
async fn total_below_127_is_admitted() {
    let launcher = Arc::new(Launcher::new(true));
    let manager = manager(launcher.clone(), Arc::new(Status::default()));
    assert!(
        spawn_count(&manager, 127)
            .await
            .into_iter()
            .all(|result| result.is_ok())
    );
    launcher.gate.cancel();
}

#[tokio::test]
async fn total_at_128_is_admitted() {
    let launcher = Arc::new(Launcher::new(true));
    let manager = manager(launcher.clone(), Arc::new(Status::default()));
    assert!(
        spawn_count(&manager, 128)
            .await
            .into_iter()
            .all(|result| result.is_ok())
    );
    launcher.gate.cancel();
}

#[tokio::test]
async fn total_above_129_is_rejected() {
    let launcher = Arc::new(Launcher::new(true));
    let manager = manager(launcher.clone(), Arc::new(Status::default()));
    let results = spawn_count(&manager, 129).await;
    assert_eq!(results.last(), Some(&Err(SpawnError::TotalLimit)));
    launcher.gate.cancel();
}

#[tokio::test]
async fn concurrent_below_15_runs() {
    concurrency_case(15, 15).await;
}

#[tokio::test]
async fn concurrent_at_16_runs() {
    concurrency_case(16, 16).await;
}

#[tokio::test]
async fn concurrent_above_17_waits_fifo() {
    let launcher = Arc::new(Launcher::new(true));
    for id in 1..=16 {
        launcher
            .gates
            .lock()
            .unwrap()
            .insert(id, CancellationToken::new());
    }
    let manager = manager(launcher.clone(), Arc::new(Status::default()));
    assert!(
        spawn_count(&manager, 17)
            .await
            .into_iter()
            .all(|result| result.is_ok())
    );
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert_eq!(
        *launcher.order.lock().unwrap(),
        (1..=16).collect::<Vec<_>>()
    );
    launcher.gates.lock().unwrap()[&1].cancel();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert_eq!(
        *launcher.order.lock().unwrap(),
        (1..=17).collect::<Vec<_>>()
    );
    launcher.gate.cancel();
}

async fn concurrency_case(count: usize, expected_peak: usize) {
    let launcher = Arc::new(Launcher::new(true));
    let manager = manager(launcher.clone(), Arc::new(Status::default()));
    assert!(
        spawn_count(&manager, count)
            .await
            .into_iter()
            .all(|result| result.is_ok())
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(launcher.peak.load(Ordering::SeqCst), expected_peak);
    launcher.gate.cancel();
}

#[tokio::test]
async fn raced_admission_never_exceeds_total_or_concurrent_bounds() {
    let launcher = Arc::new(Launcher::new(true));
    let manager = Arc::new(manager(launcher.clone(), Arc::new(Status::default())));
    let mut joins = Vec::new();
    for _ in 0..160 {
        let manager = manager.clone();
        joins.push(tokio::spawn(async move { manager.spawn(request()).await }));
    }
    let mut admitted = 0;
    for join in joins {
        if join.await.unwrap().is_ok() {
            admitted += 1;
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(admitted, SUBAGENTS_PER_PARENT_MAX);
    assert!(launcher.peak.load(Ordering::SeqCst) <= SUBAGENTS_CONCURRENT_PER_PARENT_MAX);
    launcher.gate.cancel();
}

#[tokio::test]
async fn stop_awaits_terminal_cleanup_and_result_is_retained() {
    let launcher = Arc::new(Launcher::new(true));
    let manager = manager(launcher, Arc::new(Status::default()));
    let SpawnOutcome::Background(handle) = manager.spawn(request()).await.unwrap() else {
        panic!("background handle expected");
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    manager.stop(handle.task_id).await.unwrap();
    assert_eq!(
        manager.task_state(handle.task_id).unwrap(),
        TaskState::Cancelled
    );
    assert_eq!(
        manager.task_output(handle.task_id).unwrap(),
        Some(Err(SpawnError::Cancelled))
    );
}

#[tokio::test]
async fn silent_subagent_does_not_broadcast_actual_actor_stream() {
    let launcher = Arc::new(Launcher::new(true));
    let status = Arc::new(Status::default());
    let manager = manager(launcher.clone(), status.clone());
    let mut silent = request();
    silent.silent = true;
    manager.spawn(silent).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    launcher.gate.cancel();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let snapshots = status.0.lock().unwrap();
    assert!(snapshots.iter().all(|bytes| {
        serde_json::from_slice::<serde_json::Value>(bytes).unwrap()["events"]
            .as_array()
            .unwrap()
            .is_empty()
    }));
}
