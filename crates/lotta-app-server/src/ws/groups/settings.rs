//! WebSocket settings command group.
//!
//! Decodes and routes the pinned `get_cwd_map`, `get_reflection_settings`,
//! `set_reflection_settings`, `get_experiments`, and `set_experiment`
//! commands (the §WebSocket command groups Settings row: cwd map, reflection
//! settings, experiments). Every persisted value goes through the Task 27
//! side store at exactly the scope §State outside the backend root assigns:
//! global values (experiments, cwd map, agent reflection entries for the
//! `global` scope) live in `<home>/.letta/settings.json`, and project-local
//! reflection entries live in `<workspace>/.letta/settings.local.json` —
//! never a direct filesystem write, so atomic writes, bounds, confinement,
//! and CAS conflict behavior apply uniformly.
//!
//! Reflection settings are validated before any persistence: trigger, merge
//! mode, and scope are strict wire enums and step counts must be positive
//! integers, so invalid frames answer a correlated protocol error with zero
//! writes. The per-scope write set mirrors the pinned resolver: `local_project`
//! writes only the workspace-local file, `global` only the global file, and
//! the default/`both` scope writes both.
//!
//! Cwd changes arrive through
//! [`SettingsBridge::apply_cwd_change`](crate::ws::settings::SettingsBridge::apply_cwd_change)
//! (fed by
//! device-state handling) and apply to subsequent turns: the scope's cwd map
//! entry is what Task 54 turn setup resolves as its requested cwd. A missing
//! directory falls back to the boot working directory using Task 54's own
//! [`CwdResolution`](lotta_runtime::turn::setup::CwdResolution)
//! classification and records the original path as a
//! one-time reminder claim
//! ([`SettingsBridge::claim_missing_cwd_reminder`](crate::ws::settings::SettingsBridge::claim_missing_cwd_reminder)),
//! mirroring the pinned deleted-cwd reminder machinery.
//!
//! After every successful mutating command the bridge emits an
//! `update_device_status` snapshot carrying the settings fields this group
//! owns (experiments, cwd map, boot directory, written reflection entry),
//! mirroring the pinned emit-after-mutation semantics.
//!
//! Baseline degradations, kept honest: the cwd map persists inside the scoped
//! global settings document rather than the baseline's untracked
//! `remote-settings.json`, legacy numeric memory-reminder keys are neither
//! read nor written, project-scoped reads outside the explicitly authorized
//! workspace resolve to defaults because the side store's pure scope gate
//! rejects them, and env-backed experiment toggles resolve from this
//! process's environment.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::turn::setup::CwdResolution;
use lotta_store::{
    ProjectFile, SidePaths, StoreError, StoreErrorKind, WriteMode,
    side::{OpaqueFile, write_opaque_expected},
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    error::AppServerError, errors::ProtocolErrorEnvelope, framing::DecodedFrame,
    ws::connection::ConnectionId,
};

/// Maximum persisted cwd-map entries before further changes reject.
pub const CWD_MAP_ENTRIES_MAX: usize = 4_096;
/// Pinned default reflection step count.
pub const REFLECTION_STEP_COUNT_DEFAULT: u64 = 25;
/// Catch-all rejection detail for failed global-scope persistence.
const GLOBAL_FAILURE: &str = "Failed to update global settings";
/// Catch-all rejection detail for failed project-local persistence.
const LOCAL_FAILURE: &str = "Failed to update project local settings";
/// Rejection detail when the cwd map rejects one more entry.
const CWD_MAP_LIMIT_REACHED: &str = "cwd map limit reached";
/// Scope key prefix for non-default conversation overrides.
const CONVERSATION_KEY_PREFIX: &str = "conversation:";
/// Scope key suffix for an agent's default-conversation override.
const AGENT_DEFAULT_KEY_SUFFIX: &str = "::conversation:default";
/// Placeholder agent segment used when no agent is attributed.
const UNATTRIBUTED_AGENT: &str = "__unknown__";
/// Persisted per-agent reflection entry map inside both scoped documents,
/// using the pinned camelCase vocabulary (`settings-manager.ts`).
const REFLECTION_ENTRIES_KEY: &str = "reflectionSettingsByAgent";
/// Pinned flat reflection companions written alongside the per-agent entry.
const REFLECTION_TRIGGER_KEY: &str = "reflectionTrigger";
/// Pinned flat reflection companions written alongside the per-agent entry.
const REFLECTION_STEP_COUNT_KEY: &str = "reflectionStepCount";
/// Pinned flat reflection companions written alongside the per-agent entry.
const REFLECTION_MERGE_KEY: &str = "reflectionMerge";
/// Pinned flat reflection companions written alongside the per-agent entry.
const REFLECTION_MERGE_INSTRUCTIONS_KEY: &str = "reflectionMergeInstructions";
/// Persisted experiment override map inside the global document.
const EXPERIMENTS_KEY: &str = "experiments";
/// Persisted per-scope cwd override map inside the global document.
const CWD_MAP_KEY: &str = "cwd_map";

