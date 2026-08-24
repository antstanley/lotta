//! `ws::conversations::messages` — one case per §Required query patterns
//! parameter (asc/desc order, `before`, `after`, limit, return-message-type
//! filter) plus a combined case proves each parameter changes the served page.

use serde_json::{Value, json};

use super::support::{TestConversations, bridge};

/// Fixture with five single-projection messages; returns ascending ids.
async fn seeded(
    fixture: &TestConversations,
) -> (
    lotta_domain::AgentId,
    lotta_domain::ConversationId,
    Vec<String>,
) {
    let agent = fixture.seed_agent("msgpage", "Message Page").await;
    let conversation = fixture.seed_conversation(&agent, "local-conv-40", 5).await;
    let ids = fixture.projected_ids(&agent, &conversation).await;
    (agent, conversation, ids)
}

fn served_dates(frame: &Value) -> Vec<&str> {
    frame["messages"]
        .as_array()
        .expect("page")
        .iter()
        .map(|message| message["date"].as_str().expect("date"))
        .collect()
}

#[tokio::test]
async fn asc_and_desc_orders_flip_the_served_sequence() {
    let fixture = bridge();
    let (agent, conversation, _ids) = seeded(&fixture).await;
    let base = json!({
        "type": "conversation_messages_list",
        "conversation_id": conversation.as_str(),
        "query": {"agent_id": agent.as_str()},
    });

    let mut desc_command = base.clone();
    desc_command["request_id"] = json!("ord-desc");
    fixture.send(&desc_command).await;
    assert_eq!(fixture.last()["success"], true);
    let desc_frame = fixture.last();
    let desc = served_dates(&desc_frame);

    let mut query = base.clone();
    query["query"]["order"] = json!("asc");
    query["request_id"] = json!("ord-asc");
    fixture.send(&query).await;
    assert_eq!(fixture.last()["success"], true);
    let asc_frame = fixture.last();
    let asc = served_dates(&asc_frame);

    assert_eq!(desc.len(), 5);
    assert_eq!(asc.len(), 5);
    assert!(
        desc.windows(2).all(|pair| pair[0] >= pair[1]),
        "descending serves newest first"
    );
    assert!(
        asc.windows(2).all(|pair| pair[0] <= pair[1]),
        "ascending serves oldest first"
    );
    assert_eq!(asc.first(), desc.last());
}

