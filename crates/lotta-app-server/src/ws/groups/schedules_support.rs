//! Shared fixtures for the schedules command-group certificate selectors.
//!
//! Every fixture drives the real bridge over one unique temporary side-store
//! root: canonical `crons.json` and `runs/` files are seeded and asserted
//! through the same [`SidePaths`](lotta_store::SidePaths) layout the bridge
//! resolves internally.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use lotta_domain::Timestamp;
use lotta_runtime::schedule::{RunLogAction, RunLogEntry, RunLogStatus, ScheduleFile};
use lotta_store::SidePaths;
use lotta_store::schedule::ScheduleStore;
use serde_json::{Value, json};

use super::{SchedulesBridge, SchedulesForwarder, SchedulesMessage};
use crate::{framing, ws::ConnectionId};

/// First test connection identity.
pub(super) const CONNECTION_A: ConnectionId = 21;

/// Canonical fixture evaluation instant: 2026-01-03T00:00:00Z.
pub(super) fn t0() -> Timestamp {
    Timestamp::parse_persisted_rfc3339("2026-01-03T00:00:00Z").expect("fixture instant")
}

type RecordedMessages = Arc<Mutex<Vec<(ConnectionId, SchedulesMessage)>>>;

static ROOT_ORDINAL: AtomicUsize = AtomicUsize::new(0);

/// One bridge over a unique temporary storage root plus its recorder.
pub(super) struct TestSchedules {
    /// Bridge under test; every command applies inline via [`Self::send`].
    pub(super) bridge: Arc<SchedulesBridge>,
    /// Side paths mirroring the bridge's internal layout for seeding.
    pub(super) paths: SidePaths,
    /// Fixture agent identifier.
    pub(super) agent_id: String,
    messages: RecordedMessages,
}

impl TestSchedules {
    /// Decodes a raw JSON command through framing and applies it inline.
    pub(super) async fn send(&self, command: &Value) {
        let text = command.to_string();
        let frame = framing::decode_text(&text).expect("bounded schedules frame");
        let decoded = super::decode(&frame)
            .expect("wellformed schedules command")
            .expect("schedules command routed");
        self.bridge.apply(CONNECTION_A, &decoded).await;
    }

    /// Snapshot of every forwarded message for the fixture connection.
    pub(super) fn messages(&self) -> Vec<SchedulesMessage> {
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
        encoded(messages.last().unwrap_or_else(|| panic!("no messages")))
    }

    /// Seeds one canonical schedule file directly through the store layout.
    pub(super) fn seed_file(&self, file: &ScheduleFile) {
        let bytes = file.encode().expect("canonical encode");
        let path = self.paths.crons().expect("crons path");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("letta home");
        std::fs::write(&path, bytes).expect("seed crons.json");
    }

    /// Loads the current canonical file straight from disk.
    pub(super) fn stored(&self) -> Option<ScheduleFile> {
        ScheduleStore::new(&self.paths)
            .load()
            .ok()
            .map(|loaded| loaded.file)
    }

    /// Whether the canonical file exists at all.
    pub(super) fn crons_exist(&self) -> bool {
        self.paths.crons().is_ok_and(|path| path.exists())
    }

    /// Exact bytes of the canonical file, for zero-write comparisons.
    pub(super) fn crons_bytes(&self) -> Vec<u8> {
        std::fs::read(self.paths.crons().expect("crons path")).expect("read crons.json")
    }
}

/// Builds a canonical active schedule record with pinned defaults.
#[must_use]
pub(super) fn schedule_json(overrides: Value) -> Value {
    let mut base = json!({
        "id": "schedule-1",
        "agent_id": "agent-local-fixture",
        "conversation_id": "conversation",
        "name": "name",
        "description": "description",
        "cron": "* * * * *",
        "timezone": "UTC",
        "recurring": true,
        "prompt": "prompt",
        "status": "active",
        "created_at": "2026-01-01T00:00:00Z",
        "expires_at": null,
        "last_fired_at": null,
        "fire_count": 0,
        "cancel_reason": null,
        "jitter_offset_ms": 0,
        "last_run_at": null,
        "last_run_outcome": null,
        "last_run_reason": null,
        "last_run_error": null,
        "last_missed_at": null,
        "missed_count": 0,
        "failed_count": 0,
        "scheduled_for": null,
        "fired_at": null,
        "missed_at": null,
    });
    let Value::Object(fields) = overrides else {
        panic!("overrides must be an object");
    };
    for (key, value) in fields {
        base[key] = value;
    }
    base
}

