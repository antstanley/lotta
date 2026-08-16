use crate::{
    ToolsetId,
    clamp::OverflowWriter,
    permissions::{
        PermissionDecision, PermissionGate, PermissionInvocation, matcher::PermissionError,
    },
    pipeline::{
        OutcomeSink, PipelineError, PipelineHooks, PipelineRequest, PipelineStage, PostHookStatus,
        PreHookResult, RawToolExecutionRequest, RawToolOutcome, SecretResolver, ToolExecutor,
        TraceEvent, TraceSink, execute,
    },
    registry::{ToolRegistration, ToolRegistry},
};
use lotta_domain::BoundedJsonValue;
use lotta_runtime::{
    RuntimeError, WorkspaceSandbox,
    boundary::{ProcessOutputBytesMax, ProcessStdin},
    bounds::{EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX},
    ports::*,
};
use std::{
    future,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const OWN_SECRET: &[u8] = b"own-secret-sentinel";
const PEER_SECRET: &[u8] = b"peer-secret-sentinel";

struct Fixture {
    isolation: PathBuf,
    own: PathBuf,
    peer: PathBuf,
    policy: WorkspacePolicy,
    sandbox: OsSandbox,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let isolation = temp_root(label);
        let own = isolation.join("self");
        let peer = isolation.join("peer");
        std::fs::create_dir(&own).unwrap();
        std::fs::create_dir(&peer).unwrap();
        std::fs::write(own.join("secret"), OWN_SECRET).unwrap();
        std::fs::write(peer.join("secret"), PEER_SECRET).unwrap();
        let policy =
            WorkspacePolicy::new(&WorkspaceSandbox::new(own.clone(), isolation.clone())).unwrap();
        let sandbox = OsSandbox::detect(policy.clone());
        Self {
            isolation,
            own,
            peer,
            policy,
            sandbox,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.isolation).unwrap();
    }
}

async fn collect(
    sandbox: &dyn SandboxPort,
    request: ProcessRequest,
    cancellation: CancellationToken,
) -> Result<(ProcessOutcome, Vec<u8>), RuntimeError> {
    let (sender, mut receiver) = mpsc::channel(2);
    let execute = sandbox.execute(request, sender, cancellation);
    let drain = async move {
        let mut bytes = Vec::new();
        while let Some(event) = receiver.recv().await {
            match event {
                ProcessEvent::Stdout(chunk) | ProcessEvent::Stderr(chunk) => {
                    bytes.extend_from_slice(chunk.as_slice());
                }
            }
        }
        bytes
    };
    let (outcome, bytes) = tokio::join!(execute, drain);
    outcome.map(|outcome| (outcome, bytes))
}

