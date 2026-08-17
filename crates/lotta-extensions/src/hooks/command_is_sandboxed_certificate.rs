use super::{
    command::{CommandHookConfig, CommandHookError, CommandHookExecutor, CommandHookType},
    events::{HookEvent, HookOutcome, HookPayload},
};
use lotta_domain::{AgentId, ConversationId, RuntimeScope};
use lotta_runtime::WorkspaceSandbox;
use lotta_tools::{OsSandbox, SandboxBackend, WorkspacePolicy};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

const SECRET: &str = "task44-outside-marker-7d01343a";

struct Fixture {
    isolation: PathBuf,
    root: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let isolation = unique_temp("sandbox");
        let root = isolation.join("root");
        let outside = isolation.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(root.join("inside"), "{\"ok\":true}").unwrap();
        fs::write(outside.join("marker"), SECRET).unwrap();
        std::os::unix::fs::symlink(outside.join("marker"), root.join("escape-link")).unwrap();
        Self {
            isolation,
            root,
            outside,
        }
    }

    fn executor(&self) -> CommandHookExecutor {
        let policy = WorkspacePolicy::new(&WorkspaceSandbox::new(
            self.root.clone(),
            self.isolation.clone(),
        ))
        .unwrap();
        let sandbox = OsSandbox::detect(policy);
        assert_ne!(
            sandbox.backend(),
            &SandboxBackend::Unsupported,
            "current platform sandbox backend must run"
        );
        CommandHookExecutor::new(Arc::new(sandbox), scope(), self.root.clone()).unwrap()
    }

    fn fixture(&self, name: &str, target: &Path) -> String {
        let path = self.root.join(name);
        fs::write(
            &path,
            format!("#!/bin/sh\ncat -- '{}'\n", shell_quote(target)),
        )
        .unwrap();
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

fn shell_quote(path: &Path) -> String {
    path.to_str().unwrap().replace('\'', "'\\''")
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

fn config(command: String) -> CommandHookConfig {
    CommandHookConfig {
        kind: CommandHookType::Command,
        command,
        timeout: None,
        quiet: true,
    }
}

async fn denied(fixture: &Fixture, name: &str, target: &Path) {
    let command = fixture.fixture(name, target);
    let error = fixture
        .executor()
        .execute(&config(command), &payload(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CommandHookError::Exit | CommandHookError::Process | CommandHookError::Sandbox
    ));
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(SECRET));
    assert!(!rendered.contains("outside"));
}

#[tokio::test]
async fn inside_read_allowed_and_exact_allow_json() {
    let fixture = Fixture::new();
    let command = fixture.fixture("inside-hook", &fixture.root.join("inside"));
    let outcome = fixture
        .executor()
        .execute(&config(command), &payload(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(outcome, HookOutcome::Allow);
}

#[tokio::test]
async fn absolute_outside_read_is_denied_without_marker_disclosure() {
    let fixture = Fixture::new();
    denied(&fixture, "absolute-hook", &fixture.outside.join("marker")).await;
}

#[tokio::test]
async fn parent_escape_is_denied_without_marker_disclosure() {
    let fixture = Fixture::new();
    denied(&fixture, "parent-hook", Path::new("../outside/marker")).await;
}

#[tokio::test]
async fn symlink_escape_is_denied_without_marker_disclosure() {
    let fixture = Fixture::new();
    denied(&fixture, "symlink-hook", Path::new("escape-link")).await;
}
