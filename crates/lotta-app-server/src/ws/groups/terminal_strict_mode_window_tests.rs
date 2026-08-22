//! `ws::terminal::strict_mode_window` certificate selector: reuse inside two
//! seconds, fresh spawn after, and ignored kill inside — all through the
//! injected fake clock.

use super::STRICT_MODE_REUSE_WINDOW_MS;
use super::support::{
    CONNECTION_A, advance_ms, bridge, fake_clock, kill_command, spawn_command, spawned_pids,
    wait_pid_gone,
};

const TERMINAL: &str = "term";

#[tokio::test]
async fn reuses_within_two_seconds() {
    let clock = fake_clock();
    let (bridge, messages) = bridge(clock.clone());

    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));
    wait_until_first_spawn(&bridge, &messages).await;
    advance_ms(&clock, 1_999);

    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));

    assert_eq!(
        spawned_pids(&messages, TERMINAL).len(),
        2,
        "repeat spawn must still answer terminal_spawned"
    );
    let pids = spawned_pids(&messages, TERMINAL);
    assert_eq!(pids[0], pids[1], "window-young session must be reused");
    assert_eq!(bridge.sessions_len(), 1);

    bridge.disconnect(CONNECTION_A);
}

#[tokio::test]
async fn spawns_new_after_window() {
    let clock = fake_clock();
    let (bridge, messages) = bridge(clock.clone());

    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));
    let first = wait_until_first_spawn(&bridge, &messages).await;
    advance_ms(&clock, STRICT_MODE_REUSE_WINDOW_MS);

    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));

    let second = loop {
        let pids = spawned_pids(&messages, TERMINAL);
        if pids.len() == 2 {
            break pids[1];
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    };
    assert_ne!(
        first, second,
        "post-window repeat spawn must start a new session"
    );
    assert_eq!(bridge.sessions_len(), 1);
    wait_pid_gone(first).await;

    bridge.disconnect(CONNECTION_A);
    wait_pid_gone(second).await;
}

#[tokio::test]
async fn ignores_kill_within_window() {
    use super::support::pid_live;

    let clock = fake_clock();
    let (bridge, messages) = bridge(clock.clone());

    bridge.spawn(CONNECTION_A, &spawn_command(TERMINAL));
    let pid = wait_until_first_spawn(&bridge, &messages).await;

    bridge.kill(CONNECTION_A, &kill_command(TERMINAL));
    assert!(
        pid_live(pid),
        "window-young kill must not terminate the session"
    );

    advance_ms(&clock, STRICT_MODE_REUSE_WINDOW_MS);
    bridge.kill(CONNECTION_A, &kill_command(TERMINAL));

    assert_eq!(bridge.sessions_len(), 0);
    wait_pid_gone(pid).await;
}

async fn wait_until_first_spawn(
    bridge: &super::TerminalBridge,
    messages: &super::support::RecordedMessages,
) -> u32 {
    super::support::wait_spawned(bridge, messages, TERMINAL).await
}