// ── Wire commands ───────────────────────────────────────────────────────────

/// Pinned `get_cwd_map` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GetCwdMapCommand {
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `get_reflection_settings` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GetReflectionSettingsCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Runtime whose effective reflection settings are read.
    pub runtime: lotta_domain::RuntimeScope,
}

/// Validated reflection body of one set command.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReflectionSettingsBody {
    /// When reflections fire.
    pub trigger: ReflectionTriggerMode,
    /// Positive step threshold for the step-count trigger.
    pub step_count: u64,
    /// Optional merge mode; older clients may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<ReflectionMergeMode>,
    /// Optional merge instructions; older clients may omit them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_instructions: Option<String>,
}

/// Persistence scope requested for one reflection write.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionScope {
    /// Workspace-local settings only.
    LocalProject,
    /// Global settings only.
    Global,
    /// Both workspace-local and global settings.
    Both,
}

/// Pinned `set_reflection_settings` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SetReflectionSettingsCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Runtime whose reflection settings are replaced.
    pub runtime: lotta_domain::RuntimeScope,
    /// Replacement settings body.
    pub settings: ReflectionSettingsBody,
    /// Requested persistence scope; absence means both scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ReflectionScope>,
}

/// Pinned `get_experiments` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GetExperimentsCommand {
    /// Response correlation identifier.
    pub request_id: String,
}

/// Known experiment identifiers accepted by the pinned manager.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentId {
    /// Desktop artifact tools.
    Artifacts,
    /// Automatic conversation titles.
    ConversationTitles,
    /// First-turn prior-conversation bootstrap.
    DesktopConversationBootstrap,
    /// Worktree diff previews.
    Diffs,
    /// Blind reflection-model comparisons.
    ReflectionArena,
    /// CLI-fired scheduled tasks.
    TuiCron,
}

/// Pinned `set_experiment` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SetExperimentCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Experiment to toggle.
    pub experiment_id: ExperimentId,
    /// Requested enabled state.
    pub enabled: bool,
}

/// The five concrete settings group commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum SettingsCommand {
    /// Current cwd overrides and boot directory.
    #[serde(rename = "get_cwd_map")]
    GetCwdMap(GetCwdMapCommand),
    /// Effective reflection settings for one runtime.
    #[serde(rename = "get_reflection_settings")]
    GetReflectionSettings(GetReflectionSettingsCommand),
    /// Validated reflection replacement across requested scopes.
    #[serde(rename = "set_reflection_settings")]
    SetReflectionSettings(SetReflectionSettingsCommand),
    /// Experiment listing.
    #[serde(rename = "get_experiments")]
    GetExperiments(GetExperimentsCommand),
    /// One experiment toggle.
    #[serde(rename = "set_experiment")]
    SetExperiment(SetExperimentCommand),
}

// ── Typed settings values ───────────────────────────────────────────────────

/// When reflections fire.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReflectionTriggerMode {
    /// Never fire.
    Off,
    /// Fire every configured step count.
    StepCount,
    /// Fire on compaction events.
    CompactionEvent,
}

/// How reflection merges run.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionMergeMode {
    /// Merge automatically.
    Auto,
    /// Merge only on explicit request.
    Explicit,
}

/// One effective reflection snapshot served on the wire.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReflectionSettingsSnapshot {
    /// Owning agent.
    pub agent_id: String,
    /// When reflections fire.
    pub trigger: ReflectionTriggerMode,
    /// Step threshold.
    pub step_count: u64,
    /// Merge policy.
    pub merge: ReflectionMergeMode,
    /// Merge instructions.
    pub merge_instructions: String,
}

/// Origin of one experiment's resolved state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentSource {
    /// Explicit persisted override.
    Override,
    /// Environment toggle.
    Env,
    /// Built-in default.
    Default,
}

/// One experiment definition plus its resolved state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExperimentSnapshot {
    /// Stable identifier.
    pub id: ExperimentId,
    /// Human-readable label.
    pub label: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Optional environment toggle name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<&'static str>,
    /// Resolved enabled state.
    pub enabled: bool,
    /// Where the enabled state came from.
    pub source: ExperimentSource,
    /// Explicit override value, present only under an override.
    #[serde(rename = "override")]
    pub r#override: Option<bool>,
}

/// One known experiment definition.
struct ExperimentDefinition {
    id: ExperimentId,
    label: &'static str,
    description: &'static str,
    env_var: Option<&'static str>,
}

