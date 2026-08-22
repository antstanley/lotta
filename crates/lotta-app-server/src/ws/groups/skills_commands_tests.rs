//! `ws::skills::commands` — enable adds the skill to the runtime's selected
//! sources and the discoverable global directory, disable removes it, and each
//! successful mutation emits a `skills_updated` snapshot after its response.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};

use super::{SkillsBridge, SkillsForwarder, SkillsMessage};
use crate::{framing, ws::ConnectionId};

/// First test connection identity.
const CONNECTION_A: ConnectionId = 31;

type RecordedMessages = Arc<Mutex<Vec<(ConnectionId, SkillsMessage)>>>;

static ROOT_ORDINAL: AtomicUsize = AtomicUsize::new(0);

/// One bridge over a unique temporary storage root plus its recorder.
struct TestSkills {
    bridge: Arc<SkillsBridge>,
    /// Storage root playing this server's HOME.
    root: std::path::PathBuf,
    messages: RecordedMessages,
}

impl TestSkills {
    fn send(&self, command: &Value) {
        let text = command.to_string();
        let frame = framing::decode_text(&text).expect("bounded skills frame");
        let decoded = super::decode(&frame)
            .expect("wellformed skills command")
            .expect("skills command routed");
        self.bridge.apply(CONNECTION_A, &decoded);
    }

    fn messages(&self) -> Vec<SkillsMessage> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == CONNECTION_A)
            .map(|(_, message)| message.clone())
            .collect()
    }

    fn last(&self) -> Value {
        let messages = self.messages();
        serde_json::to_value(messages.last().expect("at least one message")).expect("encodes")
    }

    /// Creates one real skill directory containing a `SKILL.md` manifest.
    fn skill_dir(&self, name: &str) -> std::path::PathBuf {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).expect("skill directory");
        std::fs::write(dir.join("SKILL.md"), "# skill").expect("manifest");
        dir
    }

    /// Selected skill sources shared with Task 54 setup.
    fn selected(&self) -> Vec<String> {
        self.bridge
            .selected_sources()
            .lock()
            .expect("selection lock")
            .ids()
    }
}

