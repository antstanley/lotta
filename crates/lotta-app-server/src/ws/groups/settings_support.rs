//! Shared fixtures for the settings command-group certificate selectors.
//!
//! Every fixture drives the real bridge over one unique temporary side-store
//! layout: a HOME-like storage root and an explicitly authorized workspace
//! root, both seeded and asserted through the same
//! [`SidePaths`](lotta_store::SidePaths) scopes the bridge resolves.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use lotta_domain::RuntimeScope;
use lotta_store::SidePaths;
use serde_json::Value;

use super::{SettingsBridge, SettingsForwarder, SettingsMessage};
use crate::{framing, ws::ConnectionId};

/// First test connection identity.
pub(super) const CONNECTION_A: ConnectionId = 41;

type RecordedMessages = Arc<Mutex<Vec<(ConnectionId, SettingsMessage)>>>;

static ROOT_ORDINAL: AtomicUsize = AtomicUsize::new(0);

/// One bridge over a unique temporary storage layout plus its recorder.
pub(super) struct TestSettings {
    /// Bridge under test; every command applies inline via [`Self::send`].
    pub(super) bridge: Arc<SettingsBridge>,
    /// Side paths mirroring the bridge's internal layout.
    pub(super) paths: SidePaths,
    /// Storage root playing this server's HOME.
    pub(super) home: std::path::PathBuf,
    /// Explicitly authorized workspace root for the project-local scope.
    pub(super) workspace: std::path::PathBuf,
    messages: RecordedMessages,
}

impl TestSettings {
    /// Decodes a raw JSON command through framing and applies it inline.
    ///
    /// Returns the protocol error when decoding rejects the frame instead of
    /// routing it, mirroring the listener's decode-chain behavior.
    pub(super) fn send(&self, command: &Value) -> Result<(), crate::errors::ProtocolErrorEnvelope> {
        let text = command.to_string();
        let frame = framing::decode_text(&text).expect("bounded settings frame");
        let Some(decoded) = super::decode(&frame)? else {
            panic!("settings command must route");
        };
        self.bridge.apply(CONNECTION_A, &decoded);
        Ok(())
    }

    /// Snapshot of every forwarded message for the fixture connection.
    pub(super) fn messages(&self) -> Vec<SettingsMessage> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == CONNECTION_A)
            .map(|(_, message)| message.clone())
            .collect()
    }

    /// Encodes the most recent outbound message for field-level assertions.
    pub(super) fn last(&self) -> Value {
        let messages = self.messages();
        serde_json::to_value(messages.last().expect("at least one message")).expect("encodes")
    }

    /// Whether any message of this group has been forwarded yet.
    pub(super) fn is_empty(&self) -> bool {
        self.messages().is_empty()
    }
}

/// Creates a bridge over a fresh temporary HOME plus workspace pair.
pub(super) fn bridge() -> TestSettings {
    let ordinal = ROOT_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let parent = std::env::temp_dir().join(format!(
        "lotta-settings-ws-{}-{ordinal}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    let root = parent.canonicalize().expect("canonical root");
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("home root");
    std::fs::create_dir_all(&workspace).expect("workspace root");
    let paths =
        SidePaths::new(home.clone(), None, [workspace.clone()]).expect("fixture side paths");
    let messages: RecordedMessages = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: SettingsForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let bridge =
        Arc::new(SettingsBridge::new(forward, &home, &workspace).expect("settings bridge"));
    TestSettings {
        bridge,
        paths,
        home,
        workspace,
        messages,
    }
}

/// Fixture runtime scope used by reflection commands.
pub(super) fn runtime(agent_id: &str, conversation_id: &str) -> RuntimeScope {
    RuntimeScope {
        agent_id: lotta_domain::AgentId::accept(agent_id).expect("agent id"),
        conversation_id: lotta_domain::ConversationId::accept(conversation_id)
            .expect("conversation id"),
        acting_user_id: None,
    }
}

/// Outbound discriminant of one settings group message.
#[must_use]
pub(super) fn discriminant_of(message: &SettingsMessage) -> &'static str {
    match message {
        SettingsMessage::CwdMapResponse(_) => "get_cwd_map_response",
        SettingsMessage::GetReflectionSettingsResponse(_) => "get_reflection_settings_response",
        SettingsMessage::SetReflectionSettingsResponse(_) => "set_reflection_settings_response",
        SettingsMessage::GetExperimentsResponse(_) => "get_experiments_response",
        SettingsMessage::SetExperimentResponse(_) => "set_experiment_response",
        SettingsMessage::DeviceStatus { .. } => "update_device_status",
    }
}
