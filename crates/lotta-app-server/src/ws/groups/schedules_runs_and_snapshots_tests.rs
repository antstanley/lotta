//! `ws::schedules::runs_and_snapshots` — bounded run-history pages plus one
//! `crons_updated` snapshot per mutating command.

use lotta_runtime::schedule::RunLogEntry;
use serde_json::json;

use super::support::{
    append_run_log, bridge, discriminant_of, encoded, file_with, run_log_entry, schedule_json,
};

/// One run-log entry carrying an explicit run identifier for filter fixtures.
fn identified_entry(ts: i64) -> RunLogEntry {
    let mut entry = run_log_entry(ts, "identified");
    entry.run_id = Some("local-run-42".to_owned());
    entry
}

fn seed_history(fixture: &super::support::TestSchedules, count: i64) {
    fixture.seed_file(&file_with(&[schedule_json(json!({"id": "schedule-1"}))]));
    for ts in 1..=count {
        append_run_log(
            &fixture.paths,
            "schedule-1",
            &run_log_entry(ts, &format!("run{ts}")),
        );
    }
}

#[tokio::test]
async fn serves_newest_first_bounded_pages() {
    // Seven plain entries plus one identified entry: eight total.
    let fixture = bridge();
    seed_history(&fixture, 7);
    append_run_log(&fixture.paths, "schedule-1", &identified_entry(8));

    fixture
        .send(&json!({
            "type": "cron_runs",
            "request_id": "r1",
            "task_id": "schedule-1",
            "limit": 3,
            "offset": 1,
        }))
        .await;
    let value = fixture.last();
    assert_eq!(value["success"], true);
    let page = &value["page"];
    assert_eq!(page["total"], 8);
    assert_eq!(page["offset"], 1);
    assert_eq!(page["limit"], 3);
    assert_eq!(page["hasMore"], true);
    assert_eq!(page["nextOffset"], 4);
    let entries = page["entries"].as_array().expect("entries");
    let stamps: Vec<i64> = entries
        .iter()
        .map(|entry| entry["ts"].as_i64().expect("ts"))
        .collect();
    assert_eq!(
        stamps,
        [7, 6, 5],
        "offset skips the newest entries of the newest-first ordering"
    );

    // Tail pages report hasMore=false with a null next offset.
    fixture
        .send(&json!({
            "type": "cron_runs",
            "request_id": "r2",
            "task_id": "schedule-1",
            "limit": 50,
            "offset": 6,
        }))
        .await;
    let tail = fixture.last()["page"].clone();
    assert_eq!(tail["total"], 8);
    assert_eq!(tail["hasMore"], false);
    assert!(tail["nextOffset"].is_null());
    assert_eq!(
        tail["entries"].as_array().expect("tail entries").len(),
        2,
        "last page carries the remainder"
    );
}

#[tokio::test]
async fn clamps_limits_filters_and_serves_absent_logs() {
    let fixture = bridge();
    seed_history(&fixture, 7);

    // Oversized limits clamp to the pinned page maximum.
    fixture
        .send(&json!({
            "type": "cron_runs",
            "request_id": "r3",
            "task_id": "schedule-1",
            "limit": 5_000,
        }))
        .await;
    let clamped = fixture.last()["page"].clone();
    assert_eq!(
        clamped["entries"].as_array().expect("clamped").len(),
        7,
        "every seeded entry fits inside the clamped page"
    );
    assert_eq!(
        clamped["limit"],
        super::RUNS_PAGE_ENTRIES_MAX,
        "oversized limits clamp to the pinned page maximum"
    );

    // The run-id filter narrows the totals to matching entries only.
    append_run_log(&fixture.paths, "schedule-1", &identified_entry(9));
    fixture
        .send(&json!({
            "type": "cron_runs",
            "request_id": "r4",
            "task_id": "schedule-1",
            "run_id": "local-run-42",
        }))
        .await;
    let narrowed = fixture.last()["page"].clone();
    assert_eq!(narrowed["total"], 1);
    let entries = narrowed["entries"].as_array().expect("filtered entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["runId"], "local-run-42");

    // Absent run logs answer an empty successful page.
    fixture
        .send(&json!({
            "type": "cron_runs",
            "request_id": "r5",
            "task_id": "never-ran",
        }))
        .await;
    let empty = fixture.last()["page"].clone();
    assert_eq!(empty["total"], 0);
    assert_eq!(
        empty["entries"].as_array().expect("empty").len(),
        0,
        "absent histories serve an empty page"
    );
}

#[tokio::test]
async fn add_emits_crons_updated_snapshot() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "cron_add",
            "request_id": "a1",
            "agent_id": fixture.agent_id,
            "name": "nightly",
            "description": "d",
            "cron": "* * * * *",
            "recurring": true,
            "prompt": "p",
        }))
        .await;
    let messages = fixture.messages();
    assert_eq!(messages.len(), 2, "response first, snapshot after");
    assert_eq!(discriminant_of(&messages[0]), "cron_add_response");
    assert_eq!(discriminant_of(&messages[1]), "crons_updated");
    super::support::assert_in_fixture("messages", "crons_updated");
    let snapshot = encoded(&messages[1]);
    assert_eq!(
        snapshot["timestamp"],
        super::support::t0().as_utc().timestamp_millis(),
        "snapshot stamp reads the injected clock"
    );
    assert_eq!(snapshot["agent_id"], fixture.agent_id);
    assert_eq!(snapshot["conversation_id"], "new");
}