fn bridge() -> TestSkills {
    let ordinal = ROOT_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let parent =
        std::env::temp_dir().join(format!("lotta-skills-ws-{}-{ordinal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    let root = parent.canonicalize().expect("canonical root");
    let messages: RecordedMessages = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: SkillsForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let clock = Arc::new(lotta_testkit::clock::FakeClock::new(
        lotta_domain::Timestamp::parse_persisted_rfc3339("2026-01-03T00:00:00Z")
            .expect("fixture instant"),
    ));
    TestSkills {
        bridge: Arc::new(SkillsBridge::new(forward, &root, clock)),
        root,
        messages,
    }
}

#[test]
fn enable_adds() {
    let fixture = bridge();
    let skill = fixture.skill_dir("note_taker");
    fixture.send(&json!({
        "type": "skill_enable",
        "request_id": "se-1",
        "skill_path": skill.to_str().expect("utf-8 path"),
    }));
    let response = serde_json::to_value(fixture.messages()[0].clone()).expect("encodes");
    assert_eq!(response["type"], "skill_enable_response");
    assert_eq!(response["success"], true);
    assert_eq!(response["skill_name"], "note_taker");
    assert_eq!(
        fixture.selected(),
        vec!["note_taker".to_owned()],
        "the runtime's selected skill sources gain the enabled skill"
    );
    // The link lands in the canonical global skills directory, so the global
    // source Task 54 setup discovers over now contains the entry.
    let link = fixture.bridge.global_skills_directory().join("note_taker");
    assert!(
        link.symlink_metadata()
            .expect("link metadata")
            .file_type()
            .is_symlink(),
        "enabling links the directory into the global source"
    );
}

#[test]
fn enable_rejects_missing_or_manifestless_paths() {
    let fixture = bridge();
    fixture.send(&json!({
        "type": "skill_enable",
        "request_id": "se-2",
        "skill_path": fixture.root.join("absent").to_str().expect("utf-8 path"),
    }));
    assert_eq!(fixture.last()["success"], false);
    assert!(
        fixture.last()["error"]
            .as_str()
            .expect("error text")
            .starts_with("Path does not exist:"),
        "pinned missing-path rejection text"
    );
    let bare = fixture.root.join("bare");
    std::fs::create_dir_all(&bare).expect("directory without manifest");
    fixture.send(&json!({
        "type": "skill_enable",
        "request_id": "se-3",
        "skill_path": bare.to_str().expect("utf-8 path"),
    }));
    assert_eq!(fixture.last()["success"], false);
    assert!(
        fixture.last()["error"]
            .as_str()
            .expect("error text")
            .starts_with("No SKILL.md found in"),
        "pinned missing-manifest rejection text"
    );
    assert!(
        fixture.selected().is_empty(),
        "rejected enables must not touch the runtime selection"
    );
}

#[test]
fn disable_removes() {
    let fixture = bridge();
    let skill = fixture.skill_dir("note_taker");
    fixture.send(&json!({
        "type": "skill_enable",
        "request_id": "sd-0",
        "skill_path": skill.to_str().expect("utf-8 path"),
    }));
    fixture.send(&json!({
        "type": "skill_disable",
        "request_id": "sd-1",
        "name": "note_taker",
    }));
    let response = serde_json::to_value(fixture.messages()[2].clone()).expect("encodes");
    assert_eq!(response["type"], "skill_disable_response");
    assert_eq!(response["success"], true);
    assert_eq!(response["skill_name"], "note_taker");
    assert!(fixture.selected().is_empty(), "disable removes the source");
    assert!(
        fixture
            .bridge
            .global_skills_directory()
            .join("note_taker")
            .symlink_metadata()
            .is_err(),
        "the unlinked entry is gone from the global source"
    );
}

#[test]
fn disable_rejects_absent_or_non_symlink_entries() {
    let fixture = bridge();
    fixture.send(&json!({
        "type": "skill_disable",
        "request_id": "sd-2",
        "name": "absent_skill",
    }));
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(
        fixture.last()["error"],
        "Skill not found: absent_skill",
        "pinned missing-skill rejection text"
    );
    let plain = fixture.bridge.global_skills_directory().join("plain");
    std::fs::create_dir_all(&plain).expect("non-symlink entry");
    fixture.send(&json!({
        "type": "skill_disable",
        "request_id": "sd-3",
        "name": "plain",
    }));
    assert_eq!(fixture.last()["success"], false);
    assert!(
        fixture.last()["error"]
            .as_str()
            .expect("error text")
            .contains("is not a symlink — refusing to delete"),
        "pinned non-symlink refusal text"
    );
    assert!(
        plain.exists(),
        "refused disables must leave the entry untouched"
    );
}

#[test]
fn emits_snapshot() {
    let fixture = bridge();
    let skill = fixture.skill_dir("snapshot_skill");
    fixture.send(&json!({
        "type": "skill_enable",
        "request_id": "su-1",
        "skill_path": skill.to_str().expect("utf-8 path"),
    }));
    fixture.send(&json!({
        "type": "skill_disable",
        "request_id": "su-2",
        "name": "snapshot_skill",
    }));
    let kinds: Vec<&str> = fixture
        .messages()
        .iter()
        .map(|message| match message {
            SkillsMessage::EnableResponse(_) => "skill_enable_response",
            SkillsMessage::DisableResponse(_) => "skill_disable_response",
            SkillsMessage::Updated(_) => "skills_updated",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "skill_enable_response",
            "skills_updated",
            "skill_disable_response",
            "skills_updated",
        ],
        "every successful mutation emits one snapshot after its response"
    );
    let encoded = serde_json::to_value(fixture.messages()[1].clone()).expect("encodes");
    assert_eq!(encoded["type"], "skills_updated");
    assert_eq!(encoded["timestamp"], 1_767_398_400_000_i64, "clock millis");
}