fn request(
    fixture: &Fixture,
    program: &str,
    arguments: &[&str],
    stdin: Option<Vec<u8>>,
    maximum: usize,
    timeout: Duration,
) -> ProcessRequest {
    use lotta_domain::{AgentId, ConversationId, RuntimeScope};
    ProcessRequest::new(
        RuntimeScope::new(
            AgentId::accept("agent").unwrap(),
            ConversationId::accept("conversation").unwrap(),
            None,
        ),
        lotta_runtime::boundary::Program::new(program.into()).unwrap(),
        lotta_runtime::boundary::ProcessArguments::new(
            arguments
                .iter()
                .map(|value| {
                    lotta_runtime::boundary::ProcessArgument::new((*value).into()).unwrap()
                })
                .collect(),
        )
        .unwrap(),
        lotta_runtime::boundary::ConfinedPath::new(fixture.own.clone(), fixture.own.clone())
            .unwrap(),
        lotta_runtime::boundary::ProcessEnvironment::new(Vec::new()).unwrap(),
        stdin.map(|bytes| ProcessStdin::new(bytes).unwrap()),
        ProcessOutputBytesMax::new(maximum).unwrap(),
        timeout,
    )
    .unwrap()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlatformSkipReason {
    #[cfg(target_os = "linux")]
    BackendUnavailable,
    NotCurrentPlatform,
}

mod adapters {
    use super::*;

    #[test]
    fn renderer_order_and_path_values_are_pinned() {
        let workspace = PathBuf::from("/isolation/work space'quoted");
        let isolation = PathBuf::from("/isolation");
        let seatbelt = seatbelt_arguments(&workspace, &isolation, "/bin/cat", &["file".into()]);
        assert_eq!(seatbelt[0..2], ["-p", SEATBELT_PROFILE]);
        assert_eq!(seatbelt[2], "-DISOLATION_ROOT=/isolation");
        assert_eq!(seatbelt[3], "-DWORKSPACE_ROOT=/isolation/work space'quoted");
        assert_eq!(&seatbelt[4..], ["--", "/bin/cat", "file"]);
        assert_eq!(
            bubblewrap_arguments(&workspace, &isolation, "/bin/cat", &["file".into()]),
            [
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--tmpfs",
                "/isolation",
                "--bind",
                "/isolation/work space'quoted",
                "/isolation/work space'quoted",
                "--die-with-parent",
                "--",
                "/bin/cat",
                "file"
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn seatbelt_blocks_peer_read() {
        let fixture = Fixture::new("seatbelt");
        assert_eq!(fixture.sandbox.backend(), &SandboxBackend::Seatbelt);
        let peer = fixture.peer.join("secret");
        let (blocked, bytes) = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/bin/cat",
                &[peer.to_str().unwrap()],
                None,
                1024,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_ne!(blocked.exit_code, Some(0));
        assert!(
            !bytes
                .windows(PEER_SECRET.len())
                .any(|value| value == PEER_SECRET)
        );
        let own = fixture.own.join("secret");
        let (allowed, bytes) = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/bin/cat",
                &[own.to_str().unwrap()],
                None,
                1024,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(allowed.exit_code, Some(0));
        assert!(
            bytes
                .windows(OWN_SECRET.len())
                .any(|value| value == OWN_SECRET)
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn seatbelt_blocks_peer_read() {
        assert_eq!(
            PlatformSkipReason::NotCurrentPlatform,
            PlatformSkipReason::NotCurrentPlatform
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn bubblewrap_blocks_peer_read() {
        let fixture = Fixture::new("bubblewrap");
        match fixture.sandbox.backend() {
            SandboxBackend::Bubblewrap { .. } => {
                let peer = fixture.peer.join("secret");
                let (outcome, bytes) = collect(
                    &fixture.sandbox,
                    request(
                        &fixture,
                        "/bin/cat",
                        &[peer.to_str().unwrap()],
                        None,
                        1024,
                        Duration::from_secs(5),
                    ),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
                assert_ne!(outcome.exit_code, Some(0));
                assert!(
                    !bytes
                        .windows(PEER_SECRET.len())
                        .any(|value| value == PEER_SECRET)
                );
            }
            SandboxBackend::Unsupported => assert_eq!(
                PlatformSkipReason::BackendUnavailable,
                PlatformSkipReason::BackendUnavailable
            ),
            SandboxBackend::Seatbelt => panic!(),
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn bubblewrap_blocks_peer_read() {
        assert_eq!(
            PlatformSkipReason::NotCurrentPlatform,
            PlatformSkipReason::NotCurrentPlatform
        );
    }

    #[tokio::test]
    async fn unsupported_platform_errors_before_exec() {
        let fixture = Fixture::new("unsupported");
        let sandbox = OsSandbox::with_backend(fixture.policy.clone(), SandboxBackend::Unsupported);
        let marker = fixture.own.join("marker");
        let error = collect(
            &sandbox,
            request(
                &fixture,
                "/usr/bin/touch",
                &[marker.to_str().unwrap()],
                None,
                1,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(error, unsupported_workspace_sandbox());
        assert!(!marker.exists());
    }
}

mod workspace {
    use super::*;

    #[test]
    fn policy_requires_strict_real_descendant() {
        let fixture = Fixture::new("policy");
        assert!(
            WorkspacePolicy::new(&WorkspaceSandbox::new(
                fixture.isolation.clone(),
                fixture.isolation.clone()
            ))
            .is_err()
        );
        let outside = temp_root("outside-root");
        let link = outside.join("link");
        std::os::unix::fs::symlink(&fixture.isolation, &link).unwrap();
        assert!(WorkspacePolicy::new(&WorkspaceSandbox::new(fixture.own.clone(), link)).is_err());
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[tokio::test]
    async fn file_tool_confined() {
        let fixture = Fixture::new("gate");
        for value in [
            fixture.peer.join("secret"),
            fixture.isolation.parent().unwrap().join("outside"),
        ] {
            let counter = Arc::new(AtomicUsize::new(0));
            let registry = tool_registry(Arc::clone(&counter));
            let gate = WorkspaceSandboxGate::new(&fixture.policy, &fixture.own);
            let sink = RecordingSink::default();
            let outcome = execute(PipelineRequest {
                registry: registry.snapshot().unwrap(),
                model_name: "Read",
                input: BoundedJsonValue::new(
                    serde_json::json!({"file_path":value.to_str().unwrap()}),
                )
                .unwrap(),
                cancellation: CancellationToken::new(),
                hooks: &RecordingHooks::default(),
                permissions: &crate::AllowAllPermissions,
                sandbox: &gate,
                secrets: &Counters::default(),
                trace: &RecordingTrace::default(),
                overflow: &sink,
                persistence: &sink,
                emit: &sink,
            })
            .await
            .unwrap();
            assert!(matches!(outcome, ToolOutcome::SandboxDenied { .. }));
            assert_eq!(counter.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn gemini_dir_paths_are_confined() {
        let fixture = Fixture::new("gemini-dir-path");
        let gate = WorkspaceSandboxGate::new(&fixture.policy, &fixture.own);
        let aliases = ["glob_gemini", "search_file_content", "list_directory"];
        for alias in aliases {
            for target in [
                fixture.peer.as_path(),
                fixture.isolation.parent().unwrap(),
            ] {
                let input = sandbox_input(serde_json::json!({
                    "dir_path": target,
                    "pattern": "*"
                }));
                assert_eq!(sandbox_check(&gate, alias, &input), Ok(SandboxDecision::Deny));
            }
            let inside = sandbox_input(serde_json::json!({
                "dir_path": fixture.own,
                "pattern": "*"
            }));
            assert_eq!(sandbox_check(&gate, alias, &inside), Ok(SandboxDecision::Allow));
            let malformed = sandbox_input(serde_json::json!({"dir_path": 7}));
            assert_eq!(sandbox_check(&gate, alias, &malformed), Err(SandboxError));
            let missing = sandbox_input(serde_json::json!({"pattern": "*"}));
            assert_eq!(sandbox_check(&gate, alias, &missing), Err(SandboxError));
        }
    }

    #[test]
    fn image_alias_paths_are_confined() {
        let fixture = Fixture::new("image-path");
        let gate = WorkspaceSandboxGate::new(&fixture.policy, &fixture.own);
        for alias in ["view_image", "ViewImage"] {
            for target in [
                fixture.peer.as_path(),
                fixture.isolation.parent().unwrap(),
            ] {
                let input = sandbox_input(serde_json::json!({"path": target}));
                assert_eq!(sandbox_check(&gate, alias, &input), Ok(SandboxDecision::Deny));
            }
            let inside = sandbox_input(serde_json::json!({"path": fixture.own}));
            assert_eq!(sandbox_check(&gate, alias, &inside), Ok(SandboxDecision::Allow));
        }
    }

    #[tokio::test]
    async fn child_process_confined() {
        let fixture = Fixture::new("child");
        let peer = fixture.peer.join("secret");
        let (outcome, bytes) = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/bin/cat",
                &[peer.to_str().unwrap()],
                None,
                1024,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_ne!(outcome.exit_code, Some(0));
        assert!(
            !bytes
                .windows(PEER_SECRET.len())
                .any(|value| value == PEER_SECRET)
        );
    }

    #[tokio::test]
    async fn peer_workspace_hidden() {
        let fixture = Fixture::new("hidden");
        let (outcome, bytes) = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/bin/ls",
                &[fixture.isolation.to_str().unwrap()],
                None,
                1024,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_ne!(outcome.exit_code, Some(0));
        assert!(!bytes.windows(b"peer".len()).any(|value| value == b"peer"));
    }

    #[tokio::test]
    async fn cwd_symlinks_rejected_before_spawn() {
        let fixture = Fixture::new("cwd-links");
        for (name, target) in [
            ("peer-link", fixture.peer.clone()),
            (
                "outside-link",
                fixture.isolation.parent().unwrap().to_path_buf(),
            ),
        ] {
            let link = fixture.own.join(name);
            std::os::unix::fs::symlink(target, &link).unwrap();
            let marker = fixture.own.join(format!("{name}-marker"));
            let mut process = request(
                &fixture,
                "/usr/bin/touch",
                &[marker.to_str().unwrap()],
                None,
                1,
                Duration::from_secs(5),
            );
            process.working_directory =
                lotta_runtime::boundary::ConfinedPath::new(fixture.own.clone(), link).unwrap();
            assert!(
                collect(&fixture.sandbox, process, CancellationToken::new())
                    .await
                    .is_err()
            );
            assert!(!marker.exists());
        }
    }
}

mod outcome_kind {
    use super::*;

    #[tokio::test]
    async fn outcome_kind() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = tool_registry(Arc::clone(&counter));
        let trace = RecordingTrace::default();
        let hooks = RecordingHooks::default();
        let counters = Counters::default();
        let persistence = RecordingSink::default();
        let emit = RecordingSink::default();
        let outcome = execute(PipelineRequest {
            registry: registry.snapshot().unwrap(),
            model_name: "Read",
            input: BoundedJsonValue::new(serde_json::json!({"file_path":"/outside"})).unwrap(),
            cancellation: CancellationToken::new(),
            hooks: &hooks,
            permissions: &CountingPermission(&counters, PermissionDecision::Allow),
            sandbox: &CountingSandbox(&counters, Ok(SandboxDecision::Deny)),
            secrets: &counters,
            trace: &trace,
            overflow: &persistence,
            persistence: &persistence,
            emit: &emit,
        })
        .await
        .unwrap();
        assert!(matches!(outcome, ToolOutcome::SandboxDenied { .. }));
        assert_eq!(
            trace.0.lock().unwrap().as_slice(),
            &[
                TraceEvent::Preflight(crate::pipeline::PreflightEvent::NameResolution),
                TraceEvent::Preflight(crate::pipeline::PreflightEvent::SchemaValidation),
                TraceEvent::Stage(PipelineStage::PreHook),
                TraceEvent::Stage(PipelineStage::Permission),
                TraceEvent::Stage(PipelineStage::Sandbox),
                TraceEvent::Stage(PipelineStage::PostHook),
                TraceEvent::Stage(PipelineStage::Scrub),
                TraceEvent::Stage(PipelineStage::Clamp),
                TraceEvent::Stage(PipelineStage::Persist),
                TraceEvent::Stage(PipelineStage::Emit),
            ]
        );
        assert_eq!(
            hooks.posts.lock().unwrap().as_slice(),
            &[PostHookStatus::Failed]
        );
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        assert_eq!(counters.secret.load(Ordering::Relaxed), 0);
        assert_eq!(counters.sandbox.load(Ordering::Relaxed), 1);
        assert_eq!(
            persistence.outcomes.lock().unwrap().as_slice(),
            std::slice::from_ref(&outcome)
        );
        assert_eq!(
            emit.outcomes.lock().unwrap().as_slice(),
            std::slice::from_ref(&outcome)
        );
    }

    #[tokio::test]
    async fn permission_deny_stops_before_sandbox() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = tool_registry(Arc::clone(&counter));
        let counters = Counters::default();
        let sink = RecordingSink::default();
        let result = execute(PipelineRequest {
            registry: registry.snapshot().unwrap(),
            model_name: "Read",
            input: valid_input(),
            cancellation: CancellationToken::new(),
            hooks: &RecordingHooks::default(),
            permissions: &CountingPermission(&counters, PermissionDecision::Deny),
            sandbox: &CountingSandbox(&counters, Ok(SandboxDecision::Allow)),
            secrets: &counters,
            trace: &RecordingTrace::default(),
            overflow: &sink,
            persistence: &sink,
            emit: &sink,
        })
        .await;
        assert_eq!(result.unwrap_err(), PipelineError::PermissionDenied);
        assert_eq!(counters.sandbox.load(Ordering::Relaxed), 0);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        assert!(sink.outcomes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sandbox_error_stops_without_sinks() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = tool_registry(Arc::clone(&counter));
        let counters = Counters::default();
        let sink = RecordingSink::default();
        let result = execute(PipelineRequest {
            registry: registry.snapshot().unwrap(),
            model_name: "Read",
            input: valid_input(),
            cancellation: CancellationToken::new(),
            hooks: &RecordingHooks::default(),
            permissions: &CountingPermission(&counters, PermissionDecision::Allow),
            sandbox: &CountingSandbox(&counters, Err(SandboxError)),
            secrets: &counters,
            trace: &RecordingTrace::default(),
            overflow: &sink,
            persistence: &sink,
            emit: &sink,
        })
        .await;
        assert_eq!(result.unwrap_err(), PipelineError::Sandbox);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        assert!(sink.outcomes.lock().unwrap().is_empty());
    }
}

mod process {
    use super::*;

    #[tokio::test]
    async fn stdout_success_and_nonzero() {
        let fixture = Fixture::new("stdout");
        let (success, bytes) = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/usr/bin/printf",
                &["hello"],
                None,
                5,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!((success.exit_code, bytes), (Some(0), b"hello".to_vec()));
        let (failure, _) = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/bin/sh",
                &["-c", "exit 7"],
                None,
                1,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(failure.exit_code, Some(7));
    }

    #[tokio::test]
    async fn stdin_echo() {
        let fixture = Fixture::new("stdin");
        let input = b"pending stdin echo".to_vec();
        let (outcome, bytes) = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/bin/cat",
                &[],
                Some(input.clone()),
                input.len(),
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(bytes, input);
    }

    #[tokio::test]
    async fn stdin_and_full_stdout_do_not_deadlock() {
        let fixture = Fixture::new("full-stdout");
        let input = vec![b'x'; 131_072];
        let (outcome, bytes) = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/bin/sh",
                &[
                    "-c",
                    "head -c 131072 /dev/zero; cat >/dev/null; printf done",
                ],
                Some(input),
                131_076,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(bytes.len(), 131_076);
        assert!(bytes.ends_with(b"done"));
    }

    #[tokio::test]
    async fn output_limit_exact_and_over() {
        let fixture = Fixture::new("limits");
        let (outcome, bytes) = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/usr/bin/printf",
                &["12345"],
                None,
                5,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(bytes, b"12345");
        let error = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/usr/bin/printf",
                &["123456"],
                None,
                5,
                Duration::from_secs(5),
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, RuntimeError::LimitExceeded { .. }));
    }

    #[tokio::test]
    async fn receiver_closed_gives_adapter_failure() {
        let fixture = Fixture::new("closed");
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let error = fixture
            .sandbox
            .execute(
                request(
                    &fixture,
                    "/usr/bin/printf",
                    &["x"],
                    None,
                    1,
                    Duration::from_secs(5),
                ),
                sender,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, RuntimeError::AdapterFailure { .. }));
    }

    #[tokio::test]
    async fn cancellation_and_timeout() {
        let fixture = Fixture::new("cancel");
        let marker = fixture.own.join("marker");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = collect(
            &fixture.sandbox,
            request(
                &fixture,
                "/usr/bin/touch",
                &[marker.to_str().unwrap()],
                None,
                1,
                Duration::from_secs(5),
            ),
            cancellation,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, RuntimeError::Cancelled { .. }));
        assert!(!marker.exists());
        let (outcome, _) = collect(
            &fixture.sandbox,
            request(&fixture, "/bin/sleep", &["60"], None, 1, Duration::ZERO),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(outcome.timed_out);
        assert_eq!(outcome.exit_code, None);
    }
}

#[cfg(target_os = "linux")]
mod linux_path {
    use super::runner::{PathSearch, SANDBOX_PATH_ENTRIES_MAX, search_path};
    use std::path::PathBuf;

    fn paths(count: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|index| PathBuf::from(format!("/missing/{index}")))
            .collect()
    }

    #[test]
    fn below_at_and_above_path_bound() {
        assert_eq!(
            search_path(paths(SANDBOX_PATH_ENTRIES_MAX - 1)),
            PathSearch::NotFound
        );
        assert_eq!(
            search_path(paths(SANDBOX_PATH_ENTRIES_MAX)),
            PathSearch::NotFound
        );
        assert_eq!(
            search_path(paths(SANDBOX_PATH_ENTRIES_MAX + 1)),
            PathSearch::TooMany
        );
    }

    #[test]
    fn finds_executable_bwrap() {
        let root = super::temp_root("path-found");
        let executable = root.join("bwrap");
        std::fs::write(&executable, b"#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            search_path([root.clone()]),
            PathSearch::Found(executable.canonicalize().unwrap())
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[derive(Default)]
struct RecordingTrace(Mutex<Vec<TraceEvent>>);
impl TraceSink for RecordingTrace {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[derive(Default)]
struct RecordingHooks {
    posts: Mutex<Vec<PostHookStatus>>,
}
impl PipelineHooks for RecordingHooks {
    fn pre(&self, _: &ValidatedToolInput) -> Result<PreHookResult, crate::pipeline::OwnerFailure> {
        Ok(PreHookResult::Allow)
    }
    fn post(&self, status: PostHookStatus) -> Result<(), crate::pipeline::OwnerFailure> {
        self.posts.lock().unwrap().push(status);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingSink {
    outcomes: Mutex<Vec<ToolOutcome>>,
}
impl OutcomeSink for RecordingSink {
    fn record(&self, _: &str, outcome: &ToolOutcome) -> Result<(), PipelineError> {
        self.outcomes.lock().unwrap().push(outcome.clone());
        Ok(())
    }
}
impl OverflowWriter for RecordingSink {
    fn write(&self, _: &str, _: &str) -> Result<String, crate::clamp::ClampError> {
        Ok("/overflow".into())
    }
}

#[derive(Default)]
struct Counters {
    secret: AtomicUsize,
    sandbox: AtomicUsize,
}
impl SecretResolver for Counters {
    fn resolve(&self, _: &str) -> Result<Option<String>, PipelineError> {
        self.secret.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
}

struct CountingPermission<'a>(&'a Counters, PermissionDecision);
impl PermissionGate for CountingPermission<'_> {
    fn check(&self, _: PermissionInvocation<'_>) -> Result<PermissionDecision, PermissionError> {
        let _ = self.0;
        Ok(self.1)
    }
}

struct CountingSandbox<'a>(&'a Counters, Result<SandboxDecision, SandboxError>);
impl SandboxGate for CountingSandbox<'_> {
    fn check(&self, _: SandboxInvocation<'_>) -> Result<SandboxDecision, SandboxError> {
        self.0.sandbox.fetch_add(1, Ordering::Relaxed);
        self.1
    }
}

struct CountingExecutor(Arc<AtomicUsize>);
impl ToolExecutor for CountingExecutor {
    fn execute(&self, _: RawToolExecutionRequest) -> crate::pipeline::ExecutorFuture<'_> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Box::pin(future::ready(Ok(RawToolOutcome::Success(
            "executed".into(),
        ))))
    }
}

fn tool_registry(counter: Arc<AtomicUsize>) -> ToolRegistry {
    let schema = serde_json::from_value::<ToolInputSchema>(serde_json::json!({
        "type": "object",
        "required": ["file_path"],
        "properties": {"file_path": {"type": "string"}}
    }))
    .unwrap();
    let secrets = serde_json::from_value::<SecretRedactionSpec>(
        serde_json::json!({"fields":[],"policy":"redact"}),
    )
    .unwrap();
    let definition = Arc::new(ToolDefinition::new(
        InternalToolName::new("Read".into()).unwrap(),
        ModelFacingToolName::new("Read".into()).unwrap(),
        schema,
        ToolDescriptionAsset::new(String::new()).unwrap(),
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Never,
        PermissionAction::new("read".into()).unwrap(),
        ToolTimeout::new(Duration::from_millis(
            EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u64,
        ))
        .unwrap(),
        ToolOutputLimit::new(
            TOOL_RESULT_BYTES_MAX.value,
            TOOL_RESULT_MODEL_CHARS_MAX.value,
        )
        .unwrap(),
        secrets,
    ));
    let registry = ToolRegistry::new([]).unwrap();
    registry
        .update(
            ToolsetId::None,
            &[ToolRegistration {
                definition,
                executor: Arc::new(CountingExecutor(counter)),
            }],
            None,
        )
        .unwrap();
    registry
}

fn valid_input() -> BoundedJsonValue {
    BoundedJsonValue::new(serde_json::json!({"file_path":"inside"})).unwrap()
}

fn sandbox_input(value: serde_json::Value) -> ValidatedToolInput {
    ValidatedToolInput::new(BoundedJsonValue::new(value).unwrap()).unwrap()
}

fn sandbox_check(
    gate: &WorkspaceSandboxGate<'_>,
    internal_name: &str,
    input: &ValidatedToolInput,
) -> Result<SandboxDecision, SandboxError> {
    gate.check(SandboxInvocation {
        internal_name,
        input,
    })
}

fn temp_root(label: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "lotta-sandbox-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir(&path).unwrap();
    path.canonicalize().unwrap()
}
