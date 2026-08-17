use super::manager::{
    CHILD_PROCESS_OUTPUT_BYTES_MAX, ManagerError, PROCESS_CHANNEL_ITEMS_MAX, ProcessManager,
    SHELL_COMMAND_BYTES_MAX, SHELL_SESSION_ID_BYTES_MAX, SHELL_SESSION_OUTPUT_BYTES_MAX,
    SHELL_SESSIONS_MAX, SHELL_STDIN_TOTAL_BYTES_MAX, SHELL_STDIN_WRITE_BYTES_MAX, SessionStatus,
    validate_command, validate_id,
};
use super::{ShellSandbox, ShellToolBundle, test_support};
use crate::{ToolsetId, clamp::FileOverflowWriter};
use lotta_runtime::bounds::TOOL_RESULT_BYTES_MAX;
use lotta_runtime::{
    RuntimeError,
    boundary::{ProcessOutputChunk, ProcessStdin},
    bounds::PROCESS_OUTPUT_CHUNK_BYTES_MAX,
    ports::{
        InteractiveSandboxPort, PortFuture, ProcessEvent, ProcessInput, ProcessOutcome,
        ProcessRequest, ProcessSession, ProcessSessionFuture, SandboxPort, ToolOutcome,
    },
};
use std::{
    fs,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct ScriptedSandbox {
    script: Arc<Vec<(bool, Vec<u8>)>>,
    active: Arc<AtomicUsize>,
    spawned: Arc<AtomicUsize>,
    interactive: Option<Arc<InteractiveState>>,
}

struct InteractiveState {
    received: Arc<Mutex<Vec<u8>>>,
    received_notify: Arc<Notify>,
    sessions: Mutex<Vec<CancellationToken>>,
}

impl ScriptedSandbox {
    fn bytes(bytes: usize) -> (Self, Arc<AtomicUsize>) {
        let active = Arc::new(AtomicUsize::new(0));
        let mut script = Vec::new();
        let mut remaining = bytes;
        while remaining != 0 {
            let count = remaining.min(PROCESS_OUTPUT_CHUNK_BYTES_MAX.value);
            script.push((false, vec![b'x'; count]));
            remaining -= count;
        }
        (Self::script(script, Arc::clone(&active)), active)
    }

    fn script(script: Vec<(bool, Vec<u8>)>, active: Arc<AtomicUsize>) -> Self {
        Self {
            script: Arc::new(script),
            active,
            spawned: Arc::new(AtomicUsize::new(0)),
            interactive: None,
        }
    }

    fn interactive() -> (Self, Arc<InteractiveState>) {
        let state = Arc::new(InteractiveState {
            received: Arc::new(Mutex::new(Vec::new())),
            received_notify: Arc::new(Notify::new()),
            sessions: Mutex::new(Vec::new()),
        });
        (
            Self {
                script: Arc::new(Vec::new()),
                active: Arc::new(AtomicUsize::new(0)),
                spawned: Arc::new(AtomicUsize::new(0)),
                interactive: Some(Arc::clone(&state)),
            },
            state,
        )
    }
}

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl SandboxPort for ScriptedSandbox {
    fn execute(
        &self,
        _: ProcessRequest,
        events: mpsc::Sender<ProcessEvent>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        self.spawned.fetch_add(1, Ordering::SeqCst);
        let script = Arc::clone(&self.script);
        let active = Arc::clone(&self.active);
        Box::pin(async move {
            active.fetch_add(1, Ordering::SeqCst);
            let _active = Active(active);
            for (stderr, bytes) in script.iter() {
                let event = ProcessOutputChunk::new(bytes.clone()).map(|chunk| {
                    if *stderr {
                        ProcessEvent::Stderr(chunk)
                    } else {
                        ProcessEvent::Stdout(chunk)
                    }
                })?;
                tokio::select! {
                    () = cancellation.cancelled() => {
                        return Err(RuntimeError::Cancelled {
                            context: "scripted".into(),
                        });
                    }
                    result = events.send(event) => {
                        result.map_err(|_| RuntimeError::AdapterFailure {
                            code: "receiver_closed",
                            context: "scripted".into(),
                        })?;
                    }
                }
            }
            drop(events);
            Ok(ProcessOutcome {
                exit_code: Some(0),
                timed_out: false,
            })
        })
    }
}

impl InteractiveSandboxPort for ScriptedSandbox {
    fn start_session(
        &self,
        _: ProcessRequest,
        cancellation: CancellationToken,
    ) -> ProcessSessionFuture<'_> {
        let state = self.interactive.clone();
        let active = Arc::clone(&self.active);
        self.spawned.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let state = state.ok_or(RuntimeError::Unsupported {
                context: "scripted session".into(),
            })?;
            let (input, mut receiver) = mpsc::channel(PROCESS_CHANNEL_ITEMS_MAX);
            let (events, output) = mpsc::channel(PROCESS_CHANNEL_ITEMS_MAX);
            state.sessions.lock().unwrap().push(cancellation.clone());
            let received_bytes = Arc::clone(&state.received);
            let received_notify = Arc::clone(&state.received_notify);
            let token = cancellation.child_token();
            let terminal = tokio::spawn(async move {
                active.fetch_add(1, Ordering::SeqCst);
                let _active = Active(active);
                loop {
                    let value = tokio::select! {
                        biased;
                        value = receiver.recv() => value,
                        () = token.cancelled() => None,
                    };
                    match value {
                        Some(ProcessInput::Bytes(bytes)) => {
                            received_bytes.lock().unwrap().extend(bytes.into_vec());
                            received_notify.notify_waiters();
                        }
                        Some(ProcessInput::Eof) | None => break,
                    }
                }
                drop(events);
                Ok(ProcessOutcome {
                    exit_code: Some(0),
                    timed_out: false,
                })
            });
            Ok(ProcessSession::new(input, output, cancellation, terminal))
        })
    }
}

