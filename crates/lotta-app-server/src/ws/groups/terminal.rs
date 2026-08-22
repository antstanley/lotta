//! WebSocket terminal command group.
//!
//! Decodes the pinned `terminal_spawn`, `terminal_input`, `terminal_resize`,
//! and `terminal_kill` commands and drives one real interactive shell session
//! per (connection, `terminal_id`) pair through
//! [`TerminalBridge`](crate::ws::terminal::TerminalBridge). Sessions are
//! scoped by connection: input, resize, and kill naming another connection's
//! `terminal_id` miss the local session map and stay silent no-ops. A
//! successful spawn emits `terminal_spawned`, session output streams as
//! `terminal_output`, and both natural exit and spawn failure emit
//! `terminal_exited`.
//!
//! To tolerate React Strict Mode mount cycles, a live session younger than
//! two seconds ([`STRICT_MODE_REUSE_WINDOW_MS`](crate::ws::terminal::STRICT_MODE_REUSE_WINDOW_MS))
//! is reused on repeat spawn (the pinned handler resends `terminal_spawned`)
//! and a kill inside that window is ignored. After the window a repeat spawn
//! terminates the old session before starting the replacement, matching the
//! pinned handler's kill-then-spawn. Every age comparison flows exclusively
//! through the injected [`Clock`](lotta_domain::Clock) port so the window
//! stays deterministic in tests. Killing or replacing a session removes it
//! from the map first, so — exactly as in the pinned baseline — only natural
//! exits and spawn failures produce a `terminal_exited` emission. Connection
//! cleanup cancels every session the connection owns through Task 38's
//! two-stage process-group termination, leaving no orphan process.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use lotta_domain::{Clock, Timestamp};
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    error::AppServerError, errors::ProtocolErrorEnvelope, framing::DecodedFrame,
    ws::connection::ConnectionId,
};

#[path = "terminal_session.rs"]
mod session;

/// React Strict Mode tolerance window for repeat spawns and kills.
///
/// Ages are compared through the injected [`Clock`] port; a live session
/// strictly younger than this window is reused on repeat spawn and protected
/// from kill.
pub const STRICT_MODE_REUSE_WINDOW_MS: i64 = 2_000;
/// Maximum `terminal_id` bytes accepted before the frame is rejected.
pub const TERMINAL_ID_BYTES_MAX: usize = 64;
/// Maximum `terminal_input` payload bytes accepted per command.
pub const TERMINAL_INPUT_BYTES_MAX: usize = 64 * 1024;
/// Exit code reported by `terminal_exited` when the shell could not spawn.
pub const SPAWN_FAILURE_EXIT_CODE: i32 = 1;

/// Pinned `terminal_spawn` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TerminalSpawnCommand {
    /// Caller-chosen terminal identity scoped to the sending connection.
    pub terminal_id: String,
    /// Requested terminal width; zero falls back to 80.
    pub cols: u16,
    /// Requested terminal height; zero falls back to 24.
    pub rows: u16,
    /// Optional working directory; absence uses the listener boot directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Pinned `terminal_input` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TerminalInputCommand {
    /// Terminal identity receiving the bytes.
    pub terminal_id: String,
    /// Raw input text forwarded to the session stdin.
    pub data: String,
}

/// Pinned `terminal_resize` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TerminalResizeCommand {
    /// Terminal identity being resized.
    pub terminal_id: String,
    /// Requested terminal width.
    pub cols: u16,
    /// Requested terminal height.
    pub rows: u16,
}

/// Pinned `terminal_kill` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TerminalKillCommand {
    /// Terminal identity to terminate.
    pub terminal_id: String,
}

/// The four concrete terminal group commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum TerminalCommand {
    /// Spawn one interactive shell session.
    #[serde(rename = "terminal_spawn")]
    Spawn(TerminalSpawnCommand),
    /// Forward raw input to one session.
    #[serde(rename = "terminal_input")]
    Input(TerminalInputCommand),
    /// Resize one live session.
    #[serde(rename = "terminal_resize")]
    Resize(TerminalResizeCommand),
    /// Terminate one session.
    #[serde(rename = "terminal_kill")]
    Kill(TerminalKillCommand),
}

/// The three outbound terminal group messages.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum TerminalMessage {
    /// Session became live, either fresh or reused inside the window.
    #[serde(rename = "terminal_spawned")]
    Spawned(TerminalSpawnedMessage),
    /// One output chunk of one session.
    #[serde(rename = "terminal_output")]
    Output(TerminalOutputMessage),
    /// Session finished naturally or failed to spawn.
    #[serde(rename = "terminal_exited")]
    Exited(TerminalExitedMessage),
}

