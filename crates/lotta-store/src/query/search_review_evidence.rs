use super::{SearchHit, TranscriptSearch};
use crate::query::test_support::{Fixture, file_snapshot, message_id};
use crate::query::{QUERY_PROJECTED_MESSAGES_MAX, SourceMessageKey};
use crate::transcript::projection::REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX;
use lotta_domain::{BoundedVec, LocalMessage, LocalMessageRole};
use lotta_runtime::ports::ConversationStore;
use serde_json::{Value, json};
use std::io::Write as _;
use std::path::Path;

pub(crate) async fn search_blockers_body(fixture: &Fixture, path: &Path) {
    structural_rows(fixture, path).await;
    searchable_surface(fixture, path).await;
}

async fn structural_rows(fixture: &Fixture, path: &Path) {
    append_rows(
        path,
        &[
            json!({
                "type":"extension",
                "id":"extension-entry",
                "parentId":null,
                "timestamp":"2026-01-01T00:00:20Z",
                "message":{
                    "id":"ui-msg-90001",
                    "role":"user",
                    "content":[{"type":"text","text":"extension-secret"}],
                    "timestamp":20_000.0
                }
            })
            .to_string(),
            "{terminated-malformed-json".into(),
        ],
    );
    assert_search(fixture, path, "extension-secret", false).await;
    assert_search(fixture, path, "needle a", true).await;
}

async fn searchable_surface(fixture: &Fixture, path: &Path) {
    let assistant = current_row(
        "entry-review-assistant",
        &json!({
            "id":"ui-msg-90002",
            "role":"assistant",
            "content":[
                {"type":"thinking","thinking":"reasoning-surface-secret"},
                {
                    "type":"text",
                    "text":"assistant-surface-secret",
                    "extension":"content-part-extension-secret"
                },
                {
                    "type":"toolCall",
                    "id":"call-review",
                    "name":"allowed-tool-name",
                    "arguments":{"path":"allowed-arguments-secret"},
                    "extension":"extension-field-secret"
                }
            ],
            "timestamp":21_000.0,
            "metadata":{"trace":"metadata-secret"},
            "extension":"message-extension-secret"
        }),
    );
    let tool_result = current_row(
        "entry-review-result",
        &json!({
            "id":"ui-msg-90003",
            "role":"toolResult",
            "content":[{
                "type":"text",
                "text":"tool-return-secret",
                "extension":"tool-part-extension-secret"
            }],
            "timestamp":22_000.0,
            "toolCallId":"call-review",
            "isError":false,
            "metadata":{"trace":"tool-metadata-secret"}
        }),
    );
    let user = current_row(
        "entry-review-user",
        &json!({
            "id":"ui-msg-90004",
            "role":"user",
            "content":[{
                "type":"text",
                "text":"user-content-secret",
                "extension":"user-part-extension-secret"
            }],
            "timestamp":23_000.0,
            "metadata":{"trace":"user-metadata-secret"}
        }),
    );
    append_rows(path, &[assistant, tool_result, user]);
    append_context(fixture, ["ui-msg-90002", "ui-msg-90003", "ui-msg-90004"]).await;
    for query in [
        "allowed-tool-name",
        "allowed-arguments-secret",
        "reasoning-surface-secret",
        "assistant-surface-secret",
        "tool-return-secret",
        "user-content-secret",
    ] {
        assert_search(fixture, path, query, true).await;
    }
    for query in [
        "extension-field-secret",
        "content-part-extension-secret",
        "metadata-secret",
        "message-extension-secret",
        "tool-metadata-secret",
        "user-part-extension-secret",
        "user-metadata-secret",
    ] {
        assert_search(fixture, path, query, false).await;
    }
    active_projection_parity(fixture, path).await;
}

async fn active_projection_parity(fixture: &Fixture, path: &Path) {
    append_rows(path, &parity_rows(&oversized_result()));
    append_context(
        fixture,
        [
            "ui-msg-90005",
            "ui-msg-90006",
            "ui-msg-90007",
            "ui-msg-90008",
            "ui-msg-90009",
        ],
    )
    .await;
    let paths = crate::transcript::transcript_paths(
        fixture.store.paths(),
        &fixture.agents[0],
        &fixture.conversations[0],
    )
    .expect("parity paths");
    let active =
        crate::transcript::load::load_search_nonmutating(&paths, QUERY_PROJECTED_MESSAGES_MAX)
            .expect("Task25 active search projection");
    assert_active_orphan_parity(&active);
    assert_active_clipping(&active);
    assert_public_parity(fixture, path, &active).await;
}

fn parity_rows(large: &str) -> Vec<String> {
    vec![
        tool_call_row(
            "entry-match-assistant",
            "ui-msg-90005",
            "call-match",
            24_000.0,
        ),
        tool_result_row(
            "entry-match-result",
            "ui-msg-90006",
            "call-match",
            "matched-tool-secret",
            25_000.0,
        ),
        tool_result_row(
            "entry-orphan-result",
            "ui-msg-90007",
            "call-orphan",
            "orphan-tool-secret",
            26_000.0,
        ),
        tool_call_row(
            "entry-large-assistant",
            "ui-msg-90008",
            "call-large",
            27_000.0,
        ),
        tool_result_row(
            "entry-large-result",
            "ui-msg-90009",
            "call-large",
            large,
            28_000.0,
        ),
    ]
}

