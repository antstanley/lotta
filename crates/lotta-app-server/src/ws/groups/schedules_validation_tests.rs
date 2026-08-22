//! `ws::schedules::validation` — invalid expressions and unknown timezones are
//! rejected before any store access, leaving the canonical file untouched.

use serde_json::json;

use super::support::{bridge, file_with, schedule_json};

async fn rejected_add(cron: Option<&str>, timezone: Option<&str>, request_id: &str) {
    let fixture = bridge();
    let mut command = json!({
        "type": "cron_add",
        "request_id": request_id,
        "agent_id": fixture.agent_id,
        "name": "broken",
        "description": "invalid input",
        "recurring": true,
        "prompt": "never persisted",
    });
    if let Some(cron) = cron {
        command["cron"] = json!(cron);
    }
    if let Some(timezone) = timezone {
        command["timezone"] = json!(timezone);
    }
    fixture.send(&command).await;
    assert_eq!(fixture.last()["success"], false);
    assert!(!fixture.crons_exist(), "rejected add must write nothing");
}

#[tokio::test]
async fn invalid_expression_rejected_without_writes() {
    rejected_add(Some("definitely not a cron"), None, "v1").await;
    // A bare-value step is equally rejected.
    rejected_add(Some("5/10 * * * *"), None, "v1b").await;
}

#[tokio::test]
async fn unknown_timezone_rejected_without_writes() {
    rejected_add(Some("* * * * *"), Some("Mars/Olympus_Mons"), "tz1").await;
}

async fn seeded_with_invalid_update(field: &str, value: &str) {
    let fixture = bridge();
    fixture.seed_file(&file_with(&[schedule_json(json!({"id": "schedule-1"}))]));
    let original = fixture.crons_bytes();
    fixture
        .send(&json!({
            "type": "cron_update",
            "request_id": format!("u-{field}"),
            "task_id": "schedule-1",
            field: value,
        }))
        .await;
    assert_eq!(fixture.last()["success"], false);
    assert_eq!(
        fixture.crons_bytes(),
        original,
        "rejected update must leave the canonical file byte-identical"
    );
}

#[tokio::test]
async fn invalid_update_values_preserve_file_bytes() {
    // Out-of-range minute fails both parsers.
    seeded_with_invalid_update("cron", "61 * * * *").await;
    // Interval forms that no parser accepts are rejected identically.
    seeded_with_invalid_update("cron", "0m").await;
    // Unknown IANA identifiers are rejected on update too.
    seeded_with_invalid_update("timezone", "Not/AZone").await;
}

#[tokio::test]
async fn read_only_paths_never_create_the_canonical_file() {
    let fresh = bridge();
    fresh
        .send(&json!({"type": "cron_list", "request_id": "v4"}))
        .await;
    assert_eq!(fresh.last()["success"], true);
    assert_eq!(
        fresh.last()["tasks"].as_array().expect("tasks").len(),
        0,
        "listing a fresh install serves an empty page"
    );
    assert!(!fresh.crons_exist(), "listing must not create crons.json");
    fresh
        .send(&json!({
            "type": "cron_update",
            "request_id": "v5",
            "task_id": "absent",
            "name": "ghost",
        }))
        .await;
    assert_eq!(fresh.last()["success"], false);
    assert_eq!(fresh.last()["error"], "Cron task not found");
    assert!(
        !fresh.crons_exist(),
        "absent-target update must not create crons.json"
    );
    fresh
        .send(&json!({
            "type": "cron_delete_all",
            "request_id": "v6",
            "agent_id": fresh.agent_id,
        }))
        .await;
    assert_eq!(fresh.last()["deleted"], 0);
    assert!(
        !fresh.crons_exist(),
        "zero-deletion delete-all must not create crons.json"
    );
}
