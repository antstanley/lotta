//! Shared fixtures for the terminal command-group certificate selectors.

use std::{sync::Arc, time::Duration};

use chrono::Duration as ChronoDuration;
use lotta_domain::Timestamp;
use lotta_testkit::clock::FakeClock;
use tokio::time::timeout;

use super::{
    TerminalBridge, TerminalForwarder, TerminalInputCommand, TerminalKillCommand, TerminalMessage,
    TerminalResizeCommand, TerminalSpawnCommand,
};
use crate::ws::ConnectionId;

/// Deterministic shell without startup-file interference.
pub(super) const SHELL: &str = "/bin/sh";
pub(super) const CONNECTION_A: ConnectionId = 7;
pub(super) const CONNECTION_B: ConnectionId = 8;
const POLL_DEADLINE_SECS: u64 = 15;

/// Recorded forwarded messages in emission order.
pub(super) type RecordedMessages = Arc<std::sync::Mutex<Vec<TerminalMessage>>>;

pub(super) fn fake_clock() -> Arc<FakeClock> {
    let stamp =
        Timestamp::parse_persisted_rfc3339("2026-08-14T12:00:00Z").expect("fixture timestamp");
    Arc::new(FakeClock::new(stamp))
}

pub(super) fn bridge(clock: Arc<FakeClock>) -> (TerminalBridge, RecordedMessages) {
    bridge_with_shell(clock, SHELL.to_owned())
}

pub(super) fn bridge_with_shell(
    clock: Arc<FakeClock>,
    shell: String,
) -> (TerminalBridge, RecordedMessages) {
    let messages: RecordedMessages = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: TerminalForwarder = Arc::new(move |_, message| {
        sink.lock().expect("message lock").push(message);
        Ok(())
    });
    let cwd = std::env::temp_dir();
    (
        TerminalBridge::with_shell(forward, clock, cwd, shell),
        messages,
    )
}

pub(super) fn spawn_command(terminal_id: &str) -> TerminalSpawnCommand {
    TerminalSpawnCommand {
        terminal_id: terminal_id.to_owned(),
        cols: 80,
        rows: 24,
        cwd: None,
    }
}

pub(super) fn spawn_command_in(terminal_id: &str, cwd: &str) -> TerminalSpawnCommand {
    TerminalSpawnCommand {
        terminal_id: terminal_id.to_owned(),
        cols: 80,
        rows: 24,
        cwd: Some(cwd.to_owned()),
    }
}

pub(super) fn input_command(terminal_id: &str, data: &str) -> TerminalInputCommand {
    TerminalInputCommand {
        terminal_id: terminal_id.to_owned(),
        data: data.to_owned(),
    }
}

pub(super) fn resize_command(terminal_id: &str, cols: u16, rows: u16) -> TerminalResizeCommand {
    TerminalResizeCommand {
        terminal_id: terminal_id.to_owned(),
        cols,
        rows,
    }
}

pub(super) fn kill_command(terminal_id: &str) -> TerminalKillCommand {
    TerminalKillCommand {
        terminal_id: terminal_id.to_owned(),
    }
}

pub(super) fn kind_of(message: &TerminalMessage) -> &'static str {
    match message {
        TerminalMessage::Spawned(_) => "terminal_spawned",
        TerminalMessage::Output(_) => "terminal_output",
        TerminalMessage::Exited(_) => "terminal_exited",
    }
}

fn matches_terminal_id(message: &TerminalMessage, terminal_id: &str) -> bool {
    match message {
        TerminalMessage::Spawned(payload) => payload.terminal_id == terminal_id,
        TerminalMessage::Output(payload) => payload.terminal_id == terminal_id,
        TerminalMessage::Exited(payload) => payload.terminal_id == terminal_id,
    }
}

pub(super) fn spawned_pids(messages: &RecordedMessages, terminal_id: &str) -> Vec<u32> {
    guard(messages)
        .iter()
        .filter_map(|message| match message {
            TerminalMessage::Spawned(payload) if matches_terminal_id(message, terminal_id) => {
                Some(payload.pid)
            }
            _ => None,
        })
        .collect()
}

pub(super) fn output_chunks(messages: &RecordedMessages, terminal_id: &str) -> Vec<String> {
    guard(messages)
        .iter()
        .filter_map(|message| match message {
            TerminalMessage::Output(payload) if matches_terminal_id(message, terminal_id) => {
                Some(payload.data.clone())
            }
            _ => None,
        })
        .collect()
}

pub(super) fn exited_codes(messages: &RecordedMessages, terminal_id: &str) -> Vec<i32> {
    guard(messages)
        .iter()
        .filter_map(|message| match message {
            TerminalMessage::Exited(payload) if matches_terminal_id(message, terminal_id) => {
                Some(payload.exit_code)
            }
            _ => None,
        })
        .collect()
}

pub(super) fn count_kind(messages: &RecordedMessages, kind: &'static str) -> usize {
    guard(messages)
        .iter()
        .filter(|m| kind_of(m) == kind)
        .count()
}

pub(super) async fn wait_until(condition: impl Fn() -> bool) {
    timeout(Duration::from_secs(POLL_DEADLINE_SECS), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("condition within poll deadline");
}

#[cfg(unix)]
pub(super) fn pid_live(pid: u32) -> bool {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,state="])
        .output()
        .expect("inspect process state");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_row)
        .any(|(row_pid, state)| row_pid == pid && state != 'Z')
}

#[cfg(unix)]
pub(super) fn group_live(group: u32) -> bool {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pgid=,state="])
        .output()
        .expect("inspect process groups");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_row)
        .any(|(row_group, state)| row_group == group && state != 'Z')
}

#[cfg(unix)]
fn parse_row(value: &str) -> Option<(u32, char)> {
    let mut fields = value.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let state = fields.next()?.chars().next()?;
    Some((pid, state))
}

pub(super) async fn wait_pid_gone(pid: u32) {
    wait_until(|| !pid_live(pid)).await;
}

pub(super) async fn wait_spawned(
    bridge: &TerminalBridge,
    messages: &RecordedMessages,
    terminal_id: &str,
) -> u32 {
    wait_until(|| !spawned_pids(messages, terminal_id).is_empty()).await;
    let pid = spawned_pids(messages, terminal_id)[0];
    wait_until(|| bridge.session_pid(CONNECTION_A, terminal_id) == Some(pid)).await;
    pid
}

pub(super) fn advance_ms(clock: &FakeClock, millis: i64) {
    clock
        .advance(ChronoDuration::milliseconds(millis))
        .expect("clock advance within range");
}

fn guard(messages: &RecordedMessages) -> std::sync::MutexGuard<'_, Vec<TerminalMessage>> {
    messages.lock().expect("message lock")
}
