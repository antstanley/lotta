use super::test_support::{Fixture, assert_complete, run_with};
use crate::ToolsetId;
use lotta_runtime::{
    RuntimeError,
    boundary::ProcessOutputChunk,
    ports::{
        InteractiveSandboxPort, PortFuture, ProcessEvent, ProcessOutcome, ProcessRequest,
        ProcessSessionFuture, SandboxPort, ToolOutcome,
    },
};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct BlockingSandbox {
    durations: Mutex<Vec<Duration>>,
}

impl InteractiveSandboxPort for BlockingSandbox {
    fn start_session(&self, _: ProcessRequest, _: CancellationToken) -> ProcessSessionFuture<'_> {
        Box::pin(async {
            Err(RuntimeError::Unsupported {
                context: "timeout test".into(),
            })
        })
    }
}

impl SandboxPort for BlockingSandbox {
    fn execute(
        &self,
        request: ProcessRequest,
        events: Sender<ProcessEvent>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        self.durations.lock().unwrap().push(request.timeout);
        Box::pin(async move {
            let _events = events;
            tokio::select! {
                () = tokio::time::sleep(request.timeout) => Ok(ProcessOutcome {
                    exit_code: None,
                    timed_out: true,
                }),
                () = cancellation.cancelled() => Err(RuntimeError::Cancelled {
                    context: "shell process".into(),
                }),
            }
        })
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_outcome() {
    let fixture = Fixture::new("timeout");
    let sandbox = Arc::new(BlockingSandbox::default());
    let run = run_with(
        &fixture,
        sandbox,
        ToolsetId::Default,
        "Bash",
        json!({"command":"blocked","description":"timeout"}),
        CancellationToken::new(),
    );
    tokio::pin!(run);
    assert!(futures_pending(&mut run).await);
    tokio::time::advance(Duration::from_mins(3)).await;
    let (result, records, bundle) = run.await;
    let outcome = result.unwrap();
    assert!(matches!(outcome, ToolOutcome::Timeout { .. }));
    assert_complete(&records, &outcome);
    bundle.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancellation_outcome_differs() {
    let fixture = Fixture::new("cancel");
    let sandbox = Arc::new(BlockingSandbox::default());
    let cancellation = CancellationToken::new();
    let run = run_with(
        &fixture,
        sandbox,
        ToolsetId::Default,
        "Bash",
        json!({"command":"blocked","description":"cancel"}),
        cancellation.clone(),
    );
    tokio::pin!(run);
    assert!(futures_pending(&mut run).await);
    cancellation.cancel();
    let (result, records, bundle) = run.await;
    let outcome = result.unwrap();
    assert!(matches!(outcome, ToolOutcome::Interrupted { .. }));
    assert!(!matches!(outcome, ToolOutcome::Timeout { .. }));
    assert_complete(&records, &outcome);
    bundle.shutdown().await.unwrap();
}

async fn futures_pending<F>(future: &mut std::pin::Pin<&mut F>) -> bool
where
    F: std::future::Future,
{
    tokio::select! {
        _ = future => false,
        () = tokio::task::yield_now() => true,
    }
}

#[tokio::test(start_paused = true)]
async fn override_bounded() {
    assert_bash_timeout_matrix().await;
    assert_monitor_timeout_matrix().await;
    assert_schema_override_matrices().await;
}

async fn assert_bash_timeout_matrix() {
    for (value, expected, valid) in [
        (1_u64, 1_u64, true),
        (600_000, 600_000, true),
        (600_001, 0, false),
    ] {
        let fixture = Fixture::new("bash-matrix");
        let sandbox = Arc::new(BlockingSandbox::default());
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let (result, _, bundle) = run_with(
            &fixture,
            Arc::clone(&sandbox) as Arc<dyn super::ShellSandbox>,
            ToolsetId::Default,
            "Bash",
            json!({"command":"blocked","description":"matrix","timeout":value}),
            cancellation,
        )
        .await;
        if valid {
            assert!(matches!(result.unwrap(), ToolOutcome::Interrupted { .. }));
            tokio::task::yield_now().await;
            assert_eq!(
                sandbox.durations.lock().unwrap().as_slice(),
                [Duration::from_millis(expected)]
            );
        } else {
            assert!(result.is_err());
            assert!(sandbox.durations.lock().unwrap().is_empty());
        }
        bundle.shutdown().await.unwrap();
    }
}

async fn assert_monitor_timeout_matrix() {
    for (value, expected, valid) in [
        (1_000_u64, 1_000_u64, true),
        (3_600_000, 3_600_000, true),
        (3_600_001, 0, false),
    ] {
        let fixture = Fixture::new("monitor-matrix");
        let sandbox = Arc::new(ImmediateSandbox::default());
        let (result, _, bundle) = super::test_support::run(
            &fixture,
            Arc::clone(&sandbox) as Arc<dyn super::ShellSandbox>,
            ToolsetId::Default,
            "Monitor",
            json!({"command":"event","description":"matrix","timeout_ms":value}),
        )
        .await;
        if valid {
            assert!(matches!(result.unwrap(), ToolOutcome::Success { .. }));
            tokio::task::yield_now().await;
            assert_eq!(
                sandbox.durations.lock().unwrap().as_slice(),
                [Duration::from_millis(expected)]
            );
        } else {
            assert!(result.is_err());
            assert!(sandbox.durations.lock().unwrap().is_empty());
        }
        bundle.shutdown().await.unwrap();
    }
    let fixture = Fixture::new("monitor-persistent");
    let sandbox = Arc::new(ImmediateSandbox::default());
    let (result, _, bundle) = super::test_support::run(
        &fixture,
        Arc::clone(&sandbox) as Arc<dyn super::ShellSandbox>,
        ToolsetId::Default,
        "Monitor",
        json!({"command":"event","description":"persistent","persistent":true}),
    )
    .await;
    assert!(matches!(result.unwrap(), ToolOutcome::Success { .. }));
    tokio::task::yield_now().await;
    assert_eq!(
        sandbox.durations.lock().unwrap().as_slice(),
        [lotta_runtime::ports::PROCESS_TIMEOUT_DISABLED]
    );
    bundle.shutdown().await.unwrap();
}

async fn assert_schema_override_matrices() {
    let fixture = Fixture::new("schema-matrices");
    let sandbox = Arc::new(ImmediateSandbox::default());
    let (exec, _, bundle) = super::test_support::run(
        &fixture,
        Arc::clone(&sandbox) as Arc<dyn super::ShellSandbox>,
        ToolsetId::Codex,
        "exec_command",
        json!({"cmd":"ok","description":"exec","yield_time_ms":30_001}),
    )
    .await;
    assert!(matches!(exec.unwrap(), ToolOutcome::Success { .. }));
    bundle.shutdown().await.unwrap();
    for (name, input) in [(
        "TaskOutput",
        json!({"task_id":"x","block":true,"timeout":600_001}),
    )] {
        let toolset = if name == "TaskOutput" {
            ToolsetId::Default
        } else {
            ToolsetId::Codex
        };
        let (result, records, bundle) = super::test_support::run(
            &fixture,
            Arc::clone(&sandbox) as Arc<dyn super::ShellSandbox>,
            toolset,
            name,
            input,
        )
        .await;
        assert!(result.is_err(), "{name}: {result:?}");
        assert_eq!(records.trace.lock().unwrap().len(), 2);
        bundle.shutdown().await.unwrap();
    }
}

#[derive(Default)]
struct ImmediateSandbox {
    durations: Mutex<Vec<Duration>>,
}
impl InteractiveSandboxPort for ImmediateSandbox {
    fn start_session(&self, _: ProcessRequest, _: CancellationToken) -> ProcessSessionFuture<'_> {
        Box::pin(async {
            Err(RuntimeError::Unsupported {
                context: "test".into(),
            })
        })
    }
}
impl SandboxPort for ImmediateSandbox {
    fn execute(
        &self,
        request: ProcessRequest,
        events: Sender<ProcessEvent>,
        _: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        self.durations.lock().unwrap().push(request.timeout);
        Box::pin(async move {
            let chunk = ProcessOutputChunk::new(b"event".to_vec())?;
            let _ = events.send(ProcessEvent::Stdout(chunk)).await;
            Ok(ProcessOutcome {
                exit_code: Some(0),
                timed_out: false,
            })
        })
    }
}