/// Pinned `terminal_spawned` message.
#[derive(Clone, Debug, Serialize)]
pub struct TerminalSpawnedMessage {
    /// Terminal identity that became live.
    pub terminal_id: String,
    /// Child process identifier of the shell.
    pub pid: u32,
}

/// Pinned `terminal_output` message.
#[derive(Clone, Debug, Serialize)]
pub struct TerminalOutputMessage {
    /// Terminal identity producing output.
    pub terminal_id: String,
    /// Lossily decoded output chunk.
    pub data: String,
}

/// Pinned `terminal_exited` message.
#[derive(Clone, Debug, Serialize)]
pub struct TerminalExitedMessage {
    /// Terminal identity that finished.
    pub terminal_id: String,
    /// Process exit code, or [`SPAWN_FAILURE_EXIT_CODE`] on spawn failure.
    #[serde(rename = "exitCode")]
    pub exit_code: i32,
    /// Scrubbed failure detail; present only on spawn failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Push callback delivering one outbound message to one connection.
pub type TerminalForwarder =
    Arc<dyn Fn(ConnectionId, TerminalMessage) -> Result<(), AppServerError> + Send + Sync>;

type Sessions = HashMap<(ConnectionId, String), SessionRecord>;
type SharedSessions = Arc<Mutex<Sessions>>;

struct SessionRecord {
    pid: u32,
    spawned_at: Timestamp,
    control: session::SessionControl,
}

/// Applies wire terminal commands to per-connection interactive sessions.
pub struct TerminalBridge {
    clock: Arc<dyn Clock + Send + Sync>,
    default_cwd: PathBuf,
    shell: String,
    forward: TerminalForwarder,
    sessions: SharedSessions,
}

impl TerminalBridge {
    /// Creates a bridge probing the pinned baseline shell candidates.
    ///
    /// The working-directory fallback is the listener's boot directory.
    #[must_use]
    pub fn new(forward: TerminalForwarder, clock: Arc<dyn Clock + Send + Sync>) -> Self {
        Self::with_shell(forward, clock, boot_directory(), detect_shell())
    }

    /// Creates a bridge with an explicit shell and fallback directory.
    ///
    /// Explicit wiring keeps hosts without `$SHELL` deterministic.
    #[must_use]
    pub fn with_shell(
        forward: TerminalForwarder,
        clock: Arc<dyn Clock + Send + Sync>,
        default_cwd: PathBuf,
        shell: String,
    ) -> Self {
        Self {
            clock,
            default_cwd,
            shell,
            forward,
            sessions: Arc::new(Mutex::new(Sessions::new())),
        }
    }

    /// Spawns one session unless a live window-young session is reusable.
    ///
    /// A reused session resends `terminal_spawned` with the original pid and
    /// spawns nothing. Otherwise any leftover record is terminated first and
    /// a fresh real shell starts; spawn failure answers `terminal_exited`
    /// with [`SPAWN_FAILURE_EXIT_CODE`].
    pub fn spawn(&self, connection: ConnectionId, command: &TerminalSpawnCommand) {
        let key = (connection, command.terminal_id.clone());
        let now = self.clock.now();
        if let Some(pid) = self.reusable_pid(&key, now) {
            tracing::debug!(pid, "reusing recent terminal session");
            self.emit(connection, spawned_message(&command.terminal_id, pid));
            return;
        }
        self.terminate_key(&key);
        let request = session::LaunchRequest {
            shell: &self.shell,
            cwd: &resolve_cwd(&self.default_cwd, command.cwd.as_ref()),
        };
        match session::launch(
            request,
            self.output_sink(connection, &command.terminal_id),
            self.exit_sink(key.clone(), connection, command.terminal_id.clone()),
        ) {
            Ok(spawned) => {
                let pid = spawned.pid;
                self.insert_record(&key, now, spawned);
                self.emit(connection, spawned_message(&command.terminal_id, pid));
            }
            Err(error) => self.emit(connection, failure_message(&command.terminal_id, error)),
        }
    }

    /// Writes input into the named session; absent sessions stay silent.
    pub fn input(&self, connection: ConnectionId, command: &TerminalInputCommand) {
        let guard = lock(&self.sessions);
        if let Some(record) = guard.get(&(connection, command.terminal_id.clone())) {
            let _ignored = record.control.write(command.data.as_bytes());
        }
    }

