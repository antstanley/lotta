//! WebSocket skills command group.
//!
//! Decodes and routes the pinned `skill_enable` and `skill_disable` commands
//! (the §WebSocket command groups Skills row) against one global skills
//! directory rooted at `<storage>/.letta/skills`, mirroring the pinned
//! listener's symlink mechanics: enabling validates that the requested path is
//! an existing directory containing `SKILL.md` and links it under its base
//! name; disabling removes exactly that link and refuses to touch anything
//! that is not a symlink. Task 36 skill discovery still owns all discovery
//! itself — this group only selects among discovered skills by
//! maintaining the runtime's selected skill sources (the same selection Task 54
//! turn setup consumes as `SetupInput.selected_skills`) in step with the
//! on-disk links, so the next turn's prompt compilation sees the change.
//!
//! Every successful mutation emits a `skills_updated` snapshot after its
//! response, mirroring the pinned emit-after-response order. Failures answer
//! the pinned scrubbed texts with `success: false` and emit nothing.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use lotta_domain::Clock;
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    error::AppServerError, errors::ProtocolErrorEnvelope, framing::DecodedFrame,
    ws::connection::ConnectionId,
};

/// Name of the skill manifest every enabled directory must contain.
const SKILL_MANIFEST_NAME: &str = "SKILL.md";
/// Rejection detail for an absent or unusable enable path.
const ENABLE_PATH_MISSING: &str = "Path does not exist";
/// Rejection detail for a path without a manifest.
const ENABLE_MANIFEST_MISSING: &str = "No SKILL.md found in";
/// Rejection detail for a colliding non-symlink entry.
const ENABLE_LINK_CONFLICT: &str = "already exists and is not a symlink — refusing to overwrite";
/// Rejection detail for disabling an absent skill.
const DISABLE_NOT_FOUND: &str = "Skill not found:";
/// Rejection detail for disabling a non-symlink entry.
const DISABLE_NOT_SYMLINK: &str =
    "is not a symlink — refusing to delete. Remove it manually if intended.";
/// Catch-all rejection detail for failed enables.
const ENABLE_FAILURE: &str = "Failed to enable skill";
/// Catch-all rejection detail for failed disables.
const DISABLE_FAILURE: &str = "Failed to disable skill";

/// Pinned `skill_enable` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SkillEnableCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Absolute path to the skill directory on the local machine.
    pub skill_path: String,
}

/// Pinned `skill_disable` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SkillDisableCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Skill name, matching the link name inside the global skills directory.
    pub name: String,
}

/// The two concrete skills group commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum SkillsCommand {
    /// Links one directory into the global skills source.
    #[serde(rename = "skill_enable")]
    Enable(SkillEnableCommand),
    /// Removes one link from the global skills source.
    #[serde(rename = "skill_disable")]
    Disable(SkillDisableCommand),
}

/// Pinned `skill_enable_response` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkillEnableResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Enabled skill name, present only on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `skill_disable_response` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkillDisableResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Disabled skill name, present only on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `skills_updated` refresh notice.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkillsUpdatedMessage {
    /// Notice instant in epoch milliseconds.
    pub timestamp: i64,
}

/// The three outbound skills group messages.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum SkillsMessage {
    /// Enable result.
    #[serde(rename = "skill_enable_response")]
    EnableResponse(SkillEnableResponseMessage),
    /// Disable result.
    #[serde(rename = "skill_disable_response")]
    DisableResponse(SkillDisableResponseMessage),
    /// Refresh notice emitted after every successful mutation.
    #[serde(rename = "skills_updated")]
    Updated(SkillsUpdatedMessage),
}

/// Push callback delivering one outbound message to one connection.
pub type SkillsForwarder =
    Arc<dyn Fn(ConnectionId, SkillsMessage) -> Result<(), AppServerError> + Send + Sync>;

#[cfg(test)]
pub(crate) fn inert_forwarder() -> SkillsForwarder {
    Arc::new(|_, _| Ok(()))
}