/// Pinned experiment definitions in pinned order.
const EXPERIMENT_DEFINITIONS: [ExperimentDefinition; 6] = [
    ExperimentDefinition {
        id: ExperimentId::Artifacts,
        label: "artifacts",
        description: "Expose Letta Code Desktop artifact creation tools and artifact UI surfaces.",
        env_var: Some("LETTA_ARTIFACTS"),
    },
    // ponytail: no server-side title-settings store exists yet, so this
    // definition resolves through overrides/env/default like every other.
    ExperimentDefinition {
        id: ExperimentId::ConversationTitles,
        label: "conversation titles",
        description: "Generate AI conversation titles automatically when possible.",
        env_var: None,
    },
    ExperimentDefinition {
        id: ExperimentId::DesktopConversationBootstrap,
        label: "conversation bootstrap",
        description: "Inject lightweight prior-conversation context into the first turn of brand-new Letta Code conversations.",
        env_var: None,
    },
    ExperimentDefinition {
        id: ExperimentId::Diffs,
        label: "diffs",
        description: "Open browser-based worktree diff previews powered by Diffs from Pierre.",
        env_var: None,
    },
    ExperimentDefinition {
        id: ExperimentId::ReflectionArena,
        label: "reflection arena",
        description: "Run blind A/B comparisons between reflection models on the same transcript sample.",
        env_var: Some("LETTA_REFLECTION_ARENA"),
    },
    ExperimentDefinition {
        id: ExperimentId::TuiCron,
        label: "TUI cron scheduler",
        description: "Fire scheduled tasks from the CLI when the desktop app isn't running.",
        env_var: None,
    },
];

/// Environment toggle values treated as enabled.
const ENV_TOGGLE_VALUES: [&str; 3] = ["1", "true", "yes"];

// ── Wire responses ──────────────────────────────────────────────────────────

/// Pinned `get_cwd_map_response` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetCwdMapResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Persisted per-scope cwd overrides keyed by listener scope key.
    pub cwd_map: BTreeMap<String, String>,
    /// Boot working directory used when a scope has no override.
    pub boot_working_directory: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `get_reflection_settings_response` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetReflectionSettingsResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Effective snapshot, or null on failure.
    pub reflection_settings: Option<ReflectionSettingsSnapshot>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `set_reflection_settings_response` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SetReflectionSettingsResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Normalized scope that was written.
    pub scope: ReflectionScope,
    /// Written snapshot, or null on failure.
    pub reflection_settings: Option<ReflectionSettingsSnapshot>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `get_experiments_response` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetExperimentsResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// All known experiments with resolved state.
    pub experiments: Vec<ExperimentSnapshot>,
}

/// Pinned `set_experiment_response` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SetExperimentResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// All known experiments with resolved state after the change.
    pub experiments: Vec<ExperimentSnapshot>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Settings portion of the post-mutation device-status snapshot.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SettingsStatusSnapshot {
    /// Resolved experiments.
    pub experiments: Vec<ExperimentSnapshot>,
    /// Current per-scope cwd overrides.
    pub cwd_map: BTreeMap<String, String>,
    /// Boot working directory.
    pub boot_working_directory: String,
    /// Written reflection entry, present only after reflection writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflection_settings: Option<ReflectionSettingsSnapshot>,
}

/// The six outbound settings group messages.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum SettingsMessage {
    /// Cwd map result.
    #[serde(rename = "get_cwd_map_response")]
    CwdMapResponse(GetCwdMapResponseMessage),
    /// Reflection read result.
    #[serde(rename = "get_reflection_settings_response")]
    GetReflectionSettingsResponse(GetReflectionSettingsResponseMessage),
    /// Reflection write result.
    #[serde(rename = "set_reflection_settings_response")]
    SetReflectionSettingsResponse(SetReflectionSettingsResponseMessage),
    /// Experiments listing result.
    #[serde(rename = "get_experiments_response")]
    GetExperimentsResponse(GetExperimentsResponseMessage),
    /// Experiment toggle result.
    #[serde(rename = "set_experiment_response")]
    SetExperimentResponse(SetExperimentResponseMessage),
    /// Refresh snapshot emitted after every successful mutation.
    #[serde(rename = "update_device_status")]
    DeviceStatus {
        /// Settings-bearing device status body.
        device_status: SettingsStatusSnapshot,
    },
}

/// Push callback delivering one outbound message to one connection.
pub type SettingsForwarder =
    Arc<dyn Fn(ConnectionId, SettingsMessage) -> Result<(), AppServerError> + Send + Sync>;

#[cfg(test)]
pub(crate) fn inert_forwarder() -> SettingsForwarder {
    Arc::new(|_, _| Ok(()))
}

// ── Scoped cwd state ────────────────────────────────────────────────────────

/// One scoped cwd change awaiting application to subsequent turns.
#[derive(Clone, Debug)]
pub struct CwdChange {
    /// Optional owning agent identifier.
    pub agent_id: Option<String>,
    /// Conversation whose cwd changes.
    pub conversation_id: String,
    /// Requested new working directory.
    pub cwd: String,
}

/// Result of applying one scoped cwd change.
#[derive(Clone, Debug)]
pub struct CwdChangeOutcome {
    /// Task 54 classification of the requested directory.
    pub resolution: CwdResolution,
    /// Whether the change persisted through the side store.
    pub persisted: bool,
}