    /// Records a resize against the named session.
    ///
    /// Pipe-backed sessions carry no kernel size, so the dimensions are only
    /// logged; absent sessions stay silent no-ops like the pinned baseline.
    pub fn resize(&self, connection: ConnectionId, command: &TerminalResizeCommand) {
        let guard = lock(&self.sessions);
        if guard.contains_key(&(connection, command.terminal_id.clone())) {
            tracing::debug!(cols = command.cols, rows = command.rows, "terminal resized");
        }
    }

    /// Terminates the named session unless it is window-young.
    ///
    /// The window guard tolerates React Strict Mode's rapid kill cycle. The
    /// map entry is removed synchronously, so the later exit callback finds
    /// no record and — matching the pinned baseline — emits nothing.
    pub fn kill(&self, connection: ConnectionId, command: &TerminalKillCommand) {
        let key = (connection, command.terminal_id.clone());
        let now = self.clock.now();
        if self.window_protected(&key, now) {
            tracing::debug!("ignoring kill for recently spawned terminal");
            return;
        }
        self.terminate_key(&key);
    }

    /// Cancels every session owned by the closing connection.
    pub fn disconnect(&self, connection: ConnectionId) {
        let removed: Vec<session::SessionControl> = {
            let mut guard = lock(&self.sessions);
            let keys: Vec<_> = guard
                .keys()
                .filter(|(owner, _)| *owner == connection)
                .cloned()
                .collect();
            keys.iter()
                .filter_map(|key| guard.remove(key))
                .map(|record| record.control)
                .collect()
        };
        for control in removed {
            control.cancel();
        }
    }

    fn insert_record(
        &self,
        key: &(ConnectionId, String),
        now: Timestamp,
        spawned: session::SpawnedSession,
    ) {
        lock(&self.sessions).insert(
            key.clone(),
            SessionRecord {
                pid: spawned.pid,
                spawned_at: now,
                control: spawned.control,
            },
        );
    }

    fn reusable_pid(&self, key: &(ConnectionId, String), now: Timestamp) -> Option<u32> {
        let guard = lock(&self.sessions);
        let record = guard.get(key)?;
        (within_window(now, record.spawned_at) && !record.control.has_exited())
            .then_some(record.pid)
    }

    fn window_protected(&self, key: &(ConnectionId, String), now: Timestamp) -> bool {
        lock(&self.sessions)
            .get(key)
            .is_some_and(|record| within_window(now, record.spawned_at))
    }

    fn terminate_key(&self, key: &(ConnectionId, String)) {
        if let Some(record) = lock(&self.sessions).remove(key) {
            record.control.cancel();
        }
    }

    fn emit(&self, connection: ConnectionId, message: TerminalMessage) {
        let _ignored = (self.forward)(connection, message);
    }

    fn output_sink(&self, connection: ConnectionId, terminal_id: &str) -> session::OutputSink {
        let forward = Arc::clone(&self.forward);
        let terminal_id = terminal_id.to_owned();
        Arc::new(move |data| {
            let _ignored = forward(
                connection,
                TerminalMessage::Output(TerminalOutputMessage {
                    terminal_id: terminal_id.clone(),
                    data,
                }),
            );
        })
    }

    fn exit_sink(
        &self,
        key: (ConnectionId, String),
        connection: ConnectionId,
        terminal_id: String,
    ) -> session::ExitSink {
        let sessions = Arc::clone(&self.sessions);
        let forward = Arc::clone(&self.forward);
        Box::new(move |pid, exit_code| {
            if take_matching_record(&sessions, &key, pid) {
                let _ignored = forward(
                    connection,
                    TerminalMessage::Exited(TerminalExitedMessage {
                        terminal_id,
                        exit_code,
                        error: None,
                    }),
                );
            }
        })
    }

    /// Returns how many sessions are currently retained.
    #[cfg(test)]
    pub(crate) fn sessions_len(&self) -> usize {
        lock(&self.sessions).len()
    }

