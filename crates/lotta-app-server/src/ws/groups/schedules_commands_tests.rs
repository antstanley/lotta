//! One decode-route-respond case per schedule command in the pinned fixture.

use super::support::{
    assert_in_fixture, bridge, discriminant_of, encoded, file_with, schedule_json,
};
use serde_json::json;

#[tokio::test]
async fn cron_list_decodes_routes_and_responds() {
    assert_in_fixture("commands", "cron_list");
    let fixture = bridge();
    fixture.seed_file(&file_with(&[
        schedule_json(json!({"id": "schedule-a"})),
        schedule_json(json!({
            "id": "schedule-b",
            "agent_id": "agent-local-other",
            "conversation_id": "conversation-2",
        })),
    ]));
    fixture
        .send(&json!({
            "type": "cron_list",
            "request_id": "l1",
            "agent_id": "agent-local-fixture",
        }))
        .await;
    let value = fixture.last();
    assert_in_fixture("messages", "cron_list_response");
    assert_eq!(value["type"], "cron_list_response");
    assert_eq!(value["request_id"], "l1");
    assert_eq!(value["success"], true);
    let tasks = value["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], "schedule-a");
    assert_eq!(tasks[0]["timezone"], "UTC");

    // Without a filter every record is returned.
    fixture
        .send(&json!({"type": "cron_list", "request_id": "l2"}))
        .await;
    let unfiltered = fixture.last();
    assert_eq!(
        unfiltered["tasks"].as_array().expect("tasks").len(),
        2,
        "unfiltered listing returns both agents"
    );
}

#[tokio::test]
async fn cron_add_decodes_routes_and_responds() {
    assert_in_fixture("commands", "cron_add");
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "cron_add",
            "request_id": "a1",
            "agent_id": fixture.agent_id,
            "name": "nightly",
            "description": "runs nightly",
            "cron": "0 3 * * *",
            "recurring": true,
            "prompt": "check the build",
        }))
        .await;
    let responses = fixture.messages();
    assert_eq!(responses.len(), 2, "response then crons_updated snapshot");
    assert_eq!(discriminant_of(&responses[0]), "cron_add_response");
    assert_in_fixture("messages", "cron_add_response");
    assert_eq!(discriminant_of(&responses[1]), "crons_updated");
    let value = encoded(&responses[0]);
    assert_eq!(value["request_id"], "a1");
    assert_eq!(value["success"], true);
    assert_eq!(value["task"]["status"], "active");
    assert_eq!(value["task"]["cron"], "0 3 * * *");
    assert_eq!(value["task"]["conversation_id"], "new");
    assert_eq!(
        value["task"]["fire_count"], 0,
        "created schedules never fired"
    );
    let stored = fixture.stored().expect("persisted file");
    assert_eq!(stored.tasks.len(), 1);
    assert_eq!(stored.tasks[0].id.as_str(), value["task"]["id"]);
}

#[tokio::test]
async fn cron_get_decodes_routes_and_responds() {
    assert_in_fixture("commands", "cron_get");
    let fixture = bridge();
    fixture.seed_file(&file_with(&[schedule_json(json!({"id": "schedule-1"}))]));
    fixture
        .send(&json!({
            "type": "cron_get",
            "request_id": "g1",
            "task_id": "schedule-1",
        }))
        .await;
    let value = fixture.last();
    assert_in_fixture("messages", "cron_get_response");
    assert_eq!(value["type"], "cron_get_response");
    assert_eq!(value["success"], true);
    assert_eq!(value["found"], true);
    assert_eq!(value["task"]["id"], "schedule-1");
    assert_eq!(value["task"]["prompt"], "prompt");

    fixture
        .send(&json!({
            "type": "cron_get",
            "request_id": "g2",
            "task_id": "absent",
        }))
        .await;
    let missing = fixture.last();
    assert_eq!(missing["success"], true);
    assert_eq!(missing["found"], false);
    assert!(missing["task"].is_null());
}

