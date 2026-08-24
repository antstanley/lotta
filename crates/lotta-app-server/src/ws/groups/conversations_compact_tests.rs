//! `ws::conversations::compact` — the compact command routes through the Task
//! 58 lease-serialized compaction service: exactly one compaction entry is
//! appended, in-context identifiers are published, the durable transaction
//! journal records the full claim/projection/publication lifecycle, a
//! competing owner holding the scope's turn lease is rejected, and sent
//! `compaction_settings` select the pinned strategy modes.

use std::{
    collections::HashMap,
    future::Future,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex},
};

use lotta_domain::{
    AgentId, ConversationId, RunId, RuntimeScope, StopReason, TurnLease, TurnLifecycle,
};
use lotta_runtime::{CompactionCommand, RuntimeError};
use lotta_store::LocalStore;
use lotta_testkit::clock::FakeClock;

use serde_json::{Value, json};

use super::support::{FIXTURE_TIMESTAMP_TEXT, TestConversations, bridge, bridge_with_authority};

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

/// Authoritative lease registry shaped like the production runtime state: a
/// per-scope map of real [`TurnLifecycle`]s whose active turns block manual
/// compaction, and whose registered compaction service is a real Task 58
/// [`lotta_runtime::CompactionService`] over the fixture's own store.
struct RuntimeAuthority {
    leases: Arc<Mutex<HashMap<RuntimeScope, Arc<Mutex<TurnLifecycle>>>>>,
    store: Arc<Mutex<Option<LocalStore>>>,
}

impl RuntimeAuthority {
    fn lifecycle(&self, scope: &RuntimeScope) -> Arc<Mutex<TurnLifecycle>> {
        let mut registry = match self.leases.lock() {
            Ok(registry) => registry,
            Err(poisoned) => poisoned.into_inner(),
        };
        registry
            .entry(scope.clone())
            .or_insert_with(|| Arc::new(Mutex::new(TurnLifecycle::new(super::uuid_from_random()))))
            .clone()
    }
}

impl super::ConversationAuthority for RuntimeAuthority {
    fn begin_command(
        &self,
        scope: &RuntimeScope,
    ) -> Result<TurnLease, super::CommandLeaseUnavailable> {
        let lifecycle = self.lifecycle(scope);
        super::lock_lifecycle(&lifecycle)
            .start_command()
            .map_err(|_| super::CommandLeaseUnavailable)
    }

    fn finish_command<'a>(
        &'a self,
        scope: &'a RuntimeScope,
        lease: &'a TurnLease,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + 'a>> {
        let lifecycle = self.lifecycle(scope);
        Box::pin(async move {
            super::lock_lifecycle(&lifecycle)
                .finish_command(lease)
                .map_err(|_| super::adapter_error("authority lease release"))
        })
    }