/// In-memory authoritative settings state mirrored through the side store.
#[derive(Debug)]
struct SettingsState {
    cwd_map: BTreeMap<String, String>,
    pending_reminders: BTreeMap<String, String>,
    boot_working_directory: PathBuf,
}

/// Applies wire settings commands to the Task 27 side store and the shared
/// cwd state consumed by Task 54 setup.
pub struct SettingsBridge {
    paths: Arc<SidePaths>,
    state: Mutex<SettingsState>,
    forward: SettingsForwarder,
}

impl SettingsBridge {
    /// Creates the production bridge over one canonical storage root and its
    /// explicitly authorized workspace root.
    ///
    /// The workspace root backs the project-local scope; other workspaces are
    /// rejected by the side store's pure scope gate, honoring §State outside
    /// the backend root.
    ///
    /// # Errors
    /// Returns [`AppServerError::Config`] when either root cannot back the
    /// canonical side-store layout.
    pub fn new(
        forward: SettingsForwarder,
        storage_dir: &Path,
        workspace_dir: &Path,
    ) -> Result<Self, AppServerError> {
        let paths = SidePaths::new(storage_dir, None, [workspace_dir.to_path_buf()])
            .map_err(|_| AppServerError::Config("settings store root invalid"))?;
        Ok(Self {
            paths: Arc::new(paths),
            state: Mutex::new(SettingsState {
                cwd_map: BTreeMap::new(),
                pending_reminders: BTreeMap::new(),
                boot_working_directory: workspace_dir.to_path_buf(),
            }),
            forward,
        })
    }

    /// Routes one decoded command in a detached task, like the pinned listener.
    pub fn handle(self: &Arc<Self>, connection: ConnectionId, command: &SettingsCommand) {
        let this = Arc::clone(self);
        let command = command.clone();
        tokio::spawn(async move { this.apply(connection, &command) });
    }

    /// Applies one command inline, emitting responses through the forwarder.
    pub fn apply(&self, connection: ConnectionId, command: &SettingsCommand) {
        match command {
            SettingsCommand::GetCwdMap(payload) => self.get_cwd_map(connection, payload),
            SettingsCommand::GetReflectionSettings(payload) => {
                self.get_reflection(connection, payload);
            }
            SettingsCommand::SetReflectionSettings(payload) => {
                self.set_reflection(connection, payload);
            }
            SettingsCommand::GetExperiments(payload) => self.get_experiments(connection, payload),
            SettingsCommand::SetExperiment(payload) => self.set_experiment(connection, payload),
        }
    }