#[tokio::test]
async fn cron_runs_decodes_routes_and_responds() {
    assert_in_fixture("commands", "cron_runs");
    let fixture = bridge();
    super::support::append_run_log(
        &fixture.paths,
        "schedule-1",
        &super::support::run_log_entry(1, "first"),
    );
    super::support::append_run_log(
        &fixture.paths,
        "schedule-1",
        &super::support::run_log_entry(2, "second"),
    );
    fixture
        .send(&json!({
            "type": "cron_runs",
            "request_id": "r1",
            "task_id": "schedule-1",
        }))
        .await;
    let value = fixture.last();
    assert_in_fixture("messages", "cron_runs_response");
    assert_eq!(value["type"], "cron_runs_response");
    assert_eq!(value["success"], true);
    let page = &value["page"];
    assert_eq!(page["total"], 2);
    assert_eq!(page["limit"], 50, "default limit is the pinned 50");
    let entries = page["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["summary"], "second", "newest first");
    assert_eq!(entries[0]["jobId"], "schedule-1");
}

#[tokio::test]
async fn cron_trigger_decodes_routes_and_responds() {
    assert_in_fixture("commands", "cron_trigger");
    let fixture = bridge();
    fixture.seed_file(&file_with(&[schedule_json(json!({"id": "schedule-1"}))]));
    fixture
        .send(&json!({
            "type": "cron_trigger",
            "request_id": "t1",
            "task_id": "schedule-1",
        }))
        .await;
    let value = fixture.last();
    assert_in_fixture("messages", "cron_trigger_response");
    assert_eq!(value["type"], "cron_trigger_response");
    assert_eq!(value["success"], true);
    assert_eq!(value["found"], true);
    assert_eq!(value["task"]["id"], "schedule-1");
    assert_eq!(value["task"]["fire_count"], 1, "trigger records one fire");

    fixture
        .send(&json!({
            "type": "cron_trigger",
            "request_id": "t2",
            "task_id": "absent",
        }))
        .await;
    let missing = fixture.last();
    assert_eq!(missing["success"], false);
    assert_eq!(missing["found"], false);
    assert_eq!(missing["error"], "Schedule not found");
}

#[tokio::test]
async fn cron_update_decodes_routes_and_responds() {
    assert_in_fixture("commands", "cron_update");
    let fixture = bridge();
    fixture.seed_file(&file_with(&[schedule_json(json!({"id": "schedule-1"}))]));
    fixture
        .send(&json!({
            "type": "cron_update",
            "request_id": "u1",
            "task_id": "schedule-1",
            "name": "renamed",
            "prompt": "updated prompt",
        }))
        .await;
    let responses = fixture.messages();
    assert_eq!(responses.len(), 2, "response then crons_updated snapshot");
    assert_eq!(discriminant_of(&responses[0]), "cron_update_response");
    assert_in_fixture("messages", "cron_update_response");
    let value = encoded(&responses[0]);
    assert_eq!(value["request_id"], "u1");
    assert_eq!(value["success"], true);
    assert_eq!(value["task"]["name"], "renamed");
    assert_eq!(value["task"]["prompt"], "updated prompt");
    assert_eq!(value["task"]["cron"], "* * * * *", "untouched field kept");

    fixture
        .send(&json!({
            "type": "cron_update",
            "request_id": "u2",
            "task_id": "absent",
            "name": "ghost",
        }))
        .await;
    let missing = fixture.last();
    assert_eq!(missing["success"], false);
    assert_eq!(missing["error"], "Cron task not found");
}

#[tokio::test]
async fn cron_delete_decodes_routes_and_responds() {
    assert_in_fixture("commands", "cron_delete");
    let fixture = bridge();
    fixture.seed_file(&file_with(&[schedule_json(json!({"id": "schedule-1"}))]));
    fixture
        .send(&json!({
            "type": "cron_delete",
            "request_id": "d1",
            "task_id": "schedule-1",
        }))
        .await;
    let responses = fixture.messages();
    assert_eq!(responses.len(), 2, "response then crons_updated snapshot");
    assert_eq!(discriminant_of(&responses[0]), "cron_delete_response");
    assert_in_fixture("messages", "cron_delete_response");
    let value = encoded(&responses[0]);
    assert_eq!(value["request_id"], "d1");
    assert_eq!(value["success"], true);
    assert_eq!(value["found"], true);
    assert!(fixture.stored().expect("stored file").tasks.is_empty());

    // Deleting an absent schedule answers found=false and writes nothing.
    fixture
        .send(&json!({
            "type": "cron_delete",
            "request_id": "d2",
            "task_id": "absent",
        }))
        .await;
    let absent = fixture.messages();
    assert_eq!(absent.len(), 3, "no snapshot for an absent delete");
    let value = encoded(absent.last().expect("delete response"));
    assert_eq!(value["success"], true);
    assert_eq!(value["found"], false);
}

#[tokio::test]
async fn cron_delete_all_decodes_routes_and_responds() {
    assert_in_fixture("commands", "cron_delete_all");
    let fixture = bridge();
    fixture.seed_file(&file_with(&[
        schedule_json(json!({"id": "schedule-a"})),
        schedule_json(json!({"id": "schedule-b"})),
        schedule_json(json!({"id": "schedule-c", "agent_id": "agent-local-other"})),
    ]));
    fixture
        .send(&json!({
            "type": "cron_delete_all",
            "request_id": "da1",
            "agent_id": "agent-local-fixture",
        }))
        .await;
    let responses = fixture.messages();
    assert_eq!(responses.len(), 2, "response then crons_updated snapshot");
    assert_eq!(discriminant_of(&responses[0]), "cron_delete_all_response");
    assert_in_fixture("messages", "cron_delete_all_response");
    let value = encoded(&responses[0]);
    assert_eq!(value["request_id"], "da1");
    assert_eq!(value["success"], true);
    assert_eq!(value["deleted"], 2);
    let stored = fixture.stored().expect("stored file");
    assert_eq!(stored.tasks.len(), 1);
    assert_eq!(stored.tasks[0].agent_id.as_str(), "agent-local-other");
}
