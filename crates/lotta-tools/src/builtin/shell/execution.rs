use super::test_support::{Fixture, assert_complete, execute_bundle, run, success_text};
use crate::{
    ToolsetId,
    sandbox::{OsSandbox, SandboxBackend, WorkspacePolicy},
};
use lotta_runtime::{WorkspaceSandbox, ports::ToolOutcome};
use serde_json::json;
use std::{fs, sync::Arc};

fn sandbox(fixture: &Fixture) -> Arc<OsSandbox> {
    let policy = WorkspacePolicy::new(&WorkspaceSandbox::new(
        fixture.workspace.clone(),
        fixture.root.clone(),
    ))
    .unwrap();
    Arc::new(OsSandbox::detect(policy))
}

fn supported(sandbox: &OsSandbox) -> bool {
    !matches!(sandbox.backend(), SandboxBackend::Unsupported)
}

async fn success(
    fixture: &Fixture,
    sandbox: Arc<OsSandbox>,
    toolset: ToolsetId,
    name: &str,
    input: serde_json::Value,
) -> (ToolOutcome, super::ShellToolBundle) {
    let bundle =
        super::ShellToolBundle::new(&fixture.workspace, super::test_support::scope(), sandbox)
            .unwrap();
    let (outcome, bundle) = success_with_bundle(fixture, bundle, toolset, name, input).await;
    (outcome, bundle)
}

async fn success_with_bundle(
    fixture: &Fixture,
    bundle: super::ShellToolBundle,
    toolset: ToolsetId,
    name: &str,
    input: serde_json::Value,
) -> (ToolOutcome, super::ShellToolBundle) {
    let (result, records) = execute_bundle(
        fixture,
        &bundle,
        toolset,
        name,
        input,
        tokio_util::sync::CancellationToken::new(),
    )
    .await;
    let outcome = result.unwrap_or_else(|error| {
        panic!(
            "{name} failed at {:?}: {error:?}",
            *records.trace.lock().unwrap()
        )
    });
    assert_complete(&records, &outcome);
    (outcome, bundle)
}

#[tokio::test]
async fn bash_foreground_success_and_nonzero() {
    let fixture = Fixture::new("bash-foreground");
    let sandbox = sandbox(&fixture);
    if !supported(&sandbox) {
        return;
    }
    let (ok, bundle) = success(
        &fixture,
        Arc::clone(&sandbox),
        ToolsetId::Default,
        "Bash",
        json!({"command":"printf ponytail","description":"success"}),
    )
    .await;
    assert_eq!(success_text(&ok), "ponytail");
    let (failed, _) = success(
        &fixture,
        sandbox,
        ToolsetId::Default,
        "Bash",
        json!({"command":"printf failure; exit 7","description":"nonzero"}),
    )
    .await;
    assert_eq!(success_text(&failed), "Exit code: 7\nfailure");
    bundle.shutdown().await.unwrap();
}

#[tokio::test]
async fn bash_background_task_output_running_then_completion() {
    let fixture = Fixture::new("background");
    let sandbox = sandbox(&fixture);
    if !supported(&sandbox) {
        return;
    }
    let bundle =
        super::ShellToolBundle::new(&fixture.workspace, super::test_support::scope(), sandbox)
            .unwrap();
    let (started, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Default,
        "Bash",
        json!({
            "command":"while :; do :; done",
            "description":"background",
            "run_in_background":true
        }),
    )
    .await;
    let id = success_text(&started).split(": ").last().unwrap();
    let (running, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Default,
        "TaskOutput",
        json!({"task_id":id,"block":false,"timeout":0}),
    )
    .await;
    assert_eq!(
        success_text(&running),
        concat!(
            r#"{"exit_code":null,"message":"Task is still running...","#,
            r#""status":"running","terminal_reason":null}"#
        )
    );
    let (stopped, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Default,
        "TaskStop",
        json!({"task_id":id}),
    )
    .await;
    assert_eq!(success_text(&stopped), "{\"killed\":true}");
    let (completed, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Default,
        "TaskOutput",
        json!({"task_id":id,"block":true,"timeout":5000}),
    )
    .await;
    assert_eq!(
        success_text(&completed),
        concat!(
            r#"{"exit_code":null,"message":"(no output yet)","#,
            r#""status":"stopped","terminal_reason":"stopped"}"#
        )
    );
    bundle.shutdown().await.unwrap();
}

#[tokio::test]
async fn gemini_alias_real_one_shot() {
    let fixture = Fixture::new("gemini");
    let sandbox = sandbox(&fixture);
    if !supported(&sandbox) {
        return;
    }
    let (outcome, bundle) = success(
        &fixture,
        sandbox,
        ToolsetId::Gemini,
        "RunShellCommand",
        json!({"command":"printf gemini","description":"alias","dir_path":"."}),
    )
    .await;
    assert_eq!(success_text(&outcome), "gemini");
    bundle.shutdown().await.unwrap();
}