    fn get_cwd_map(&self, connection: ConnectionId, command: &GetCwdMapCommand) {
        let state = self.lock();
        self.emit(
            connection,
            SettingsMessage::CwdMapResponse(GetCwdMapResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                cwd_map: state.cwd_map.clone(),
                boot_working_directory: Some(
                    state.boot_working_directory.to_string_lossy().into_owned(),
                ),
                error: None,
            }),
        );
    }

    fn get_reflection(&self, connection: ConnectionId, command: &GetReflectionSettingsCommand) {
        let agent_id = command.runtime.agent_id.as_str();
        let working_directory =
            self.working_directory_for(agent_id, command.runtime.conversation_id.as_str());
        let mut resolved = default_reflection(agent_id);
        apply_persisted_reflection(
            &mut resolved,
            read_global_document(&self.paths).ok().as_ref(),
            agent_id,
        );
        apply_persisted_reflection(
            &mut resolved,
            read_local_document(&self.paths, &working_directory)
                .ok()
                .as_ref(),
            agent_id,
        );
        self.emit(
            connection,
            SettingsMessage::GetReflectionSettingsResponse(GetReflectionSettingsResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                reflection_settings: Some(resolved),
                error: None,
            }),
        );
    }

    fn set_reflection(&self, connection: ConnectionId, command: &SetReflectionSettingsCommand) {
        let scope = command.scope.unwrap_or(ReflectionScope::Both);
        let agent_id = command.runtime.agent_id.as_str();
        let working_directory =
            self.working_directory_for(agent_id, command.runtime.conversation_id.as_str());
        let mut resolved = default_reflection(agent_id);
        apply_persisted_reflection(
            &mut resolved,
            read_global_document(&self.paths).ok().as_ref(),
            agent_id,
        );
        apply_persisted_reflection(
            &mut resolved,
            read_local_document(&self.paths, &working_directory)
                .ok()
                .as_ref(),
            agent_id,
        );
        if let Some(merge) = command.settings.merge {
            resolved.merge = merge;
        }
        if let Some(instructions) = command.settings.merge_instructions.as_deref() {
            instructions
                .trim()
                .clone_into(&mut resolved.merge_instructions);
        }
        resolved.trigger = command.settings.trigger;
        resolved.step_count = command.settings.step_count;
        match persist_reflection(&self.paths, &working_directory, agent_id, &resolved, scope) {
            Ok(()) => {
                self.emit(
                    connection,
                    SettingsMessage::SetReflectionSettingsResponse(
                        SetReflectionSettingsResponseMessage {
                            request_id: command.request_id.clone(),
                            success: true,
                            scope,
                            reflection_settings: Some(resolved.clone()),
                            error: None,
                        },
                    ),
                );
                self.emit_status(connection, Some(resolved));
            }
            Err(message) => self.emit(
                connection,
                SettingsMessage::SetReflectionSettingsResponse(
                    SetReflectionSettingsResponseMessage {
                        request_id: command.request_id.clone(),
                        success: false,
                        scope,
                        reflection_settings: None,
                        error: Some(message),
                    },
                ),
            ),
        }
    }

    fn get_experiments(&self, connection: ConnectionId, command: &GetExperimentsCommand) {
        self.emit(
            connection,
            SettingsMessage::GetExperimentsResponse(GetExperimentsResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                experiments: self.resolved_experiments(),
            }),
        );
    }

    fn set_experiment(&self, connection: ConnectionId, command: &SetExperimentCommand) {
        let outcome = match read_global_document(&self.paths) {
            Ok(mut document) => {
                set_experiment_override(&mut document, command.experiment_id, command.enabled);
                write_global_document(&self.paths, &document)
            }
            Err(message) => Err(message),
        };
        match outcome {
            Ok(()) => {
                self.emit(
                    connection,
                    SettingsMessage::SetExperimentResponse(SetExperimentResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        experiments: self.resolved_experiments(),
                        error: None,
                    }),
                );
                self.emit_status(connection, None);
            }
            Err(message) => self.emit(
                connection,
                SettingsMessage::SetExperimentResponse(SetExperimentResponseMessage {
                    request_id: command.request_id.clone(),
                    success: false,
                    experiments: self.resolved_experiments(),
                    error: Some(message.unwrap_or_else(|| GLOBAL_FAILURE.to_owned())),
                }),
            ),
        }
    }

    /// Applies one scoped cwd change to subsequent turns.
    ///
    /// The requested path persists into the scope's cwd map — exactly what
    /// Task 54 setup resolves as its requested cwd on the next turn. A missing
    /// directory classifies as Task 54's deleted fallback against the boot
    /// working directory and records the original path as a one-time reminder
    /// claim. Successful applications emit one device-status snapshot.
    ///
    /// # Errors
    /// Returns a scrubbed message when the cwd map rejects one more entry.
    pub fn apply_cwd_change(
        &self,
        connection: ConnectionId,
        change: &CwdChange,
    ) -> Result<CwdChangeOutcome, String> {
        let key = scope_key(change.agent_id.as_deref(), Some(&change.conversation_id));
        let boot = self.lock().boot_working_directory.clone();
        let resolution = classify_cwd(Path::new(&change.cwd), &boot);
        let original = match &resolution {
            CwdResolution::Requested(_) => None,
            CwdResolution::DeletedFallback { original, .. } => {
                Some(original.to_string_lossy().into_owned())
            }
        };
        {
            let mut state = self.lock();
            if state.cwd_map.len() >= CWD_MAP_ENTRIES_MAX && !state.cwd_map.contains_key(&key) {
                return Err(CWD_MAP_LIMIT_REACHED.to_owned());
            }
            state.cwd_map.insert(key.clone(), change.cwd.clone());
            match original {
                Some(original) => {
                    state.pending_reminders.insert(key.clone(), original);
                }
                None => {
                    state.pending_reminders.remove(&key);
                }
            }
        }
        let persisted = self.persist_cwd_entry(&key, &change.cwd);
        if persisted {
            self.emit_status(connection, None);
        }
        Ok(CwdChangeOutcome {
            resolution,
            persisted,
        })
    }

    /// Resolves the exact cwd classification a subsequent turn receives.
    ///
    /// This reuses Task 54's [`CwdResolution`] so callers feed
    /// [`CwdResolution::Requested`] paths straight into `SetupInput.cwd` while
    /// fallbacks carry the retained original path for its reminder claim.
    #[must_use]
    pub fn cwd_for_next_turn(
        &self,
        agent_id: Option<&str>,
        conversation_id: &str,
    ) -> CwdResolution {
        let (requested, boot) = {
            let state = self.lock();
            let key = scope_key(agent_id, Some(conversation_id));
            let requested = state
                .cwd_map
                .get(&key)
                .cloned()
                .unwrap_or_else(|| state.boot_working_directory.to_string_lossy().into_owned());
            (requested, state.boot_working_directory.clone())
        };
        classify_cwd(Path::new(&requested), &boot)
    }

    /// Returns the recorded scoped cwd override, if a device-state change
    /// stored one for this scope.
    #[must_use]
    pub fn cwd_override(&self, agent_id: Option<&str>, conversation_id: &str) -> Option<String> {
        let key = scope_key(agent_id, Some(conversation_id));
        self.lock().cwd_map.get(&key).cloned()
    }

    /// Claims a recorded missing-directory reminder once for this scope.
    ///
    /// Mirrors Task 54's durable one-use claim: the first call returns the
    /// originally requested path and later calls return nothing until another
    /// missing-directory change re-records it.
    #[must_use]
    pub fn claim_missing_cwd_reminder(
        &self,
        agent_id: Option<&str>,
        conversation_id: &str,
    ) -> Option<String> {
        let key = scope_key(agent_id, Some(conversation_id));
        self.lock().pending_reminders.remove(&key)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SettingsState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Persists one cwd-map entry into the global side-store document.
    fn persist_cwd_entry(&self, key: &str, cwd: &str) -> bool {
        match read_global_document(&self.paths) {
            Ok(mut document) => {
                document[CWD_MAP_KEY][key] = serde_json::Value::String(cwd.to_owned());
                write_global_document(&self.paths, &document).is_ok()
            }
            Err(_) => false,
        }
    }

    /// Working directory feeding project-scoped reflection reads and writes.
    fn working_directory_for(&self, agent_id: &str, conversation_id: &str) -> PathBuf {
        let state = self.lock();
        let key = scope_key(Some(agent_id), Some(conversation_id));
        state
            .cwd_map
            .get(&key)
            .cloned()
            .unwrap_or_else(|| state.boot_working_directory.to_string_lossy().into_owned())
            .into()
    }

    fn resolved_experiments(&self) -> Vec<ExperimentSnapshot> {
        let overrides = read_global_document(&self.paths)
            .map_or(serde_json::Value::Null, |document| {
                document[EXPERIMENTS_KEY].clone()
            });
        experiment_snapshots(&overrides)
    }

    fn emit_status(
        &self,
        connection: ConnectionId,
        reflection: Option<ReflectionSettingsSnapshot>,
    ) {
        let snapshot = {
            let state = self.lock();
            SettingsStatusSnapshot {
                experiments: self.resolved_experiments(),
                cwd_map: state.cwd_map.clone(),
                boot_working_directory: state.boot_working_directory.to_string_lossy().into_owned(),
                reflection_settings: reflection,
            }
        };
        self.emit(
            connection,
            SettingsMessage::DeviceStatus {
                device_status: snapshot,
            },
        );
    }

    fn emit(&self, connection: ConnectionId, message: SettingsMessage) {
        let _ = (self.forward)(connection, message);
    }
}