fn manager(fixture: &test_support::Fixture, sandbox: ScriptedSandbox) -> ProcessManager {
    ProcessManager::new(
        fixture.workspace.clone(),
        test_support::scope(),
        Arc::new(sandbox),
    )
}

async fn one_shot(fixture: &test_support::Fixture, bytes: usize) -> Result<usize, ManagerError> {
    manager(fixture, ScriptedSandbox::bytes(bytes).0)
        .one_shot(
            "printf bounded",
            Path::new(""),
            Duration::from_secs(5),
            CancellationToken::new(),
        )
        .await
        .map(|result| result.output.len())
}

async fn finished(manager: &ProcessManager, id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, status) = manager
                .wait_output(id, Duration::from_millis(100))
                .await
                .unwrap();
            if status != SessionStatus::Running {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("session did not finish: {:?}", manager.inspect(id)));
}

async fn received_bytes(state: &InteractiveState, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notified = state.received_notify.notified();
            if state.received.lock().unwrap().len() >= expected {
                return;
            }
            notified.await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "stdin receiver did not reach {expected} bytes: {}",
            state.received.lock().unwrap().len()
        )
    });
}

#[tokio::test]
async fn pipeline_shell_clamp_retains_full_overflow() {
    let fixture = test_support::Fixture::new("pipeline-clamp");
    let raw = format!("{}{}", "髪".repeat(10_001), "x".repeat(20_000));
    let active = Arc::new(AtomicUsize::new(0));
    let sandbox: Arc<dyn ShellSandbox> = Arc::new(ScriptedSandbox::script(
        vec![(false, raw.as_bytes().to_vec())],
        active,
    ));
    let bundle = ShellToolBundle::new(&fixture.workspace, test_support::scope(), sandbox).unwrap();
    let writer = FileOverflowWriter::new(fixture.overflow.clone()).unwrap();
    let (result, records) = test_support::execute_bundle_with_overflow(
        &bundle,
        ToolsetId::Default,
        "Bash",
        serde_json::json!({
            "command": "printf bounded",
            "description": "Print bounded scripted output"
        }),
        CancellationToken::new(),
        &writer,
    )
    .await;
    let outcome = result.unwrap();
    test_support::assert_complete(&records, &outcome);
    let content = test_support::success_text(&outcome);
    assert_eq!(
        content
            .matches("[Output truncated: showing 30,000 of ")
            .count(),
        1
    );
    assert_eq!(content.matches("[Output truncated: showing").count(), 1);
    assert_eq!(content.matches("[Full output written to:").count(), 1);
    let files = fs::read_dir(&fixture.overflow)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(fs::read(files[0].path()).unwrap(), raw.as_bytes());
    assert_eq!(*records.persisted.lock().unwrap(), vec![outcome.clone()]);
    assert_eq!(*records.emitted.lock().unwrap(), vec![outcome]);
}

