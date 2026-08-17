use super::{
    command::{CommandHookConfig, CommandHookError, CommandHookExecutor, CommandHookType},
    events::{HookEvent, HookPayload},
    prompt::{
        PROMPT_HOOK_RESPONSE_BYTES_MAX, PromptHookConfig, PromptHookError, PromptHookExecutor,
        PromptHookType,
    },
};
use lotta_domain::{AgentId, ConversationId, RuntimeScope};
use lotta_runtime::{
    RuntimeError, WorkspaceSandbox,
    boundary::ProviderText,
    ports::{ModelCapabilityPort, ModelCapabilityRequest, ModelCapabilityResponse, PortFuture},
};
use lotta_tools::{OsSandbox, SandboxBackend, WorkspacePolicy};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct Fixture {
    isolation: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let isolation = unique_temp("timeout");
        let root = isolation.join("root");
        fs::create_dir_all(&root).unwrap();
        Self { isolation, root }
    }

    fn executor(&self) -> CommandHookExecutor {
        let policy = WorkspacePolicy::new(&WorkspaceSandbox::new(
            self.root.clone(),
            self.isolation.clone(),
        ))
        .unwrap();
        let sandbox = OsSandbox::detect(policy);
        assert_ne!(sandbox.backend(), &SandboxBackend::Unsupported);
        CommandHookExecutor::new(Arc::new(sandbox), scope(), self.root.clone()).unwrap()
    }

    fn script(&self, name: &str, body: &str) -> String {
        let path = self.root.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        path.to_str().unwrap().to_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.isolation).unwrap();
    }
}

fn unique_temp(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lotta-task44-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir(&path).unwrap();
    path
}

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("task44-agent").unwrap(),
        ConversationId::accept("task44-conversation").unwrap(),
        None,
    )
}

fn payload() -> HookPayload {
    HookPayload::new(HookEvent::Stop, json!({"event_type":"Stop"})).unwrap()
}

fn command(command: String, timeout: Option<u64>) -> CommandHookConfig {
    CommandHookConfig {
        kind: CommandHookType::Command,
        command,
        timeout,
        quiet: true,
    }
}