// ── Pure helpers ────────────────────────────────────────────────────────────

/// Formats the listener scope key for one cwd-map entry, mirroring the pinned
/// shapes `conversation:<id>` and `agent:<agent>::conversation:default`.
#[must_use]
pub fn scope_key(agent_id: Option<&str>, conversation_id: Option<&str>) -> String {
    let conversation = conversation_id.unwrap_or("default");
    if conversation == "default" {
        return format!(
            "agent:{}{AGENT_DEFAULT_KEY_SUFFIX}",
            agent_id.unwrap_or(UNATTRIBUTED_AGENT)
        );
    }
    format!("{CONVERSATION_KEY_PREFIX}{conversation}")
}

/// Classifies one requested cwd exactly like Task 54's resolution stage.
///
/// Existing directories serve as requested; anything else falls back to the
/// boot directory while retaining the original path for the reminder.
fn classify_cwd(requested: &Path, boot: &Path) -> CwdResolution {
    if requested.is_dir() {
        CwdResolution::Requested(requested.to_path_buf())
    } else {
        CwdResolution::DeletedFallback {
            original: requested.to_path_buf(),
            fallback: boot.to_path_buf(),
        }
    }
}

/// Pinned default reflection snapshot for one agent.
fn default_reflection(agent_id: &str) -> ReflectionSettingsSnapshot {
    ReflectionSettingsSnapshot {
        agent_id: agent_id.to_owned(),
        trigger: ReflectionTriggerMode::CompactionEvent,
        step_count: REFLECTION_STEP_COUNT_DEFAULT,
        merge: ReflectionMergeMode::Auto,
        merge_instructions: String::new(),
    }
}

/// Overlays one scoped document's persisted agent entry onto the resolution.
fn apply_persisted_reflection(
    resolved: &mut ReflectionSettingsSnapshot,
    document: Option<&serde_json::Value>,
    agent_id: &str,
) {
    let Some(entry) = document.map(|document| &document[REFLECTION_ENTRIES_KEY][agent_id]) else {
        return;
    };
    if let Ok(trigger) = serde_json::from_value::<ReflectionTriggerMode>(entry["trigger"].clone()) {
        resolved.trigger = trigger;
    }
    if let Some(step_count) = entry["stepCount"].as_u64().filter(|count| *count > 0) {
        resolved.step_count = step_count;
    }
    if let Ok(merge) = serde_json::from_value::<ReflectionMergeMode>(entry["merge"].clone()) {
        resolved.merge = merge;
    }
    if let Some(instructions) = entry["mergeInstructions"].as_str() {
        instructions.clone_into(&mut resolved.merge_instructions);
    }
}

