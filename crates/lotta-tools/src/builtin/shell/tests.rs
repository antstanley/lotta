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

#[cfg(target_os = "macos")]
mod production_cancellation {
    use super::*;
    use crate::{
        ToolsetId,
        sandbox::{OsSandbox, WorkspacePolicy},
    };
    use lotta_runtime::{WorkspaceSandbox, turn::TurnChildOwner};
    use serde_json::json;
    use std::time::Duration;

    async fn wait_for_file(path: &std::path::Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if path.exists() {
                return;
            }
            tokio::task::yield_now().await;
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("process did not create {}", path.display());
    }

    fn process_exists(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }

    async fn launch(
        fixture: &super::test_support::Fixture,
        bundle: &ShellToolBundle,
        command: String,
    ) -> String {
        let (outcome, _) = super::test_support::execute_bundle(
            fixture,
            bundle,
            ToolsetId::Default,
            "Bash",
            json!({
                "command": command,
                "description": "production cancellation process",
                "run_in_background": true
            }),
            CancellationToken::new(),
        )
        .await;
        super::test_support::success_text(&outcome.unwrap())
            .split(": ")
            .last()
            .unwrap()
            .to_owned()
    }

    #[tokio::test(start_paused = true)]
    async fn production_shell_registration_cancellation_is_scoped_exact_and_reaped() {
        let fixture = super::test_support::Fixture::new("production-cancel");
        let policy = WorkspacePolicy::new(&WorkspaceSandbox::new(
            fixture.workspace.clone(),
            fixture.root.clone(),
        ))
        .unwrap();
        let bundle = ShellToolBundle::new(
            &fixture.workspace,
            super::test_support::scope(),
            Arc::new(OsSandbox::detect(policy)),
        )
        .unwrap();
        let identity_owner = bundle
            .cancellation_owner(super::test_support::scope(), 57)
            .unwrap();
        assert_eq!(bundle.manager_id(), identity_owner.manager_id());

        let parent_file = fixture.workspace.join("parent.pid");
        let child_file = fixture.workspace.join("child.pid");
        let survivor_file = fixture.workspace.join("survivor.pid");
        let owned_task = launch(
            &fixture,
            &bundle,
            format!(
                concat!(
                    "trap '' TERM; printf %s $$ > '{}'; ",
                    "(trap '' TERM; printf %s $$ > '{}'; exec sleep 300) & wait"
                ),
                parent_file.display(),
                child_file.display()
            ),
        )
        .await;
        wait_for_file(&parent_file).await;
        wait_for_file(&child_file).await;
        let survivor_owner = bundle
            .cancellation_owner(super::test_support::scope(), 58)
            .unwrap();
        let survivor = launch(
            &fixture,
            &bundle,
            format!(
                "trap '' TERM; printf %s $$ > '{}'; exec sleep 300",
                survivor_file.display()
            ),
        )
        .await;
        wait_for_file(&survivor_file).await;
        let owned_owner = bundle
            .cancellation_owner(super::test_support::scope(), 57)
            .unwrap();
        let parent: u32 = std::fs::read_to_string(parent_file)
            .unwrap()
            .parse()
            .unwrap();
        let child: u32 = std::fs::read_to_string(child_file)
            .unwrap()
            .parse()
            .unwrap();
        let surviving: u32 = std::fs::read_to_string(survivor_file)
            .unwrap()
            .parse()
            .unwrap();

        let cancellation = owned_owner.terminate_and_reap(Duration::from_secs(2));
        tokio::pin!(cancellation);
        assert!(
            tokio::time::timeout(Duration::ZERO, &mut cancellation)
                .await
                .is_err()
        );
        tokio::time::advance(Duration::from_millis(1_999)).await;
        assert!(process_exists(parent));
        assert!(process_exists(child));
        assert!(process_exists(surviving));
        tokio::time::advance(Duration::from_millis(1)).await;
        cancellation.await.unwrap();
        assert!(!process_exists(parent));
        assert!(!process_exists(child));
        assert!(process_exists(surviving));
        assert!(!owned_owner.has_operations());

        let empty = bundle
            .cancellation_owner(super::test_support::scope(), 59)
            .unwrap();
        assert!(!empty.has_operations());
        empty
            .terminate_and_reap(Duration::from_secs(2))
            .await
            .unwrap();
        assert!(process_exists(surviving));

        survivor_owner
            .terminate_and_reap(Duration::from_secs(2))
            .await
            .unwrap();
        assert!(!process_exists(surviving));
        assert!(!bundle.has_operations());
        assert!(!owned_task.is_empty());
        assert!(!survivor.is_empty());
    }
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