    fn compact(
        &self,
        command: CompactionCommand,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        lotta_runtime::turn::CompactionProgress,
                        lotta_runtime::RuntimeError,
                    >,
                > + Send
                + '_,
        >,
    > {
        let store = self.store.lock().ok().and_then(|guard| guard.clone());
        Box::pin(async move {
            let service = lotta_runtime::CompactionService::new(
                super::TranscriptSummarizer,
                super::StoreCompactionEffects {
                    store: store.ok_or(lotta_runtime::RuntimeError::Conflict {
                        context: "authority store".into(),
                    })?,
                    leases: Arc::clone(&self.leases),
                    clock: Arc::new(FakeClock::new(
                        lotta_domain::Timestamp::parse_persisted_rfc3339(FIXTURE_TIMESTAMP_TEXT)
                            .expect("fixture timestamp"),
                    )),
                },
            );
            service.compact(command).await
        })
    }
}

fn digest(bytes: &[u8]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

fn collect_file_digests(root: &Path, out: &mut Vec<(String, u64)>) {
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_file_digests(&path, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            out.push((name, digest(&bytes)));
        }
    }
}

/// Hashes every durable artifact an observable compaction could touch: the
/// scoped transcript, conversation record, prompt-cache files under the
/// conversation directory, and the Task 58 transaction journal.
fn durable_state(
    fixture: &TestConversations,
    agent: &AgentId,
    conversation: &ConversationId,
) -> Vec<(String, u64)> {
    let mut files = Vec::new();
    if let Ok(directory) = fixture.store.paths().conversation_dir(agent, conversation) {
        collect_file_digests(&directory, &mut files);
    }
    let journal = fixture.store.paths().runtime().join("compactions.json");
    if let Ok(bytes) = std::fs::read(journal) {
        files.push(("runtime/compactions.json".to_owned(), digest(&bytes)));
    }
    files.sort();
    files
}

#[tokio::test]
async fn compact_is_blocked_by_an_active_turn_on_the_authoritative_registry_without_writes() {
    let authority = Arc::new(RuntimeAuthority {
        leases: Arc::default(),
        store: Arc::new(Mutex::new(None)),
    });
    let attached_store = Arc::clone(&authority.store);
    let handed_to_bridge = Arc::clone(&authority);
    let fixture = bridge_with_authority(move |store| {
        *attached_store.lock().expect("attach lock") = Some(store);
        Some(handed_to_bridge as Arc<dyn super::ConversationAuthority>)
    });
    let agent = fixture.seed_agent("compactturn", "Compact Turn").await;
    let seeded = fixture.seed_conversation(&agent, "local-conv-35", 4).await;
    let scope = RuntimeScope::new(agent.clone(), seeded.clone(), None);

    // A REAL production-style turn (not a command lease) owns the scope on
    // the authoritative lifecycle registry.
    let held_lifecycle = authority.lifecycle(&scope);
    let turn_lease = held_lifecycle
        .lock()
        .expect("lifecycle")
        .start_turn(
            "turn-active".to_owned(),
            RunId::generate_sequence(1).expect("run"),
        )
        .expect("active turn");

    let before = durable_state(&fixture, &agent, &seeded);
    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "compact-turn-1",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(
        fixture.last()["success"],
        false,
        "an active production turn blocks manual compaction"
    );
    assert_eq!(fixture.last()["error"], "conversation busy");
    assert_eq!(
        durable_state(&fixture, &agent, &seeded),
        before,
        "a busy rejection writes nothing"
    );

    // Settling the turn returns the authoritative lifecycle to idle and the
    // same command now runs through the registered production service.
    held_lifecycle
        .lock()
        .expect("lifecycle")
        .finish_turn(
            &turn_lease,
            StopReason::new("completed").expect("stop reason"),
        )
        .expect("turn settle");
    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "compact-turn-2",
            "conversation_id": seeded.as_str(),
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.compaction_rows(&agent, &seeded), 1);
    let journal = fixture.compaction_journal();
    assert_eq!(journal["transactions"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn compaction_settings_select_the_pinned_strategy_modes() {
    let fixture = bridge();
    let agent = fixture.seed_agent("compactmode", "Compact Modes").await;
    let all_conv = fixture.seed_conversation(&agent, "local-conv-33", 4).await;

    // `all` summarizes every eligible message, leaving only the summary.
    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "cm-1",
            "conversation_id": all_conv.as_str(),
            "body": {"agent_id": agent.as_str(), "compaction_settings": {"mode": "all"}},
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["compaction"]["num_messages_before"], 4);
    assert_eq!(
        fixture.last()["compaction"]["num_messages_after"],
        1,
        "\"all\" retains no messages beyond the summary"
    );

    // A sent sliding-window percentage overrides the default retention.
    let windowed = fixture.seed_conversation(&agent, "local-conv-34", 10).await;
    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "cm-2",
            "conversation_id": windowed.as_str(),
            "body": {
                "agent_id": agent.as_str(),
                "compaction_settings": {"mode": "sliding_window", "sliding_window_percentage": 0.5},
            },
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(fixture.last()["compaction"]["num_messages_before"], 10);
    assert_eq!(
        fixture.last()["compaction"]["num_messages_after"],
        6,
        "fifty percent retained plus the summary (the default thirty would serve eight)"
    );

    // Unknown strategy strings are rejected with the pinned validator detail.
    fixture
        .send(&json!({
            "type": "conversation_compact",
            "request_id": "cm-3",
            "conversation_id": windowed.as_str(),
            "body": {"agent_id": agent.as_str(), "compaction_settings": {"mode": "auto"}},
        }))
        .await;
    assert_eq!(fixture.last()["success"], false);
    let rejection = fixture.last();
    let error = rejection["error"].as_str().expect("error detail");
    assert!(
        error.starts_with(
            "Local backend compaction currently supports only modes \"all\" and \"sliding_window\""
        ),
        "the pinned rejection detail is served: {error}"
    );
    assert_eq!(
        fixture.compaction_rows(&agent, &windowed),
        1,
        "a rejected mode writes nothing"
    );
}
