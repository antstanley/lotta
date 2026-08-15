use super::test_support::*;
use super::*;
use std::collections::BTreeSet;
use std::path::Path;

pub(crate) async fn cursor_real_body(fixture: &Fixture) {
    let extra = ["cursor-a", "cursor-b", "cursor-c"].map(conversation_id);
    save_conversation(
        &fixture.store,
        &fixture.agents[0],
        &extra[0],
        false,
        false,
        7,
    )
    .await;
    save_conversation(
        &fixture.store,
        &fixture.agents[0],
        &extra[1],
        false,
        false,
        7,
    )
    .await;
    save_conversation(
        &fixture.store,
        &fixture.agents[0],
        &extra[2],
        false,
        false,
        6,
    )
    .await;
    let query = || async {
        fixture
            .store
            .query_conversations(
                &fixture.agents[0],
                ConversationFilters::default(),
                PageRequest::default(),
            )
            .await
            .expect("full visible conversations")
    };
    let full = query().await;
    let repeat = query().await;
    let expected: Vec<_> = full.items.iter().map(|value| value.id.clone()).collect();
    assert!(expected.len() >= 4);
    assert_eq!(full.items, repeat.items);
    let mut visited = Vec::new();
    let mut cursor = None;
    loop {
        let page = fixture
            .store
            .query_conversations(
                &fixture.agents[0],
                ConversationFilters::default(),
                PageRequest {
                    limit: Some(1),
                    after: cursor,
                },
            )
            .await
            .expect("one-item page");
        visited.extend(page.items.into_iter().map(|value| value.id));
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(visited, expected);
    assert_eq!(
        visited.iter().cloned().collect::<BTreeSet<_>>().len(),
        visited.len()
    );
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn message_options_exact_body(fixture: &Fixture) {
    async fn ids(fixture: &Fixture, options: MessageListOptions) -> Vec<MessageId> {
        fixture
            .store
            .query_messages_for_conversation(&fixture.agents[0], &fixture.conversations[0], options)
            .await
            .expect("message options")
            .into_iter()
            .map(|value| value.id)
            .collect()
    }
    let full_asc = ids(
        fixture,
        MessageListOptions {
            order: MessageOrder::Ascending,
            ..Default::default()
        },
    )
    .await;
    assert!(full_asc.len() >= 4);
    let desc = ids(fixture, MessageListOptions::default()).await;
    assert_eq!(desc, full_asc.iter().rev().cloned().collect::<Vec<_>>());

    let projected = fixture
        .store
        .query_messages_for_conversation(
            &fixture.agents[0],
            &fixture.conversations[0],
            MessageListOptions {
                order: MessageOrder::Ascending,
                ..Default::default()
            },
        )
        .await
        .expect("typed full");
    let excluded = projected[0].message_type;
    let included = projected
        .iter()
        .find(|value| value.message_type != excluded)
        .expect("different type")
        .message_type;
    let filtered: Vec<_> = projected
        .iter()
        .filter(|value| value.message_type == included)
        .map(|value| value.id.clone())
        .collect();
    let filtered_cursor = ids(
        fixture,
        MessageListOptions {
            order: MessageOrder::Ascending,
            return_types: vec![included],
            before: Some(projected[0].id.clone()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(filtered_cursor, filtered);

    let before_index = 3;
    let after_index = 1;
    assert_eq!(
        ids(
            fixture,
            MessageListOptions {
                order: MessageOrder::Ascending,
                before: Some(full_asc[before_index].clone()),
                ..Default::default()
            }
        )
        .await,
        full_asc[..before_index]
    );
    assert_eq!(
        ids(
            fixture,
            MessageListOptions {
                order: MessageOrder::Ascending,
                after: Some(full_asc[after_index].clone()),
                ..Default::default()
            }
        )
        .await,
        full_asc[after_index + 1..]
    );
    assert_eq!(
        ids(
            fixture,
            MessageListOptions {
                order: MessageOrder::Ascending,
                before: Some(full_asc[before_index].clone()),
                after: Some(full_asc[after_index].clone()),
                ..Default::default()
            }
        )
        .await,
        full_asc[after_index + 1..before_index]
    );
    assert_eq!(
        ids(
            fixture,
            MessageListOptions {
                before: Some(full_asc[before_index].clone()),
                ..Default::default()
            }
        )
        .await,
        full_asc[..before_index]
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        ids(
            fixture,
            MessageListOptions {
                order: MessageOrder::Ascending,
                limit: Some(2),
                ..Default::default()
            }
        )
        .await,
        full_asc[..2]
    );
    assert_eq!(
        ids(
            fixture,
            MessageListOptions {
                order: MessageOrder::Ascending,
                before: Some(message_id("letta-msg-999999999")),
                ..Default::default()
            }
        )
        .await,
        full_asc
    );
}

pub(crate) fn projection_exact_body() {
    let agent = agent_id("agent-local-review");
    let conversation = conversation_id("review");
    let path = Path::new("review-evidence");
    let max = QUERY_PROJECTED_MESSAGES_MAX;
    let source = vec![projection_message("ui-msg-77", max, 1_000.0)];
    let projected =
        super::projection_impl::messages(&source, &agent, &conversation, path).expect("exact max");
    let numbers: Vec<_> = projected
        .iter()
        .map(|value| projected_number(&value.id))
        .collect();
    let slots = u64::try_from(max).expect("slots fit u64");
    let start = (77_u64 - 1) * slots + 1;
    assert_eq!(numbers, (start..=77 * slots).collect::<Vec<_>>());
    assert_eq!(numbers.iter().copied().collect::<BTreeSet<_>>().len(), max);
    assert!(numbers.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        super::projection_impl::messages(
            &[projection_message("ui-msg-77", max + 1, 1.0)],
            &agent,
            &conversation,
            path
        )
        .expect_err("max plus one")
        .kind(),
        StoreErrorKind::Limit
    );

    let highest = (u64::MAX / 2) / slots;
    let accepted = super::projection_impl::messages(
        &[projection_message(&format!("ui-msg-{highest}"), 1, 1.0)],
        &agent,
        &conversation,
        path,
    )
    .expect("highest sequence");
    assert_eq!(projected_number(&accepted[0].id), (highest - 1) * slots + 1);
    assert!(projected_number(&accepted[0].id) <= u64::MAX / 2);
    assert_eq!(
        super::projection_impl::messages(
            &[projection_message(
                &format!("ui-msg-{}", highest + 1),
                1,
                1.0
            )],
            &agent,
            &conversation,
            path
        )
        .expect_err("next sequence")
        .kind(),
        StoreErrorKind::Limit
    );

    chronology_body(&agent, path);
    fallback_collision_body(&agent, path);
}

fn chronology_body(agent: &AgentId, path: &Path) {
    let scopes = [
        conversation_id("chron-a"),
        conversation_id("chron-b"),
        conversation_id("chron-c"),
    ];
    let sources = [
        vec![projection_message("ui-msg-101", 9, 1_000.0)],
        vec![projection_message("ui-msg-102", 1, 1_001.0)],
        vec![projection_message("ui-msg-103", 2, 1_001.0)],
    ];
    let mut merged = Vec::new();
    for (scope, source) in scopes.iter().zip(sources.iter()) {
        merged.extend(
            super::projection_impl::messages(source, agent, scope, path)
                .expect("chronology projection"),
        );
    }
    merged.sort_by(super::message_order);
    assert!(
        merged[..9]
            .iter()
            .all(|value| value.source.source_id == message_id("ui-msg-101")
                && value.timestamp_ms.to_bits() == 1_000.0_f64.to_bits())
    );
    assert!(
        merged[9..]
            .iter()
            .all(|value| value.timestamp_ms.to_bits() == 1_001.0_f64.to_bits())
    );
    assert_eq!(merged[9].source.conversation_id, scopes[1]);
    assert_eq!(merged[10].source.conversation_id, scopes[2]);
    assert_eq!(merged[10].projection_ordinal, 0);
    assert_eq!(merged[11].projection_ordinal, 1);
}

fn fallback_collision_body(agent: &AgentId, path: &Path) {
    let left_scope = conversation_id("legacy-left");
    let right_scope = conversation_id("legacy-right");
    let left_source = vec![projection_message("legacy-left-source", 1, 1.0)];
    let right_source = vec![projection_message("legacy-right-source", 1, 2.0)];
    let left_snapshot = left_source.clone();
    let right_snapshot = right_source.clone();
    let mut collision = super::projection_impl::messages_with_fallback_group(
        &left_source,
        agent,
        &left_scope,
        path,
        41,
    )
    .expect("left forced group");
    collision.extend(
        super::projection_impl::messages_with_fallback_group(
            &right_source,
            agent,
            &right_scope,
            path,
            41,
        )
        .expect("right forced group"),
    );
    assert_eq!(
        super::ensure_unique_projection_ids(&collision)
            .expect_err("forced collision")
            .kind(),
        StoreErrorKind::StorageConflict
    );
    let mut distinct = super::projection_impl::messages_with_fallback_group(
        &left_source,
        agent,
        &left_scope,
        path,
        41,
    )
    .expect("left group");
    distinct.extend(
        super::projection_impl::messages_with_fallback_group(
            &right_source,
            agent,
            &right_scope,
            path,
            42,
        )
        .expect("different group"),
    );
    super::ensure_unique_projection_ids(&distinct).expect("different groups accepted");
    assert_eq!(left_source, left_snapshot);
    assert_eq!(right_source, right_snapshot);
}