#[tokio::test]
async fn before_cursor_excludes_the_cursor_and_everything_newer() {
    let fixture = bridge();
    let (agent, conversation, ids) = seeded(&fixture).await;
    fixture
        .send(&json!({
            "type": "conversation_messages_list",
            "request_id": "before-1",
            "conversation_id": conversation.as_str(),
            "query": {"agent_id": agent.as_str(), "before": ids[3]},
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    let served = fixture.last()["messages"]
        .as_array()
        .expect("page")
        .iter()
        .map(|message| message["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    // Descending over the strictly older prefix of the cursor.
    assert_eq!(served, vec![ids[2].clone(), ids[1].clone(), ids[0].clone()]);
}

#[tokio::test]
async fn after_cursor_excludes_the_cursor_and_everything_older() {
    let fixture = bridge();
    let (agent, conversation, ids) = seeded(&fixture).await;
    fixture
        .send(&json!({
            "type": "conversation_messages_list",
            "request_id": "after-1",
            "conversation_id": conversation.as_str(),
            "query": {"agent_id": agent.as_str(), "after": ids[1], "limit": 10},
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    let served = fixture.last()["messages"]
        .as_array()
        .expect("page")
        .iter()
        .map(|message| message["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    // Descending over the strictly newer suffix beyond the cursor.
    assert_eq!(served, vec![ids[4].clone(), ids[3].clone(), ids[2].clone()]);
}

#[tokio::test]
async fn limit_caps_the_page_and_reports_whether_more_remain() {
    let fixture = bridge();
    let (agent, conversation, _ids) = seeded(&fixture).await;
    fixture
        .send(&json!({
            "type": "conversation_messages_list",
            "request_id": "limit-1",
            "conversation_id": conversation.as_str(),
            "query": {"agent_id": agent.as_str(), "limit": 2},
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let messages = frame["messages"].as_array().expect("page");
    assert_eq!(messages.len(), 2);
    assert_eq!(frame["has_more"], true, "three more remain");
    let next_before = frame["next_before"].as_str().expect("cursor");
    let second_served = messages[1]["id"].as_str().expect("id");
    assert_eq!(next_before, second_served);

    // A bound larger than the history reports no further pages.
    fixture
        .send(&json!({
            "type": "conversation_messages_list",
            "request_id": "limit-2",
            "conversation_id": conversation.as_str(),
            "query": {"agent_id": agent.as_str(), "limit": 50},
        }))
        .await;
    assert_eq!(fixture.last()["has_more"], false);
}

#[tokio::test]
async fn include_return_message_types_filters_categories() {
    let fixture = bridge();
    let (agent, conversation, _ids) = seeded(&fixture).await;

    fixture
        .send(&json!({
            "type": "conversation_messages_list",
            "request_id": "types-1",
            "conversation_id": conversation.as_str(),
            "query": {
                "agent_id": agent.as_str(),
                "include_return_message_types": ["user_message"],
                "order": "asc",
            },
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let served = frame["messages"].as_array().expect("page");
    assert_eq!(served.len(), 3, "only the three user turns remain");
    assert!(
        served
            .iter()
            .all(|message| message["message_type"] == "user_message")
    );

    fixture
        .send(&json!({
            "type": "conversation_messages_list",
            "request_id": "types-2",
            "conversation_id": conversation.as_str(),
            "query": {
                "agent_id": agent.as_str(),
                "include_return_message_types": ["reasoning_message"],
            },
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(
        fixture.last()["messages"].as_array().expect("page").len(),
        0,
        "text-only seeds carry no reasoning category"
    );
    assert_eq!(
        fixture.last()["next_before"],
        json!(null),
        "an empty page has no cursor"
    );
}

#[tokio::test]
async fn combined_parameters_apply_together() {
    let fixture = bridge();
    let (agent, conversation, ids) = seeded(&fixture).await;
    fixture
        .send(&json!({
            "type": "conversation_messages_list",
            "request_id": "combo-1",
            "conversation_id": conversation.as_str(),
            "query": {
                "agent_id": agent.as_str(),
                "order": "asc",
                "after": ids[1],
                "before": ids[3],
                "limit": 2,
            },
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let served = frame["messages"].as_array().expect("page");
    // Task 28 applies the exclusive cursors in sequence: `before` truncates
    // at the cursor's position and `after` drains through its own, so the
    // intersection of the two bounds is the single middle turn.
    assert_eq!(
        served
            .iter()
            .map(|m| m["id"].as_str().expect("id"))
            .collect::<Vec<_>>(),
        vec![ids[2].as_str()],
    );
    assert_eq!(frame["has_more"], false);
}

/// Golden wire shape of one served stored-message object: flattened identity
/// fields plus the pinned `user_message` payload, byte-for-byte field parity
/// with the pinned protocol.
#[tokio::test]
async fn served_user_messages_match_the_pinned_golden_shape() {
    let fixture = bridge();
    let (agent, conversation, ids) = seeded(&fixture).await;
    fixture
        .send(&json!({
            "type": "conversation_messages_list",
            "request_id": "gold-1",
            "conversation_id": conversation.as_str(),
            "query": {"agent_id": agent.as_str(), "order": "asc", "limit": 1},
        }))
        .await;
    assert_eq!(fixture.last()["success"], true);
    let frame = fixture.last();
    let served = frame["messages"].as_array().expect("page");
    assert_eq!(served.len(), 1);
    assert_eq!(
        served[0],
        json!({
            "id": ids[0],
            "date": "1970-01-01T00:00:01.000Z",
            "agent_id": agent.as_str(),
            "conversation_id": conversation.as_str(),
            "message_type": "user_message",
            "role": "user",
            "content": [{"type": "text", "text": "seed message 0"}],
        }),
        "the served projection matches the pinned stored-message golden"
    );
}

/// Golden translations from Task 28 projected categories into the pinned
/// message-type payloads (`reasoning`, `approval_request`, `tool_return`,
/// and `summary`), including the JSON-stringified tool-call arguments.
#[test]
fn pinned_translations_match_the_golden_category_shapes() {
    use lotta_store::query::{ProjectedMessage, ReturnMessageType, SourceMessageKey};

    let agent = lotta_domain::AgentId::accept("agent-local-golden").expect("agent id");
    let conversation =
        lotta_domain::ConversationId::accept("local-conv-91").expect("conversation id");
    let projected = |message_type: ReturnMessageType, value: Value| ProjectedMessage {
        id: lotta_domain::MessageId::generate_projection(7).expect("projection id"),
        source: SourceMessageKey {
            agent_id: agent.clone(),
            conversation_id: conversation.clone(),
            source_id: lotta_domain::MessageId::accept("ui-msg-9").expect("source id"),
            source_ordinal: 3,
        },
        projection_ordinal: 0,
        message_type,
        timestamp_ms: 1_500.0,
        value,
    };
    let identity = json!({
        "id": "letta-msg-7",
        "date": "1970-01-01T00:00:01.500Z",
        "agent_id": agent.as_str(),
        "conversation_id": conversation.as_str(),
    });

    for (projected, golden_payload) in golden_translation_cases(&projected) {
        let mut expected = identity.clone();
        for (key, value) in golden_payload.as_object().expect("golden object") {
            expected[key] = value.clone();
        }
        let translated = super::pinned_message(&projected).expect("translation");
        let encoded = serde_json::to_value(&translated).expect("encodes");
        let label = encoded["message_type"].as_str().unwrap_or_default();
        assert_eq!(
            encoded, expected,
            "the {label} translation matches its golden shape"
        );
    }
}

/// One case per pinned category payload beyond user/assistant: the projected
/// value plus its exact golden typed-payload fields.
fn golden_translation_cases(
    projected: &dyn Fn(
        lotta_store::query::ReturnMessageType,
        Value,
    ) -> lotta_store::query::ProjectedMessage,
) -> Vec<(lotta_store::query::ProjectedMessage, Value)> {
    use lotta_store::query::ReturnMessageType;

    vec![
        (
            projected(
                ReturnMessageType::Reasoning,
                json!({"reasoning": "thinking hard"}),
            ),
            json!({"message_type": "reasoning_message", "reasoning": "thinking hard"}),
        ),
        (
            projected(
                ReturnMessageType::ApprovalRequest,
                json!({
                    "type": "toolCall", "id": "call-1", "name": "shell",
                    "arguments": {"cmd": ["ls"]},
                }),
            ),
            json!({
                "message_type": "approval_request_message",
                "tool_call": {
                    "tool_call_id": "call-1",
                    "name": "shell",
                    "arguments": "{\"cmd\":[\"ls\"]}",
                },
            }),
        ),
        (
            projected(
                ReturnMessageType::ToolReturn,
                json!({
                    "tool_call_id": "call-1",
                    "tool_return": {"output": "ok"}, "is_error": true,
                }),
            ),
            json!({
                "message_type": "tool_return_message",
                "tool_call_id": "call-1",
                "status": "error",
                "tool_return": {"output": "ok"},
            }),
        ),
        (
            projected(ReturnMessageType::Summary, json!({"summary": "settled"})),
            json!({"message_type": "summary_message", "summary": "settled"}),
        ),
    ]
}