/// Wraps schedule records into a canonical file.
#[must_use]
pub(super) fn file_with(tasks: &[Value]) -> ScheduleFile {
    serde_json::from_value(json!({
        "version": 1,
        "scheduler_owner": null,
        "tasks": tasks,
    }))
    .expect("canonical file")
}

/// Appends one canonical run-log entry through the real store adapter.
pub(super) fn append_run_log(paths: &SidePaths, task_id: &str, entry: &RunLogEntry) {
    lotta_store::schedule::RunLogStore::new(paths)
        .append(task_id, entry)
        .expect("run-log append");
}

/// Builds one finished run-log record for fixtures.
#[must_use]
pub(super) fn run_log_entry(ts: i64, summary: &str) -> RunLogEntry {
    RunLogEntry {
        ts,
        job_id: "schedule-1".to_owned(),
        action: RunLogAction::Finished,
        status: Some(RunLogStatus::Ok),
        outcome: None,
        reason: None,
        error: None,
        summary: Some(summary.to_owned()),
        agent_id: Some("agent-local-fixture".to_owned()),
        conversation_id: Some("conversation".to_owned()),
        run_id: None,
        run_at_ms: Some(ts),
        queue_item_id: None,
        scheduled_for: None,
        fired_at: None,
        missed_count: None,
        window_start: None,
        window_end: None,
    }
}

/// Creates a bridge over a fresh temporary canonical storage root.
pub(super) fn bridge() -> TestSchedules {
    let ordinal = ROOT_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let parent = std::env::temp_dir().join(format!(
        "lotta-schedules-ws-{}-{ordinal}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    let root = parent.canonicalize().expect("canonical root");
    let paths = SidePaths::new(root.clone(), None, []).expect("fixture side paths");
    let messages: RecordedMessages = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: SchedulesForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let clock = Arc::new(lotta_testkit::clock::FakeClock::new(t0()));
    let bridge = Arc::new(SchedulesBridge::new(forward, &root, clock).expect("schedules bridge"));
    TestSchedules {
        bridge,
        paths,
        agent_id: "agent-local-schedules-test".to_owned(),
        messages,
    }
}

/// Outbound discriminant of one schedule group message.
#[must_use]
pub(super) fn discriminant_of(message: &SchedulesMessage) -> &'static str {
    match message {
        SchedulesMessage::ListResponse(_) => "cron_list_response",
        SchedulesMessage::AddResponse(_) => "cron_add_response",
        SchedulesMessage::GetResponse(_) => "cron_get_response",
        SchedulesMessage::RunsResponse(_) => "cron_runs_response",
        SchedulesMessage::TriggerResponse(_) => "cron_trigger_response",
        SchedulesMessage::UpdateResponse(_) => "cron_update_response",
        SchedulesMessage::DeleteResponse(_) => "cron_delete_response",
        SchedulesMessage::DeleteAllResponse(_) => "cron_delete_all_response",
        SchedulesMessage::Updated(_) => "crons_updated",
    }
}

pub(super) fn fixture_discriminants(section: &str) -> Vec<String> {
    let raw = include_str!("../../../../../fixtures/protocol/discriminants.json");
    let fixture: Value = serde_json::from_str(raw).expect("bounded fixture");
    fixture[section]["discriminants"]
        .as_array()
        .expect("discriminants")
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn assert_in_fixture(section: &str, tag: &str) {
    assert!(
        fixture_discriminants(section)
            .iter()
            .any(|entry| entry == tag),
        "{tag} missing from the {section} fixture group"
    );
}

/// Encodes one outbound message for field-level assertions.
pub(super) fn encoded(message: &SchedulesMessage) -> Value {
    serde_json::to_value(message).expect("response encodes")
}