/// Reads one scoped settings document, serving an empty object when absent.
fn read_document(
    document: Result<OpaqueFile, StoreError>,
) -> Result<serde_json::Value, Option<String>> {
    match document {
        Ok(file) => serde_json::from_slice(file.bytes())
            .map_err(|_| Some("failed to parse settings".to_owned())),
        Err(error) if error.kind() == StoreErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(_) => Err(None),
    }
}

/// Reads the global settings document through the Task 27 side store.
fn read_global_document(paths: &SidePaths) -> Result<serde_json::Value, Option<String>> {
    read_document(lotta_store::side::settings::read(paths))
}

/// Reads one project-local settings document through the side store.
fn read_local_document(
    paths: &SidePaths,
    workspace: &Path,
) -> Result<serde_json::Value, Option<String>> {
    read_document(lotta_store::side::project::read(
        paths,
        workspace,
        ProjectFile::LocalSettings,
    ))
}

/// Atomically replaces the global settings document with CAS protection.
fn write_global_document(
    paths: &SidePaths,
    document: &serde_json::Value,
) -> Result<(), Option<String>> {
    let bytes = serde_json::to_vec_pretty(document).map_err(|_| None)?;
    let outcome = match lotta_store::side::settings::read(paths) {
        Ok(source) => lotta_store::side::settings::write_expected(paths, &source, &bytes),
        Err(error) if error.kind() == StoreErrorKind::NotFound => {
            lotta_store::side::settings::write(paths, &bytes)
        }
        Err(_) => return Err(None),
    };
    outcome.map_err(|_| Some(GLOBAL_FAILURE.to_owned()))
}

/// Atomically replaces one project-local settings document with CAS.
fn write_local_document(
    paths: &SidePaths,
    workspace: &Path,
    document: &serde_json::Value,
) -> Result<(), Option<String>> {
    let bytes = serde_json::to_vec_pretty(document).map_err(|_| None)?;
    let path = paths
        .project_file(workspace, ProjectFile::LocalSettings)
        .map_err(|_| None)?;
    let outcome =
        match lotta_store::side::project::read(paths, workspace, ProjectFile::LocalSettings) {
            Ok(source) => {
                write_opaque_expected(&path, &bytes, source.revision(), WriteMode::Standard)
            }
            Err(error) if error.kind() == StoreErrorKind::NotFound => {
                lotta_store::side::project::write(
                    paths,
                    workspace,
                    ProjectFile::LocalSettings,
                    &bytes,
                )
            }
            Err(_) => return Err(None),
        };
    outcome.map_err(|_| Some(LOCAL_FAILURE.to_owned()))
}

/// Writes one agent reflection entry into every requested scope.
///
/// Each scope writes atomically through the Task 27 side store with CAS; any
/// failure answers one scrubbed rejection naming nothing but the scope kind.
fn persist_reflection(
    paths: &SidePaths,
    working_directory: &Path,
    agent_id: &str,
    entry: &ReflectionSettingsSnapshot,
    scope: ReflectionScope,
) -> Result<(), String> {
    let stored = serde_json::json!({
        "trigger": entry.trigger,
        "stepCount": entry.step_count,
        "merge": entry.merge,
        "mergeInstructions": entry.merge_instructions,
    });
    let mut failure = None;
    if matches!(scope, ReflectionScope::LocalProject | ReflectionScope::Both) {
        let outcome = upsert_agent_entry(
            read_local_document(paths, working_directory),
            |document| write_local_document(paths, working_directory, document),
            agent_id,
            &stored,
        );
        if let Err(message) = outcome {
            failure = Some(message.unwrap_or_else(|| LOCAL_FAILURE.to_owned()));
        }
    }
    if failure.is_none() && matches!(scope, ReflectionScope::Global | ReflectionScope::Both) {
        let outcome = upsert_agent_entry(
            read_global_document(paths),
            |document| write_global_document(paths, document),
            agent_id,
            &stored,
        );
        if let Err(message) = outcome {
            failure = Some(message.unwrap_or_else(|| GLOBAL_FAILURE.to_owned()));
        }
    }
    failure.map_or(Ok(()), Err)
}

/// Read-modify-writes one agent reflection entry into one scoped document.
///
/// Like the pinned `persistReflectionSettingsForAgent`, the same document also
/// receives the flat `reflection*` companions so readers without the per-agent
/// map observe the latest written values.
fn upsert_agent_entry(
    read: Result<serde_json::Value, Option<String>>,
    write: impl FnOnce(&serde_json::Value) -> Result<(), Option<String>>,
    agent_id: &str,
    entry: &serde_json::Value,
) -> Result<(), Option<String>> {
    let mut document = read?;
    document[REFLECTION_ENTRIES_KEY][agent_id] = entry.clone();
    document[REFLECTION_TRIGGER_KEY] = entry["trigger"].clone();
    document[REFLECTION_STEP_COUNT_KEY] = entry["stepCount"].clone();
    document[REFLECTION_MERGE_KEY] = entry["merge"].clone();
    document[REFLECTION_MERGE_INSTRUCTIONS_KEY] = entry["mergeInstructions"].clone();
    write(&document)
}

