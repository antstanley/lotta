//! `ws::settings::cwd_change` — a cwd change applies to subsequent turns and
//! a missing directory falls back while recording the original path for a
//! one-time reminder, reusing Task 54's `CwdResolution` behavior.

use lotta_runtime::turn::setup::CwdResolution;
use serde_json::json;

use super::support::{CONNECTION_A, bridge, discriminant_of};
use super::{CwdChange, scope_key};

/// Fixture scope identifiers.
const AGENT_ID: &str = "agent-local-cwd-change";
const CONVERSATION_ID: &str = "local-conv-cwd";

fn change(cwd: &std::path::Path) -> CwdChange {
    CwdChange {
        agent_id: Some(AGENT_ID.to_owned()),
        conversation_id: CONVERSATION_ID.to_owned(),
        cwd: cwd.to_string_lossy().into_owned(),
    }
}

#[test]
fn applies_to_next_turn() {
    let fixture = bridge();
    let target = fixture.workspace.join("next-turn");
    std::fs::create_dir_all(&target).expect("target directory");
    let outcome = fixture
        .bridge
        .apply_cwd_change(CONNECTION_A, &change(&target))
        .expect("cwd change applies");
    assert!(
        outcome.persisted,
        "the change persists through the side store"
    );
    // The next turn in this scope resolves the new directory as requested.
    assert_eq!(
        fixture
            .bridge
            .cwd_for_next_turn(Some(AGENT_ID), CONVERSATION_ID),
        CwdResolution::Requested(target.clone()),
        "subsequent turns receive the new cwd"
    );
    // No reminder is pending for a usable directory.
    assert_eq!(
        fixture
            .bridge
            .claim_missing_cwd_reminder(Some(AGENT_ID), CONVERSATION_ID),
        None,
        "usable directories never record reminders"
    );
    // The change is visible on the wire map under the pinned scope key.
    fixture
        .send(&json!({"type": "get_cwd_map", "request_id": "cw-1"}))
        .expect("wellformed command");
    let key = scope_key(Some(AGENT_ID), Some(CONVERSATION_ID));
    assert_eq!(
        fixture.last()["cwd_map"][key.as_str()],
        target.to_string_lossy().as_ref(),
        "the persisted map carries the pinned listener scope key"
    );
}

#[test]
fn missing_dir_falls_back_and_reminds_once() {
    let fixture = bridge();
    let missing = fixture.workspace.join("deleted");
    std::fs::create_dir_all(&missing).expect("seed then delete");
    std::fs::remove_dir(&missing).expect("deleted");
    let outcome = fixture
        .bridge
        .apply_cwd_change(CONNECTION_A, &change(&missing))
        .expect("cwd change applies");
    // Task 54 classification: deleted directories fall back to the boot root
    // and retain the original path for the one-use reminder.
    let CwdResolution::DeletedFallback { original, fallback } = &outcome.resolution else {
        panic!("expected deleted fallback, got {:?}", outcome.resolution);
    };
    assert_eq!(original, &missing);
    assert_eq!(fallback, &fixture.workspace, "fallback is the boot cwd");
    // The subsequent turn uses the fallback, not the missing path.
    assert_eq!(
        fixture
            .bridge
            .cwd_for_next_turn(Some(AGENT_ID), CONVERSATION_ID),
        CwdResolution::DeletedFallback {
            original: missing.clone(),
            fallback: fixture.workspace.clone(),
        },
    );
    // The reminder fires exactly once.
    assert_eq!(
        fixture
            .bridge
            .claim_missing_cwd_reminder(Some(AGENT_ID), CONVERSATION_ID),
        Some(missing.to_string_lossy().into_owned()),
        "first claim returns the original requested path"
    );
    assert_eq!(
        fixture
            .bridge
            .claim_missing_cwd_reminder(Some(AGENT_ID), CONVERSATION_ID),
        None,
        "second claim is empty — the reminder is one-time"
    );
    // A successful mutation still emits exactly one device-status snapshot.
    let snapshots = fixture
        .messages()
        .iter()
        .filter(|message| discriminant_of(message) == "update_device_status")
        .count();
    assert_eq!(snapshots, 1, "one snapshot per successful mutation");
}
