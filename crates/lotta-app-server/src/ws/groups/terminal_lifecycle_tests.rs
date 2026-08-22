//! `ws::terminal::lifecycle` certificate selector: spawned, output, exit, and
//! spawn-failure-emits-exited.

use super::support::{
    CONNECTION_A, bridge, exited_codes, fake_clock, input_command, output_chunks, spawn_command,
    spawn_command_in, wait_pid_gone, wait_spawned, wait_until,
};
use super::{SPAWN_FAILURE_EXIT_CODE, TerminalMessage};

const TERMINAL: &str = "term";
const OUTPUT_MARKER: &str = "lotta-terminal-lifecycle-ok";

#[tokio::test]
async fn spawn_emits_spawned_with_real_pid() {
    let (bridge, messages) = bridge(fake_clock());
    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));

    let pid = wait_spawned(&bridge, &messages, TERMINAL).await;
    assert!(pid > 0, "terminal_spawned must carry a real pid");
    assert_eq!(bridge.sessions_len(), 1);

    // Cleanup ignores the Strict-Mode window like the pinned connection close.
    bridge.disconnect(CONNECTION_A);
    assert_eq!(bridge.sessions_len(), 0);
    wait_pid_gone(pid).await;
}

#[tokio::test]
async fn input_produces_terminal_output() {
    let (bridge, messages) = bridge(fake_clock());
    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));
    let pid = wait_spawned(&bridge, &messages, TERMINAL).await;

    bridge.input(
        CONNECTION_A,
        &input_command(TERMINAL, &format!("echo {OUTPUT_MARKER}\n")),
    );
    wait_until(|| {
        output_chunks(&messages, TERMINAL)
            .iter()
            .any(|chunk| chunk.contains(OUTPUT_MARKER))
    })
    .await;

    bridge.disconnect(CONNECTION_A);
    wait_pid_gone(pid).await;
}

#[tokio::test]
async fn natural_exit_emits_terminal_exited() {
    let (bridge, messages) = bridge(fake_clock());
    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));
    wait_spawned(&bridge, &messages, TERMINAL).await;

    bridge.input(CONNECTION_A, &input_command(TERMINAL, "exit\n"));
    wait_until(|| !exited_codes(&messages, TERMINAL).is_empty()).await;
    assert_eq!(exited_codes(&messages, TERMINAL), [0]);
    wait_until(|| bridge.sessions_len() == 0).await;
}

#[tokio::test]
async fn spawn_failure_emits_terminal_exited() {
    let (bridge, messages) = bridge(fake_clock());
    let missing_cwd = "/definitely-missing-lotta-task-64-cwd";

    bridge.spawn(CONNECTION_A, &spawn_command_in(TERMINAL, missing_cwd));

    wait_until(|| !exited_codes(&messages, TERMINAL).is_empty()).await;
    assert_eq!(
        exited_codes(&messages, TERMINAL),
        [SPAWN_FAILURE_EXIT_CODE],
        "spawn failure must emit terminal_exited with the failure code"
    );
    let guard = messages.lock().expect("message lock");
    let failure = guard
        .iter()
        .find_map(|message| match message {
            TerminalMessage::Exited(payload) if payload.terminal_id == TERMINAL => {
                payload.error.clone()
            }
            _ => None,
        })
        .expect("spawn-failure exit carries scrubbed detail");
    drop(guard);
    assert!(!failure.is_empty());
    assert_eq!(bridge.sessions_len(), 0);
}