#[tokio::test]
async fn pipeline_raw_limit_is_tool_error() {
    for bytes in [
        TOOL_RESULT_BYTES_MAX.value - 1,
        TOOL_RESULT_BYTES_MAX.value,
        TOOL_RESULT_BYTES_MAX.value + 1,
    ] {
        let fixture = test_support::Fixture::new("pipeline-raw");
        let sandbox: Arc<dyn ShellSandbox> = Arc::new(ScriptedSandbox::bytes(bytes).0);
        let (result, _, bundle) = test_support::run(
            &fixture,
            sandbox,
            ToolsetId::Default,
            "Bash",
            serde_json::json!({
                "command": "printf bounded",
                "description": "Print bounded scripted output"
            }),
        )
        .await;
        match (bytes > TOOL_RESULT_BYTES_MAX.value, result) {
            (true, Ok(ToolOutcome::ToolDefinedError { code, .. })) => {
                assert_eq!(code.as_str(), "limit_exceeded");
            }
            (false, Ok(ToolOutcome::Success { .. }))
            | (_, Err(crate::PipelineError::Executor | crate::PipelineError::ResultLimit)) => {}
            (_, Err(error)) => panic!("unexpected pipeline error: {error:?}"),
            (_, other) => panic!("unexpected pipeline result: {other:?}"),
        }
        bundle.shutdown().await.unwrap();
        assert_eq!(fs::read_dir(&fixture.overflow).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn one_shot_raw_below_at_above_exact_limit() {
    let fixture = test_support::Fixture::new("one-shot-below");
    assert_eq!(
        one_shot(&fixture, TOOL_RESULT_BYTES_MAX.value - 1)
            .await
            .unwrap(),
        TOOL_RESULT_BYTES_MAX.value - 1
    );
    let fixture = test_support::Fixture::new("one-shot-at");
    assert_eq!(
        one_shot(&fixture, TOOL_RESULT_BYTES_MAX.value)
            .await
            .unwrap(),
        TOOL_RESULT_BYTES_MAX.value
    );
    let fixture = test_support::Fixture::new("one-shot-above");
    assert_eq!(
        one_shot(&fixture, TOOL_RESULT_BYTES_MAX.value + 1).await,
        Err(ManagerError::Limit)
    );
}

#[tokio::test]
async fn background_aggregate_retention_and_terminal_matrix() {
    for bytes in [
        CHILD_PROCESS_OUTPUT_BYTES_MAX - 1,
        CHILD_PROCESS_OUTPUT_BYTES_MAX,
        CHILD_PROCESS_OUTPUT_BYTES_MAX + 1,
    ] {
        let fixture = test_support::Fixture::new("aggregate");
        let (sandbox, active) = ScriptedSandbox::bytes(bytes);
        let manager = manager(&fixture, sandbox);
        let id = manager
            .start(
                "printf bounded",
                Path::new(""),
                Duration::from_secs(5),
                "exec",
                false,
            )
            .await
            .unwrap();
        finished(&manager, &id).await;
        let state = manager.inspect(&id).unwrap();
        assert_eq!(
            state.aggregate_bytes,
            bytes.min(CHILD_PROCESS_OUTPUT_BYTES_MAX)
        );
        assert!(state.retained_bytes <= SHELL_SESSION_OUTPUT_BYTES_MAX);
        assert!(state.read_offset <= state.retained_bytes);
        assert!(state.peak_retained_bytes <= SHELL_SESSION_OUTPUT_BYTES_MAX);
        assert_eq!(state.session_count, 1);
        assert!(state.join_present && state.join_finished);
        if bytes > CHILD_PROCESS_OUTPUT_BYTES_MAX {
            assert_eq!(
                (state.status, state.terminal),
                (SessionStatus::Failed, Some(ManagerError::Limit))
            );
        } else {
            assert_eq!(
                (state.status, state.terminal),
                (SessionStatus::Completed, None)
            );
        }
        manager.shutdown().await.unwrap();
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn rolling_retention_tail_and_unicode() {
    let fixture = test_support::Fixture::new("rolling");
    let mut script = Vec::new();
    let mut remaining = 999_999;
    while remaining != 0 {
        let count = remaining.min(PROCESS_OUTPUT_CHUNK_BYTES_MAX.value);
        script.push((false, vec![b'a'; count]));
        remaining -= count;
    }
    script.extend([
        (true, vec![b'b']),
        (false, "髪".as_bytes()[..2].to_vec()),
        (true, "髪".as_bytes()[2..].to_vec()),
        (false, b"ORDER-END".to_vec()),
    ]);
    let expected = script
        .iter()
        .flat_map(|(_, bytes)| bytes.iter().copied())
        .collect::<Vec<_>>();
    let mut sandbox = ScriptedSandbox::script(script, Arc::new(AtomicUsize::new(0)));
    sandbox.interactive = None;
    let manager = manager(&fixture, sandbox);
    let id = manager
        .start(
            "printf bounded",
            Path::new(""),
            Duration::from_secs(5),
            "exec",
            false,
        )
        .await
        .unwrap();
    finished(&manager, &id).await;
    let first = manager.read_exec(&id).unwrap();
    assert!(first.output.len() <= expected.len());
    let state = manager.inspect(&id).unwrap();
    assert_eq!(state.terminal, None);
    assert_eq!(state.aggregate_bytes, expected.len());
    assert_eq!(state.retained_bytes, SHELL_SESSION_OUTPUT_BYTES_MAX);
    assert_eq!(state.peak_retained_bytes, SHELL_SESSION_OUTPUT_BYTES_MAX);
    assert!(state.read_offset <= state.retained_bytes);
    let (output, _) = manager.output(&id).unwrap();
    let tail = &expected[expected.len() - SHELL_SESSION_OUTPUT_BYTES_MAX..];
    assert_eq!(output, String::from_utf8_lossy(tail));
    assert!(output.ends_with("ORDER-END"));
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn session_capacity_and_eviction() {
    let fixture = test_support::Fixture::new("capacity");
    let (sandbox, state) = ScriptedSandbox::interactive();
    let spawned = Arc::clone(&sandbox.spawned);
    let active = Arc::clone(&sandbox.active);
    let manager = manager(&fixture, sandbox);
    let mut ids = Vec::new();
    for _ in 0..SHELL_SESSIONS_MAX {
        ids.push(
            manager
                .start("cat", Path::new(""), Duration::from_secs(5), "exec", true)
                .await
                .unwrap(),
        );
    }
    assert_eq!(ids.len(), 32);
    assert_eq!(
        manager
            .start("cat", Path::new(""), Duration::from_secs(5), "exec", true)
            .await,
        Err(ManagerError::Limit)
    );
    assert_eq!(spawned.load(Ordering::SeqCst), 32);
    {
        state.sessions.lock().unwrap()[0].cancel();
    }
    finished(&manager, &ids[0]).await;
    let next = manager
        .start("cat", Path::new(""), Duration::from_secs(5), "exec", true)
        .await
        .unwrap();
    assert!(manager.inspect(&ids[0]).is_none());
    assert!(ids[1..].iter().all(|id| {
        manager
            .inspect(id)
            .is_some_and(|value| value.status == SessionStatus::Running)
    }));
    assert!(manager.inspect(&next).is_some());
    assert_eq!(manager.session_count(), 32);
    manager.shutdown().await.unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(state.sessions.lock().unwrap().len(), 33);
}

#[test]
fn id_and_command_boundaries() {
    assert_eq!(
        validate_id(&"a".repeat(SHELL_SESSION_ID_BYTES_MAX - 1)),
        Ok(())
    );
    assert_eq!(validate_id(&"a".repeat(SHELL_SESSION_ID_BYTES_MAX)), Ok(()));
    assert_eq!(
        validate_id(&"a".repeat(SHELL_SESSION_ID_BYTES_MAX + 1)),
        Err(ManagerError::Invalid)
    );
    assert_eq!(validate_id(""), Err(ManagerError::Invalid));
    assert_eq!(validate_id("bad-id"), Err(ManagerError::Invalid));
    assert_eq!(
        validate_command(&"x".repeat(SHELL_COMMAND_BYTES_MAX - 1)),
        Ok(())
    );
    assert_eq!(
        validate_command(&"x".repeat(SHELL_COMMAND_BYTES_MAX)),
        Ok(())
    );
    assert_eq!(
        validate_command(&"x".repeat(SHELL_COMMAND_BYTES_MAX + 1)),
        Err(ManagerError::Invalid)
    );
    assert_eq!(validate_command(""), Err(ManagerError::Invalid));
    assert_eq!(validate_command("x\0y"), Err(ManagerError::Invalid));
}

#[tokio::test]
async fn stdin_call_and_aggregate_boundaries() {
    for size in [
        SHELL_STDIN_WRITE_BYTES_MAX - 1,
        SHELL_STDIN_WRITE_BYTES_MAX,
        SHELL_STDIN_WRITE_BYTES_MAX + 1,
    ] {
        assert_stdin_call_bound(size).await;
    }
    assert_stdin_total_bound().await;
}

async fn assert_stdin_call_bound(size: usize) {
    let fixture = test_support::Fixture::new("stdin-call");
    let (sandbox, state) = ScriptedSandbox::interactive();
    let manager = manager(&fixture, sandbox);
    let id = manager
        .start("cat", Path::new(""), Duration::from_secs(5), "exec", true)
        .await
        .unwrap();
    let input = vec![b'x'; size];
    let write = manager.write(&id, &input, false);
    let result = tokio::time::timeout(Duration::from_secs(5), write)
        .await
        .expect("boundary write timeout");
    let expected = if size > SHELL_STDIN_WRITE_BYTES_MAX {
        0
    } else {
        size
    };
    let wanted = (size <= SHELL_STDIN_WRITE_BYTES_MAX)
        .then_some(())
        .ok_or(ManagerError::Limit);
    assert_eq!(result, wanted);
    tokio::time::timeout(Duration::from_secs(5), manager.write(&id, &[], true))
        .await
        .expect("boundary eof timeout")
        .unwrap();
    received_bytes(&state, expected).await;
    finished(&manager, &id).await;
    assert_eq!(state.received.lock().unwrap().len(), expected);
    manager.shutdown().await.unwrap();
}

async fn assert_stdin_total_bound() {
    let fixture = test_support::Fixture::new("stdin-total");
    let (sandbox, state) = ScriptedSandbox::interactive();
    let manager = manager(&fixture, sandbox);
    let id = manager
        .start("cat", Path::new(""), Duration::from_secs(5), "exec", true)
        .await
        .unwrap();
    let chunk = vec![b'y'; SHELL_STDIN_WRITE_BYTES_MAX];
    for _ in 0..SHELL_STDIN_TOTAL_BYTES_MAX / chunk.len() {
        tokio::time::timeout(Duration::from_secs(5), manager.write(&id, &chunk, false))
            .await
            .expect("aggregate stdin write timeout")
            .unwrap();
    }
    assert_eq!(
        manager.write(&id, b"z", false).await,
        Err(ManagerError::Limit)
    );
    tokio::time::timeout(Duration::from_secs(5), manager.write(&id, &[], true))
        .await
        .expect("aggregate stdin eof timeout")
        .unwrap();
    received_bytes(&state, SHELL_STDIN_TOTAL_BYTES_MAX).await;
    finished(&manager, &id).await;
    assert_eq!(
        state.received.lock().unwrap().len(),
        SHELL_STDIN_TOTAL_BYTES_MAX
    );
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn bounded_channels_backpressure() {
    let (sender, mut receiver) = mpsc::channel::<ProcessInput>(PROCESS_CHANNEL_ITEMS_MAX);
    for _ in 0..PROCESS_CHANNEL_ITEMS_MAX {
        sender
            .send(ProcessInput::Bytes(ProcessStdin::new(vec![b'x']).unwrap()))
            .await
            .unwrap();
    }
    let (done, mut pending) = oneshot::channel();
    let blocked = tokio::spawn(async move {
        sender.send(ProcessInput::Eof).await.unwrap();
        let _ = done.send(());
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut pending)
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("input receiver timeout")
            .is_some()
    );
    tokio::time::timeout(Duration::from_secs(1), blocked)
        .await
        .expect("input sender remained blocked")
        .unwrap();

    let (events, mut output) = mpsc::channel(PROCESS_CHANNEL_ITEMS_MAX);
    let event = ProcessEvent::Stdout(ProcessOutputChunk::new(vec![b'x']).unwrap());
    for _ in 0..PROCESS_CHANNEL_ITEMS_MAX {
        events.send(event.clone()).await.unwrap();
    }
    let blocked = tokio::spawn(async move { events.send(event).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!blocked.is_finished());
    assert!(
        tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .expect("event receiver timeout")
            .is_some()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .expect("event sender remained blocked")
            .unwrap()
            .is_ok()
    );
}