#[tokio::test]
async fn update_emits_crons_updated_snapshot() {
    let fixture = bridge();
    fixture.seed_file(&file_with(&[schedule_json(json!({"id": "schedule-1"}))]));
    fixture
        .send(&json!({
            "type": "cron_update",
            "request_id": "u1",
            "task_id": "schedule-1",
            "name": "renamed",
        }))
        .await;
    let messages = fixture.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(discriminant_of(&messages[0]), "cron_update_response");
    assert_eq!(discriminant_of(&messages[1]), "crons_updated");
    let snapshot = encoded(&messages[1]);
    assert_eq!(snapshot["agent_id"], "agent-local-fixture");
    assert_eq!(snapshot["conversation_id"], "conversation");
}

#[tokio::test]
async fn delete_emits_crons_updated_snapshot() {
    let fixture = bridge();
    fixture.seed_file(&file_with(&[schedule_json(json!({"id": "schedule-1"}))]));
    fixture
        .send(&json!({
            "type": "cron_delete",
            "request_id": "d1",
            "task_id": "schedule-1",
        }))
        .await;
    let messages = fixture.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(discriminant_of(&messages[0]), "cron_delete_response");
    assert_eq!(discriminant_of(&messages[1]), "crons_updated");
    let snapshot = encoded(&messages[1]);
    assert_eq!(snapshot["agent_id"], "agent-local-fixture");

    // An absent delete mutates nothing and therefore emits no snapshot.
    fixture
        .send(&json!({
            "type": "cron_delete",
            "request_id": "d2",
            "task_id": "absent",
        }))
        .await;
    let after_absent = fixture.messages();
    assert_eq!(
        after_absent.len(),
        3,
        "found=false delete skips the snapshot"
    );
    assert_eq!(
        discriminant_of(after_absent.last().expect("absent response")),
        "cron_delete_response"
    );
}

#[tokio::test]
async fn delete_all_emits_crons_updated_snapshot() {
    let fixture = bridge();
    fixture.seed_file(&file_with(&[
        schedule_json(json!({"id": "schedule-a"})),
        schedule_json(json!({"id": "schedule-b", "agent_id": "agent-local-other"})),
    ]));
    fixture
        .send(&json!({
            "type": "cron_delete_all",
            "request_id": "da1",
            "agent_id": "agent-local-fixture",
        }))
        .await;
    let messages = fixture.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(discriminant_of(&messages[0]), "cron_delete_all_response");
    assert_eq!(discriminant_of(&messages[1]), "crons_updated");
    let snapshot = encoded(&messages[1]);
    assert_eq!(snapshot["agent_id"], "agent-local-fixture");
    assert!(
        snapshot.get("conversation_id").is_none(),
        "delete-all scopes by agent only, like the pinned spread"
    );

    // Deleting zero schedules emits no snapshot.
    fixture
        .send(&json!({
            "type": "cron_delete_all",
            "request_id": "da2",
            "agent_id": "agent-local-absent",
        }))
        .await;
    let after_empty = fixture.messages();
    assert_eq!(
        after_empty.len(),
        3,
        "zero-deletion delete-all skips the snapshot"
    );
    assert_eq!(
        discriminant_of(after_empty.last().expect("empty response")),
        "cron_delete_all_response"
    );
}