/// Runtime-selected skill sources shared with Task 54 turn setup.
///
/// Holds the exact selected identifiers a subsequent turn feeds to
/// [`SetupInput.selected_skills`](lotta_runtime::turn::setup::SetupInput);
/// discovery itself stays owned by Task 36.
#[derive(Debug, Default)]
pub struct SkillSelection {
    ids: Vec<String>,
}

impl SkillSelection {
    /// Adds one identifier unless already selected.
    pub fn add(&mut self, id: &str) {
        if !self.ids.iter().any(|existing| existing == id) {
            self.ids.push(id.to_owned());
        }
    }

    /// Removes one identifier, reporting whether it was selected.
    pub fn remove(&mut self, id: &str) -> bool {
        let Some(position) = self.ids.iter().position(|existing| existing == id) else {
            return false;
        };
        self.ids.remove(position);
        true
    }

    /// Returns the selected identifiers in selection order.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.ids.clone()
    }
}

/// Applies wire skills commands to the global skills source and the shared
/// runtime selection consumed by Task 54 setup.
pub struct SkillsBridge {
    global_skills_dir: PathBuf,
    selected: Arc<Mutex<SkillSelection>>,
    clock: Arc<dyn Clock + Send + Sync>,
    forward: SkillsForwarder,
}

impl SkillsBridge {
    /// Creates the production bridge over one canonical storage root.
    ///
    /// The global skills directory lives at `<root>/.letta/skills`, mirroring
    /// the pinned `~/.letta/skills` layout under this server's storage root.
    #[must_use]
    pub fn new(
        forward: SkillsForwarder,
        storage_dir: &Path,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Self {
        Self {
            global_skills_dir: storage_dir.join(".letta").join("skills"),
            selected: Arc::new(Mutex::new(SkillSelection::default())),
            clock,
            forward,
        }
    }

    /// Returns the canonical global skills directory this bridge links into.
    #[must_use]
    pub fn global_skills_directory(&self) -> &Path {
        &self.global_skills_dir
    }

    /// Returns the shared handle holding the runtime's selected skill sources.
    #[must_use]
    pub fn selected_sources(&self) -> Arc<Mutex<SkillSelection>> {
        Arc::clone(&self.selected)
    }

    /// Routes one decoded command in a detached task, like the pinned listener.
    pub fn handle(self: &Arc<Self>, connection: ConnectionId, command: &SkillsCommand) {
        let this = Arc::clone(self);
        let command = command.clone();
        tokio::spawn(async move { this.apply(connection, &command) });
    }

    /// Applies one command inline, emitting responses through the forwarder.
    pub fn apply(&self, connection: ConnectionId, command: &SkillsCommand) {
        match command {
            SkillsCommand::Enable(payload) => self.enable(connection, payload),
            SkillsCommand::Disable(payload) => self.disable(connection, payload),
        }
    }

    fn enable(&self, connection: ConnectionId, command: &SkillEnableCommand) {
        let outcome = self.link_skill(Path::new(&command.skill_path));
        match outcome {
            Ok(name) => {
                if let Some(selected) = self.selected.lock().ok().as_mut() {
                    selected.add(&name);
                }
                self.emit(
                    connection,
                    SkillsMessage::EnableResponse(SkillEnableResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        skill_name: Some(name),
                        error: None,
                    }),
                );
                self.emit_snapshot(connection);
            }
            Err(message) => self.emit(
                connection,
                SkillsMessage::EnableResponse(SkillEnableResponseMessage {
                    request_id: command.request_id.clone(),
                    success: false,
                    skill_name: None,
                    error: Some(message.unwrap_or_else(|| ENABLE_FAILURE.to_owned())),
                }),
            ),
        }
    }

