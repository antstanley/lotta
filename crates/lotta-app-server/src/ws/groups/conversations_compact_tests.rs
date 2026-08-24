//! `ws::conversations::compact` — the compact command routes through the Task
//! 58 lease-serialized compaction service: exactly one compaction entry is
//! appended, in-context identifiers are published, the durable transaction
//! journal records the full claim/projection/publication lifecycle, and a
//! competing owner holding the scope's turn lease is rejected.

use serde_json::{Value, json};

use super::support::{TestConversations, bridge};
use lotta_domain::{ConversationId, RuntimeScope};

fn transcript_lines(
    fixture: &TestConversations,
    agent: &lotta_domain::AgentId,
    conversation: &ConversationId,
) -> Vec<Value> {
    let path = fixture.transcript_path(agent, conversation);
    let bytes = std::fs::read(path).expect("transcript bytes");
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
        .map(|row| serde_json::from_slice(row).expect("canonical transcript row"))
        .collect()
}

#[tokio::test]
async fn compact_appends_exactly_one_entry_and_updates_in_context_ids() {
    let fixture = bridge();
    let agent = fixture.seed_agent("compactflow", "Compact Flow").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-30", 4).await;
    assert_eq!(fixture.compaction_rows(&agent, &seeded), 0);

    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "compact-1",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);

    // Exactly one compaction entry was appended after the untouched header
    // and four inherited message rows.
    let rows = transcript_lines(&fixture, &agent, &seeded);
    assert_eq!(rows.len(), 6);
    assert_eq!(
        rows[0]["type"], "session",
        "the original session header survives"
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row["type"] == "compaction")
            .count(),
        1,
        "exactly one compaction entry was appended"
    );
    let entry = rows
        .iter()
        .find(|row| row["type"] == "compaction")
        .expect("compaction row");
    assert_eq!(entry["tokensBefore"], entry["tokensBefore"]);
    assert_eq!(entry["messagesBefore"], 4);
    assert_eq!(entry["messagesAfter"], 3);

    // In-context identifiers were republished: summary first, retained tail.
    let record = fixture.conversation_value(&agent, &seeded).await;
    let context = record["in_context_message_ids"]
        .as_array()
        .expect("context ids");
    assert_eq!(
        context.len(),
        3,
        "summary plus the retained sliding-window tail"
    );
    assert!(
        context[0].as_str().expect("id").ends_with("-summary"),
        "the summary message id leads in-context identifiers"
    );
    assert_eq!(record["summary"], fixture.last()["compaction"]["summary"]);
}

#[tokio::test]
async fn compact_routes_through_the_task58_transaction_flow_not_a_direct_write() {
    let fixture = bridge();
    let agent = fixture.seed_agent("compact58", "Compact Task58").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-31", 4).await;

    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "compact-2",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);

    // The Task 58 service owns a durable scoped transaction; direct transcript
    // writes would leave no claimed journal with its recorded projection.
    let journal = fixture.compaction_journal();
    let transactions = journal["transactions"].as_array().expect("transactions");
    assert_eq!(transactions.len(), 1, "one scoped request was claimed");
    let transaction = &transactions[0];
    assert_eq!(transaction["state"]["state"], "published");
    let projection = &transaction["projection"];
    assert_eq!(projection["messages_before"], 4);
    assert_eq!(projection["messages_after"], 3);
    assert_eq!(
        projection["retained_message_ids"].as_array().map(Vec::len),
        Some(2)
    );

    // The transcript grew by append only: the original five lines remain and
    // exactly one line was added.
    let path = fixture.transcript_path(&agent, &seeded);
    let bytes = std::fs::read(path).expect("transcript bytes");
    let lines: Vec<&[u8]> = bytes
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
        .collect();
    assert_eq!(lines.len(), 6);

    // A second manual compaction claims its own request and appends once more.
    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "compact-3",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.compaction_rows(&agent, &seeded), 2);
}

#[tokio::test]
async fn compact_is_serialized_under_the_turn_lease_and_rejects_busy_scopes() {
    let fixture = bridge();
    let agent = fixture.seed_agent("compactlease", "Compact Lease").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-32", 4).await;
    let scope = RuntimeScope::new(
        agent.clone(),
        ConversationId::accept(seeded.as_str()).expect("scope id"),
        None,
    );

    // Another owner holds the scope's turn lease while compacting.
    let held = fixture.bridge.test_hold_lease(&scope).expect("held lease");

    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "compact-4",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(
        fixture.last()["success"],
        false,
        "a busy scope rejects compaction"
    );
    assert_eq!(fixture.last()["error"], "conversation busy");
    assert_eq!(
        fixture.compaction_rows(&agent, &seeded),
        0,
        "no entry without the lease"
    );

    // Once the lease returns to idle the same command serializes through.
    drop(held);
    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "compact-5",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.compaction_rows(&agent, &seeded), 1);
}