    /// Returns the live pid recorded for one session, if any.
    #[cfg(test)]
    pub(crate) fn session_pid(&self, connection: ConnectionId, terminal_id: &str) -> Option<u32> {
        lock(&self.sessions)
            .get(&(connection, terminal_id.to_owned()))
            .map(|record| record.pid)
    }
}

fn take_matching_record(sessions: &SharedSessions, key: &(ConnectionId, String), pid: u32) -> bool {
    let mut guard = lock(sessions);
    matches!(guard.get(key), Some(record) if record.pid == pid) && guard.remove(key).is_some()
}

fn within_window(now: Timestamp, spawned_at: Timestamp) -> bool {
    let window = chrono::Duration::milliseconds(STRICT_MODE_REUSE_WINDOW_MS);
    spawned_at
        .checked_add(window)
        .is_ok_and(|deadline| now < deadline)
}

fn resolve_cwd(default_cwd: &Path, requested: Option<&String>) -> PathBuf {
    requested.map_or_else(|| default_cwd.to_path_buf(), PathBuf::from)
}

fn spawned_message(terminal_id: &str, pid: u32) -> TerminalMessage {
    TerminalMessage::Spawned(TerminalSpawnedMessage {
        terminal_id: terminal_id.to_owned(),
        pid,
    })
}

fn failure_message(terminal_id: &str, error: String) -> TerminalMessage {
    TerminalMessage::Exited(TerminalExitedMessage {
        terminal_id: terminal_id.to_owned(),
        exit_code: SPAWN_FAILURE_EXIT_CODE,
        error: Some(error),
    })
}

fn lock(sessions: &Mutex<Sessions>) -> MutexGuard<'_, Sessions> {
    sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn boot_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Probes the pinned baseline shell candidates in order.
#[must_use]
pub fn detect_shell() -> String {
    #[cfg(unix)]
    {
        let platform_default = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };
        let candidates = [
            std::env::var_os("SHELL").map(PathBuf::from),
            Some(PathBuf::from(platform_default)),
            Some(PathBuf::from("/bin/bash")),
        ];
        for path in candidates.into_iter().flatten() {
            if path.exists() {
                return path.to_string_lossy().into_owned();
            }
        }
        "/bin/sh".to_owned()
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("COMSPEC").map_or_else(
            || "cmd.exe".to_owned(),
            |value| value.to_string_lossy().into_owned(),
        )
    }
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known terminal commands,
/// including oversized `terminal_id` or `terminal_input` payloads.
pub fn decode(frame: &DecodedFrame) -> Result<Option<TerminalCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let parsed = match tag {
        Tag::TerminalSpawn => typed::<TerminalSpawnCommand>(frame).map(TerminalCommand::Spawn),
        Tag::TerminalInput => typed::<TerminalInputCommand>(frame).map(TerminalCommand::Input),
        Tag::TerminalResize => typed::<TerminalResizeCommand>(frame).map(TerminalCommand::Resize),
        Tag::TerminalKill => typed::<TerminalKillCommand>(frame).map(TerminalCommand::Kill),
        _ => return Ok(None),
    }?;
    validate(&parsed, frame.request_id.as_ref())?;
    Ok(Some(parsed))
}

fn typed<T: DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ProtocolErrorEnvelope> {
    serde_json::from_value(frame.value.clone()).map_err(|_| invalid(frame))
}

fn validate(
    command: &TerminalCommand,
    request_id: Option<&String>,
) -> Result<(), ProtocolErrorEnvelope> {
    let oversized = match command {
        TerminalCommand::Spawn(payload) => over_limit(&payload.terminal_id, None),
        TerminalCommand::Input(payload) => {
            over_limit(&payload.terminal_id, Some(payload.data.len()))
        }
        TerminalCommand::Resize(payload) => over_limit(&payload.terminal_id, None),
        TerminalCommand::Kill(payload) => over_limit(&payload.terminal_id, None),
    };
    if oversized {
        return Err(ProtocolErrorEnvelope::new(
            "terminal_command_invalid",
            "invalid terminal command",
            request_id.cloned(),
        ));
    }
    Ok(())
}

fn over_limit(terminal_id: &str, data_bytes: Option<usize>) -> bool {
    terminal_id.len() > TERMINAL_ID_BYTES_MAX
        || data_bytes.is_some_and(|bytes| bytes > TERMINAL_INPUT_BYTES_MAX)
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "terminal_command_invalid",
        "invalid terminal command",
        frame.request_id.clone(),
    )
}

#[cfg(test)]
#[path = "terminal_absent_session_noop_tests.rs"]
mod absent_session_noop;
#[cfg(test)]
#[path = "terminal_cleanup_tests.rs"]
mod cleanup;
#[cfg(test)]
#[path = "terminal_lifecycle_tests.rs"]
mod lifecycle;
#[cfg(test)]
#[path = "terminal_scoping_tests.rs"]
mod scoping;
#[cfg(test)]
#[path = "terminal_strict_mode_window_tests.rs"]
mod strict_mode_window;
#[cfg(test)]
#[path = "terminal_support.rs"]
mod support;
#[cfg(test)]
#[path = "terminal_wire_tests.rs"]
mod wire;
