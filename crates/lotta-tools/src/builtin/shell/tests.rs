use super::*;
use lotta_domain::{AgentId, ConversationId};
#[cfg(not(target_os = "macos"))]
use lotta_runtime::{
    RuntimeError,
    ports::{
        InteractiveSandboxPort, PortFuture, ProcessEvent, ProcessOutcome, ProcessRequest,
        ProcessSessionFuture, SandboxPort,
    },
};
use std::sync::Arc;
#[cfg(not(target_os = "macos"))]
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

#[cfg(not(target_os = "macos"))]
struct FakeSandbox;
#[cfg(not(target_os = "macos"))]
impl InteractiveSandboxPort for FakeSandbox {
    fn start_session(&self, _: ProcessRequest, _: CancellationToken) -> ProcessSessionFuture<'_> {
        Box::pin(async {
            Err(RuntimeError::Unsupported {
                context: "fake interactive sandbox".into(),
            })
        })
    }
}
#[cfg(not(target_os = "macos"))]
impl SandboxPort for FakeSandbox {
    fn execute(
        &self,
        request: ProcessRequest,
        events: Sender<ProcessEvent>,
        _: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        Box::pin(async move {
            let command = request
                .arguments
                .as_slice()
                .last()
                .map_or("", lotta_runtime::boundary::ProcessArgument::as_str);
            let chunk =
                lotta_runtime::boundary::ProcessOutputChunk::new(command.as_bytes().to_vec())?;
            events
                .send(ProcessEvent::Stdout(chunk))
                .await
                .map_err(|_| RuntimeError::AdapterFailure {
                    code: "test",
                    context: "shell test".into(),
                })?;
            Ok(ProcessOutcome {
                exit_code: Some(0),
                timed_out: false,
            })
        })
    }
}

#[cfg(not(target_os = "macos"))]
fn bundle() -> ShellToolBundle {
    let root = std::env::current_dir().unwrap().canonicalize().unwrap();
    let scope = RuntimeScope::new(
        AgentId::accept("agent").unwrap(),
        ConversationId::accept("conversation").unwrap(),
        None,
    );
    ShellToolBundle::new(&root, scope, Arc::new(FakeSandbox)).unwrap()
}

#[test]
fn output_bounds() {
    assert_eq!(CHILD_PROCESS_OUTPUT_BYTES_MAX, 16 * 1024 * 1024);
    assert!(
        lotta_runtime::boundary::ProcessOutputBytesMax::new(CHILD_PROCESS_OUTPUT_BYTES_MAX - 1,)
            .is_ok()
    );
    assert!(
        lotta_runtime::boundary::ProcessOutputBytesMax::new(CHILD_PROCESS_OUTPUT_BYTES_MAX,)
            .is_ok()
    );
}

mod lifecycle {
    use super::*;
    use crate::sandbox::{OsSandbox, WorkspacePolicy};
    use lotta_runtime::WorkspaceSandbox;
    use std::{path::PathBuf, time::Duration};

    struct TestRoot(PathBuf);
    impl TestRoot {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "lotta-shell-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }
    }
    impl Drop for TestRoot {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn manager(label: &str) -> (TestRoot, super::super::manager::ProcessManager) {
        let isolation = TestRoot::new(label);
        let root = isolation.0.join("workspace");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let policy =
            WorkspacePolicy::new(&WorkspaceSandbox::new(root.clone(), isolation.0.clone()))
                .unwrap();
        let scope = RuntimeScope::new(
            AgentId::accept("agent").unwrap(),
            ConversationId::accept("conversation").unwrap(),
            None,
        );
        (
            isolation,
            super::super::manager::ProcessManager::new(
                root,
                scope,
                Arc::new(OsSandbox::detect(policy)),
            ),
        )
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn one_shot_stdout_stderr_and_exit() {
        let (_root, manager) = manager("one-shot");
        let result = manager
            .one_shot(
                "printf out; printf err >&2; exit 7",
                std::path::Path::new(""),
                Duration::from_secs(5),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(7));
        assert!(result.output.contains("out"));
        assert!(result.output.contains("err"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn interactive_background_write_completion_and_shutdown() {
        let (_root, manager) = manager("interactive-manager");
        let id = manager
            .start(
                "test -t 0 && test -t 1 && read value && printf 'VALUE:%s\\n' \"$value\"",
                std::path::Path::new(""),
                Duration::from_secs(5),
                "exec",
                true,
            )
            .await
            .unwrap();
        manager.write(&id, b"ponytail\n", false).await.unwrap();
        for _ in 0..100 {
            if manager.output(&id).unwrap().1 != super::super::manager::SessionStatus::Running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (output, status) = manager.output(&id).unwrap();
        assert_eq!(status, super::super::manager::SessionStatus::Completed);
        assert!(output.contains("VALUE:ponytail"));
        manager.shutdown().await.unwrap();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn stop_joins_and_stores_terminal_reason() {
        let (_root, manager) = manager("stop");
        let id = manager
            .start(
                "while :; do sleep 1; done",
                std::path::Path::new(""),
                Duration::from_secs(30),
                "bash",
                false,
            )
            .await
            .unwrap();
        assert!(manager.stop(&id).await.unwrap());
        assert_eq!(
            manager.output(&id).unwrap().1,
            super::super::manager::SessionStatus::Stopped
        );
        assert!(!manager.stop(&id).await.unwrap());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn timeout_and_cancellation_are_distinct() {
        let (_root, manager) = manager("terminal-reasons");
        let timeout = manager
            .one_shot(
                "sleep 30",
                std::path::Path::new(""),
                Duration::from_millis(10),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(
            timeout.unwrap_err(),
            super::super::manager::ManagerError::Timeout
        );
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = manager
            .one_shot(
                "sleep 30",
                std::path::Path::new(""),
                Duration::from_secs(30),
                cancellation,
            )
            .await;
        assert_eq!(
            cancelled.unwrap_err(),
            super::super::manager::ManagerError::Interrupted
        );
    }
}