/// Sets one experiment override inside the global document.
fn set_experiment_override(document: &mut serde_json::Value, id: ExperimentId, enabled: bool) {
    document[EXPERIMENTS_KEY][experiment_discriminant(id)] = serde_json::Value::Bool(enabled);
}

/// Resolves every known experiment definition against persisted overrides.
fn experiment_snapshots(overrides: &serde_json::Value) -> Vec<ExperimentSnapshot> {
    EXPERIMENT_DEFINITIONS
        .iter()
        .map(|definition| {
            let r#override = overrides
                .get(experiment_discriminant(definition.id))
                .and_then(serde_json::Value::as_bool);
            let (enabled, source) = match r#override {
                Some(enabled) => (enabled, ExperimentSource::Override),
                None => env_resolution(definition.env_var),
            };
            ExperimentSnapshot {
                id: definition.id,
                label: definition.label,
                description: definition.description,
                env_var: definition.env_var,
                enabled,
                source,
                r#override,
            }
        })
        .collect()
}

/// Wire discriminant for one experiment identifier.
fn experiment_discriminant(id: ExperimentId) -> &'static str {
    match id {
        ExperimentId::Artifacts => "artifacts",
        ExperimentId::ConversationTitles => "conversation_titles",
        ExperimentId::DesktopConversationBootstrap => "desktop_conversation_bootstrap",
        ExperimentId::Diffs => "diffs",
        ExperimentId::ReflectionArena => "reflection_arena",
        ExperimentId::TuiCron => "tui_cron",
    }
}

/// Resolves environment-toggle defaults for one definition.
fn env_resolution(env_var: Option<&'static str>) -> (bool, ExperimentSource) {
    let enabled = env_var.is_some_and(|name| {
        std::env::var(name).is_ok_and(|value| {
            ENV_TOGGLE_VALUES.contains(&value.trim().to_ascii_lowercase().as_str())
        })
    });
    let source = if enabled {
        ExperimentSource::Env
    } else {
        ExperimentSource::Default
    };
    (enabled, source)
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// Reflection bodies validate strictly here — unknown enum values, negative
/// or fractional step counts, and zero thresholds all fail before any
/// persistence can run.
///
/// # Errors
/// Returns a correlated protocol error for malformed known settings commands.
pub fn decode(frame: &DecodedFrame) -> Result<Option<SettingsCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let parsed = match tag {
        Tag::GetCwdMap => typed::<GetCwdMapCommand>(frame).map(SettingsCommand::GetCwdMap),
        Tag::GetReflectionSettings => {
            typed::<GetReflectionSettingsCommand>(frame).map(SettingsCommand::GetReflectionSettings)
        }
        Tag::SetReflectionSettings => typed::<SetReflectionSettingsCommand>(frame)
            .and_then(validate_reflection_body)
            .map(SettingsCommand::SetReflectionSettings),
        Tag::GetExperiments => {
            typed::<GetExperimentsCommand>(frame).map(SettingsCommand::GetExperiments)
        }
        Tag::SetExperiment => {
            typed::<SetExperimentCommand>(frame).map(SettingsCommand::SetExperiment)
        }
        _ => return Ok(None),
    }?;
    Ok(Some(parsed))
}

/// Rejects out-of-range reflection bodies before persistence can run.
fn validate_reflection_body(
    command: SetReflectionSettingsCommand,
) -> Result<SetReflectionSettingsCommand, ProtocolErrorEnvelope> {
    if command.settings.step_count == 0 {
        return Err(invalid_body(&command.request_id));
    }
    Ok(command)
}

fn typed<T: DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ProtocolErrorEnvelope> {
    serde_json::from_value(frame.value.clone()).map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "settings_command_invalid",
        "invalid settings command",
        frame.request_id.clone(),
    )
}

fn invalid_body(request_id: &str) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "settings_command_invalid",
        "invalid settings command",
        Some(request_id.to_owned()),
    )
}

#[cfg(test)]
#[path = "settings_commands_tests.rs"]
mod commands;
#[cfg(test)]
#[path = "settings_cwd_change_tests.rs"]
mod cwd_change;
#[cfg(test)]
#[path = "settings_fixture_round_trip_tests.rs"]
mod fixture_round_trip;
#[cfg(test)]
#[path = "settings_reflection_validation_tests.rs"]
mod reflection_validation;
#[cfg(test)]
#[path = "settings_scopes_tests.rs"]
mod scopes;
#[cfg(test)]
#[path = "settings_support.rs"]
mod support;