    fn disable(&self, connection: ConnectionId, command: &SkillDisableCommand) {
        let outcome = self.unlink_skill(&command.name);
        match outcome {
            Ok(()) => {
                if let Some(selected) = self.selected.lock().ok().as_mut() {
                    selected.remove(&command.name);
                }
                self.emit(
                    connection,
                    SkillsMessage::DisableResponse(SkillDisableResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        skill_name: Some(command.name.clone()),
                        error: None,
                    }),
                );
                self.emit_snapshot(connection);
            }
            Err(message) => self.emit(
                connection,
                SkillsMessage::DisableResponse(SkillDisableResponseMessage {
                    request_id: command.request_id.clone(),
                    success: false,
                    skill_name: None,
                    error: Some(message.unwrap_or_else(|| DISABLE_FAILURE.to_owned())),
                }),
            ),
        }
    }

    /// Validates one enable request and creates its global-source link.
    fn link_skill(&self, skill_path: &Path) -> Result<String, Option<String>> {
        if !skill_path.exists() {
            return Err(Some(format!(
                "{ENABLE_PATH_MISSING}: {}",
                skill_path.display()
            )));
        }
        if !skill_path.join(SKILL_MANIFEST_NAME).exists() {
            return Err(Some(format!(
                "{ENABLE_MANIFEST_MISSING} {}",
                skill_path.display()
            )));
        }
        let Some(name) = skill_path.file_name().and_then(std::ffi::OsStr::to_str) else {
            return Err(Some(format!(
                "{ENABLE_PATH_MISSING}: {}",
                skill_path.display()
            )));
        };
        std::fs::create_dir_all(&self.global_skills_dir)
            .map_err(|_| Some(format!("{ENABLE_FAILURE}: global skills directory")))?;
        let link_path = self.global_skills_dir.join(name);
        match std::fs::symlink_metadata(&link_path) {
            Ok(metadata) => {
                if !metadata.file_type().is_symlink() {
                    return Err(Some(format!(
                        "{} {ENABLE_LINK_CONFLICT}",
                        link_path.display()
                    )));
                }
                std::fs::remove_file(&link_path).map_err(|_| None)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(None),
        }
        create_link(skill_path, &link_path).map_err(|_| None)?;
        Ok(name.to_owned())
    }

    /// Removes exactly one symlinked entry from the global skills source.
    fn unlink_skill(&self, name: &str) -> Result<(), Option<String>> {
        let link_path = self.global_skills_dir.join(name);
        let metadata = match std::fs::symlink_metadata(&link_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Some(format!("{DISABLE_NOT_FOUND} {name}")));
            }
            Err(_) => return Err(None),
        };
        if !metadata.file_type().is_symlink() {
            return Err(Some(format!("{name} {DISABLE_NOT_SYMLINK}")));
        }
        std::fs::remove_file(&link_path).map_err(|_| None)?;
        Ok(())
    }

    fn emit_snapshot(&self, connection: ConnectionId) {
        self.emit(
            connection,
            SkillsMessage::Updated(SkillsUpdatedMessage {
                timestamp: self.clock.now().as_utc().timestamp_millis(),
            }),
        );
    }

    fn emit(&self, connection: ConnectionId, message: SkillsMessage) {
        let _ = (self.forward)(connection, message);
    }
}

/// Creates one directory symlink where the platform supports it.
#[cfg(unix)]
fn create_link(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Refuses enables on platforms without supported directory links.
#[cfg(not(unix))]
fn create_link(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "directory links unsupported",
    ))
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known skills commands,
/// including missing required fields or wrong-typed values.
pub fn decode(frame: &DecodedFrame) -> Result<Option<SkillsCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let parsed = match tag {
        Tag::SkillEnable => typed::<SkillEnableCommand>(frame).map(SkillsCommand::Enable),
        Tag::SkillDisable => typed::<SkillDisableCommand>(frame).map(SkillsCommand::Disable),
        _ => return Ok(None),
    }?;
    Ok(Some(parsed))
}

fn typed<T: DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ProtocolErrorEnvelope> {
    serde_json::from_value(frame.value.clone()).map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "skills_command_invalid",
        "invalid skills command",
        frame.request_id.clone(),
    )
}

#[cfg(test)]
#[path = "skills_commands_tests.rs"]
mod commands;
#[cfg(test)]
#[path = "skills_fixture_round_trip_tests.rs"]
mod fixture_round_trip;