#[tokio::test]
async fn codex_exec_non_pty_completes_within_yield() {
    let fixture = Fixture::new("exec");
    let sandbox = sandbox(&fixture);
    if !supported(&sandbox) {
        return;
    }
    let (outcome, bundle) = success(
        &fixture,
        sandbox,
        ToolsetId::Codex,
        "exec_command",
        json!({"cmd":"printf codex","description":"exec","yield_time_ms":30_000}),
    )
    .await;
    let text = success_text(&outcome);
    assert!(text.contains("Process exited with code 0"));
    assert!(text.ends_with("Output:\ncodex"));
    bundle.shutdown().await.unwrap();
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn real_pty_exec_write_stdin_reads_unread_output() {
    let fixture = Fixture::new("pty");
    let sandbox = sandbox(&fixture);
    if !supported(&sandbox) {
        return;
    }
    let bundle =
        super::ShellToolBundle::new(&fixture.workspace, super::test_support::scope(), sandbox)
            .unwrap();
    let (started, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Codex,
        "exec_command",
        json!({
            "cmd":"printf READY; read value; printf VALUE:$value",
            "description":"pty", "tty":true, "yield_time_ms":250
        }),
    )
    .await;
    assert!(success_text(&started).contains("Process running with session ID 1"));
    let (read, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Codex,
        "write_stdin",
        json!({"session_id":1,"chars":"ponytail\n","yield_time_ms":250}),
    )
    .await;
    assert!(success_text(&read).contains("VALUE:ponytail"));
    bundle.shutdown().await.unwrap();
}

#[tokio::test]
async fn monitor_starts_and_output_is_readable() {
    let fixture = Fixture::new("monitor");
    let sandbox = sandbox(&fixture);
    if !supported(&sandbox) {
        return;
    }
    let bundle =
        super::ShellToolBundle::new(&fixture.workspace, super::test_support::scope(), sandbox)
            .unwrap();
    let input = json!({
        "command": "printf event; while :; do :; done",
        "description": "monitor",
        "timeout_ms": 5000
    });
    let (started, bundle) =
        success_with_bundle(&fixture, bundle, ToolsetId::Default, "Monitor", input).await;
    let id = success_text(&started)
        .split("task ")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap();
    let (output, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Default,
        "TaskOutput",
        json!({"task_id":id,"block":false,"timeout":0}),
    )
    .await;
    assert_eq!(
        success_text(&output),
        concat!(
            r#"{"exit_code":null,"message":"Task is still running...","#,
            r#""status":"running","terminal_reason":null}"#
        )
    );
    bundle.shutdown().await.unwrap();
}

#[tokio::test]
async fn task_stop_running_terminal_and_unknown() {
    let fixture = Fixture::new("stop");
    let sandbox = sandbox(&fixture);
    if !supported(&sandbox) {
        return;
    }
    let bundle =
        super::ShellToolBundle::new(&fixture.workspace, super::test_support::scope(), sandbox)
            .unwrap();
    let (started, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Default,
        "Bash",
        json!({"command":"read value","description":"running","run_in_background":true}),
    )
    .await;
    let id = success_text(&started).split(": ").last().unwrap();
    let (first, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Default,
        "TaskStop",
        json!({"task_id":id}),
    )
    .await;
    assert_eq!(success_text(&first), "{\"killed\":true}");
    let (terminal, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Default,
        "TaskStop",
        json!({"task_id":id}),
    )
    .await;
    assert_eq!(success_text(&terminal), "{\"killed\":false}");
    let (unknown, bundle) = success_with_bundle(
        &fixture,
        bundle,
        ToolsetId::Default,
        "TaskStop",
        json!({"task_id":"unknown"}),
    )
    .await;
    assert_eq!(success_text(&unknown), "{\"killed\":false}");
    bundle.shutdown().await.unwrap();
}

#[tokio::test]
async fn cwd_traversal_absolute_peer_and_symlink_are_denied() {
    let fixture = Fixture::new("cwd-denial");
    fixture.write_peer("secret", "sealed");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&fixture.peer, fixture.workspace.join("link")).unwrap();
    let sandbox = sandbox(&fixture);
    for cwd in [
        "../peer".to_owned(),
        fixture.peer.to_string_lossy().into_owned(),
        "link".to_owned(),
    ] {
        let (result, records, _) = run(
            &fixture,
            sandbox.clone(),
            ToolsetId::GeminiSnake,
            "run_shell_command",
            json!({"command":"cat secret","description":"denied","dir_path":cwd}),
        )
        .await;
        let outcome = result.expect("schema-valid confinement request reaches executor");
        assert!(matches!(
            outcome,
            ToolOutcome::SpawnFailure { .. } | ToolOutcome::ToolDefinedError { .. }
        ));
        assert_eq!(records.trace.lock().unwrap().len(), 12);
        assert_eq!(records.persisted.lock().unwrap().len(), 1);
    }
    assert_eq!(
        fs::read_to_string(fixture.peer.join("secret")).unwrap(),
        "sealed"
    );
}

#[tokio::test]
async fn os_sandbox_cannot_read_or_write_peer() {
    let fixture = Fixture::new("sandbox-peer");
    fixture.write_peer("secret", "sealed");
    let sandbox = sandbox(&fixture);
    if !supported(&sandbox) {
        return;
    }
    let command = format!(
        "cat '{}' 2>/dev/null || true; printf altered > '{}' 2>/dev/null || true",
        fixture.peer.join("secret").display(),
        fixture.peer.join("secret").display()
    );
    let (outcome, bundle) = success(
        &fixture,
        sandbox,
        ToolsetId::Default,
        "Bash",
        json!({"command":command,"description":"confinement"}),
    )
    .await;
    assert!(!success_text(&outcome).contains("sealed"));
    assert_eq!(
        fs::read_to_string(fixture.peer.join("secret")).unwrap(),
        "sealed"
    );
    bundle.shutdown().await.unwrap();
}