fn tool_call_row(entry: &str, id: &str, call: &str, timestamp: f64) -> String {
    current_row(
        entry,
        &json!({
            "id":id,
            "role":"assistant",
            "content":[{
                "type":"toolCall",
                "id":call,
                "name":"allowed-parity-tool",
                "arguments":{"path":"allowed-parity-arguments"}
            }],
            "timestamp":timestamp
        }),
    )
}

fn tool_result_row(entry: &str, id: &str, call: &str, text: &str, timestamp: f64) -> String {
    current_row(
        entry,
        &json!({
            "id":id,
            "role":"toolResult",
            "content":[{"type":"text","text":text}],
            "timestamp":timestamp,
            "toolCallId":call,
            "isError":false
        }),
    )
}

fn oversized_result() -> String {
    let flank = REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX;
    format!(
        "HEAD-retained-secret{}OMITTED-MIDDLE-SECRET{}TAIL-retained-secret",
        "a".repeat(flank),
        "b".repeat(flank)
    )
}

async fn append_context<const N: usize>(fixture: &Fixture, ids: [&str; N]) {
    let mut conversation = fixture
        .store
        .query_conversation(&fixture.agents[0], &fixture.conversations[0])
        .await
        .expect("review conversation");
    let mut context = conversation.in_context_message_ids.as_slice().to_vec();
    context.extend(ids.map(message_id));
    conversation.in_context_message_ids = BoundedVec::new(context).expect("review context");
    ConversationStore::save(&fixture.store, &conversation)
        .await
        .expect("save review context");
}

fn assert_active_orphan_parity(active: &[LocalMessage]) {
    let ids: Vec<_> = active.iter().map(|message| message.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "ui-msg-10001",
            "ui-msg-10002",
            "ui-msg-90002",
            "ui-msg-90003",
            "ui-msg-90004",
            "ui-msg-90005",
            "ui-msg-90006",
            "ui-msg-90008",
            "ui-msg-90009",
        ]
    );
}

fn assert_active_clipping(active: &[LocalMessage]) {
    let result = active
        .iter()
        .find(|message| message.id.as_str() == "ui-msg-90009")
        .expect("clipped active result");
    assert_eq!(result.role, LocalMessageRole::ToolResult);
    let text = message_text(result);
    assert!(text.encode_utf16().count() <= REPAIRED_TOOL_RESULT_TEXT_CHARS_MAX);
    assert!(text.contains("HEAD-retained-secret"));
    assert!(text.contains("TAIL-retained-secret"));
    assert!(text.contains("Tool result truncated during local transcript repair"));
    assert!(!text.contains("OMITTED-MIDDLE-SECRET"));
}

async fn assert_public_parity(fixture: &Fixture, path: &Path, active: &[LocalMessage]) {
    let active_ids: std::collections::BTreeSet<_> =
        active.iter().map(|message| message.id.clone()).collect();
    assert_hit_sources(
        fixture,
        path,
        "matched-tool-secret",
        &active_ids,
        "ui-msg-90006",
    )
    .await;
    assert_search(fixture, path, "orphan-tool-secret", false).await;
    for query in [
        "HEAD-retained-secret",
        "TAIL-retained-secret",
        "Tool result truncated during local transcript repair",
    ] {
        assert_hit_sources(fixture, path, query, &active_ids, "ui-msg-90009").await;
    }
    assert_search(fixture, path, "OMITTED-MIDDLE-SECRET", false).await;
}

async fn assert_hit_sources(
    fixture: &Fixture,
    path: &Path,
    query: &str,
    active_ids: &std::collections::BTreeSet<lotta_domain::MessageId>,
    expected: &str,
) {
    let before = file_snapshot(path);
    let hits = search(fixture, query).await;
    assert!(!hits.is_empty(), "query {query}");
    assert!(
        hits.iter()
            .all(|hit| active_ids.contains(&hit.source.source_id))
    );
    assert_eq!(
        hits.iter()
            .map(|hit| &hit.source)
            .collect::<Vec<&SourceMessageKey>>(),
        hits.iter()
            .filter(|hit| hit.source.source_id.as_str() == expected)
            .map(|hit| &hit.source)
            .collect::<Vec<&SourceMessageKey>>()
    );
    assert_eq!(before, file_snapshot(path), "query mutated {query}");
}

fn message_text(message: &LocalMessage) -> &str {
    message
        .content
        .as_ref()
        .and_then(|content| content.as_value().as_array())
        .and_then(|parts| parts.first())
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .expect("tool-result text")
}

fn current_row(id: &str, message: &Value) -> String {
    json!({
        "type":"message",
        "id":id,
        "parentId":null,
        "timestamp":"2026-01-01T00:00:20Z",
        "message":message
    })
    .to_string()
}

fn append_rows(path: &Path, rows: &[String]) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("append search review rows");
    for row in rows {
        file.write_all(row.as_bytes()).expect("write review row");
        file.write_all(b"\n").expect("terminate review row");
    }
    file.sync_all().expect("sync review rows");
}

async fn assert_search(fixture: &Fixture, path: &Path, query: &str, expected: bool) {
    let before = file_snapshot(path);
    let hits = search(fixture, query).await;
    assert_eq!(!hits.is_empty(), expected, "query {query}");
    assert_eq!(before, file_snapshot(path), "query mutated {query}");
}

async fn search(fixture: &Fixture, query: &str) -> Vec<SearchHit> {
    fixture
        .store
        .query_transcript_search(TranscriptSearch {
            query: query.into(),
            agent_id: Some(fixture.agents[0].clone()),
            conversation_id: Some(fixture.conversations[0].clone()),
            include_hidden: false,
            limit: 10,
        })
        .await
        .expect("review search")
}