#[tokio::test]
async fn command_60000_below_at_above_clamp() {
    let fixture = Fixture::new();
    let program = fixture.script("allow", "printf '{\"ok\":true}'");
    for timeout in [Some(59_999), Some(60_000), Some(60_001), None] {
        fixture
            .executor()
            .execute(
                &command(program.clone(), timeout),
                &payload(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn command_timeout_and_cancellation_kill_and_join_process_group() {
    let fixture = Fixture::new();
    let ready_timeout = fixture.root.join("ready-timeout");
    fs::write(&ready_timeout, "ready").unwrap();
    let ready_cancel = fixture.root.join("ready-cancel");
    let timeout_pids = fixture.root.join("timeout-pids");
    fs::write(&timeout_pids, format!("{}\n", std::process::id())).unwrap();
    let timeout_script =
        fixture.script("fork-timeout", &forking_body(&ready_timeout, &timeout_pids));
    let cancel_script = fixture.script(
        "fork-cancel",
        &forking_body(&ready_cancel, &fixture.root.join("cancel-pids")),
    );

    assert_eq!(
        fixture
            .executor()
            .execute(
                &command(timeout_script, Some(10)),
                &payload(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err(),
        CommandHookError::Timeout
    );
    assert_pids_gone(&timeout_pids);

    let cancellation = CancellationToken::new();
    let child = cancellation.clone();
    let ready = ready_cancel.clone();
    let cancel_task = tokio::spawn(async move {
        wait_for_file(&ready).await;
        child.cancel();
    });
    assert_eq!(
        fixture
            .executor()
            .execute(&command(cancel_script, None), &payload(), cancellation)
            .await
            .unwrap_err(),
        CommandHookError::Cancelled
    );
    cancel_task.await.unwrap();
    assert_pids_gone(&fixture.root.join("cancel-pids"));
}

fn forking_body(ready: &Path, pids: &Path) -> String {
    format!(
        "sleep 600 &\ndesc=$!\nprintf '%s\\n%s\\n' $$ $desc > '{}'\n: > '{}'\nwait $desc",
        pids.display(),
        ready.display()
    )
}

async fn wait_for_file(path: &Path) {
    loop {
        match tokio::fs::metadata(path).await {
            Ok(_) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::task::yield_now().await;
            }
            Err(error) => panic!("ready marker failed: {error}"),
        }
    }
}

fn assert_pids_gone(path: &Path) {
    let values = fs::read_to_string(path).unwrap();
    for pid in values.lines() {
        if pid == std::process::id().to_string() {
            continue;
        }
        let status = std::process::Command::new("/bin/kill")
            .args(["-0", pid])
            .status()
            .unwrap();
        assert!(!status.success(), "pid {pid} survived executor completion");
    }
}

struct PendingModel {
    started: Notify,
    child: Mutex<Option<CancellationToken>>,
    request_timeouts: Mutex<Vec<Duration>>,
}

impl PendingModel {
    fn new() -> Self {
        Self {
            started: Notify::new(),
            child: Mutex::new(None),
            request_timeouts: Mutex::new(Vec::new()),
        }
    }
}

impl ModelCapabilityPort for PendingModel {
    fn generate(&self, request: ModelCapabilityRequest) -> PortFuture<'_, ModelCapabilityResponse> {
        self.request_timeouts.lock().unwrap().push(request.timeout);
        *self.child.lock().unwrap() = Some(request.cancellation.clone());
        self.started.notify_one();
        Box::pin(async move {
            request.cancellation.cancelled().await;
            Err(RuntimeError::Cancelled {
                context: "prompt hook certificate".into(),
            })
        })
    }
}

struct ResponseModel(String);

impl ModelCapabilityPort for ResponseModel {
    fn generate(&self, _: ModelCapabilityRequest) -> PortFuture<'_, ModelCapabilityResponse> {
        Box::pin(async move {
            Ok(ModelCapabilityResponse {
                content: ProviderText::new(self.0.clone()).unwrap(),
            })
        })
    }
}

fn prompt(timeout: Option<u64>) -> PromptHookConfig {
    PromptHookConfig {
        kind: PromptHookType::Prompt,
        prompt: "evaluate".into(),
        model: None,
        timeout,
        quiet: true,
    }
}

#[tokio::test]
async fn prompt_30000_below_at_above_and_raw_request_cannot_extend() {
    for (configured, expected) in [
        (Some(29_999), 29_999),
        (Some(30_000), 30_000),
        (Some(30_001), 30_000),
        (None, 30_000),
    ] {
        let model = Arc::new(PendingModel::new());
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = PromptHookExecutor::new(model.clone())
            .execute(
                HookEvent::Stop,
                &prompt(configured),
                &payload(),
                cancellation,
            )
            .await
            .unwrap_err();
        assert_eq!(error, PromptHookError::Cancelled);
        assert_eq!(
            *model.request_timeouts.lock().unwrap(),
            [Duration::from_millis(expected)]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn prompt_pending_timeout_is_typed_and_cancels_model_child() {
    let model = Arc::new(PendingModel::new());
    let executor = PromptHookExecutor::new(model.clone());
    let config = prompt(Some(1));
    let hook_payload = payload();
    let execute = executor.execute(
        HookEvent::Stop,
        &config,
        &hook_payload,
        CancellationToken::new(),
    );
    tokio::pin!(execute);
    assert!(matches!(
        futures_poll(&mut execute),
        std::task::Poll::Pending
    ));
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(execute.await.unwrap_err(), PromptHookError::Timeout);
    assert!(model.child.lock().unwrap().as_ref().unwrap().is_cancelled());
}

fn futures_poll<F: std::future::Future>(
    future: &mut std::pin::Pin<&mut F>,
) -> std::task::Poll<F::Output> {
    use std::task::{Context, Waker};
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    future.as_mut().poll(&mut context)
}

#[tokio::test]
async fn prompt_precancel_is_cancelled_and_cancels_model_child() {
    let model = Arc::new(PendingModel::new());
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        PromptHookExecutor::new(model.clone())
            .execute(HookEvent::Stop, &prompt(Some(1)), &payload(), cancellation)
            .await
            .unwrap_err(),
        PromptHookError::Cancelled
    );
    assert!(model.child.lock().unwrap().as_ref().unwrap().is_cancelled());
}

#[tokio::test]
async fn malformed_and_genuinely_overbound_model_responses_are_rejected() {
    for response in [
        "not-json".to_owned(),
        "x".repeat(PROMPT_HOOK_RESPONSE_BYTES_MAX + 1),
    ] {
        assert_eq!(
            PromptHookExecutor::new(Arc::new(ResponseModel(response)))
                .execute(
                    HookEvent::Stop,
                    &prompt(None),
                    &payload(),
                    CancellationToken::new(),
                )
                .await
                .unwrap_err(),
            PromptHookError::Response
        );
    }
}
