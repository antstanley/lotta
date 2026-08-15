use super::test_support::*;
use super::*;
use crate::StoreErrorKind;
use lotta_domain::{BoundedVec, NonEmptyString, RuntimeConnection, RuntimeScope};
use std::collections::BTreeSet;

pub(crate) async fn wrong_prefix_404_body() {
    let fixture = fixture("query-agent").await;
    let found = fixture
        .store
        .query_agent(fixture.agents[0].as_str())
        .await
        .expect("direct");
    assert_eq!(found.id, fixture.agents[0]);
    for value in [
        "agent-local-absent",
        "agent-cloud-x",
        "../agent-local-a",
        "agent-local-a/../../escape",
        "agent-local-",
    ] {
        let error = fixture
            .store
            .query_agent(value)
            .await
            .expect_err("not found");
        assert_eq!(error.kind(), StoreErrorKind::NotFound);
        assert!(!error.to_string().contains(value));
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn deterministic_order_body() {
    let fixture = fixture("query-agents").await;
    let visible = fixture
        .store
        .query_agents(AgentFilters::default(), PageRequest::default())
        .await
        .expect("visible");
    assert_eq!(ids(&visible.items), vec!["agent-local-a"]);
    assert_eq!(
        visible,
        fixture
            .store
            .query_agents(AgentFilters::default(), PageRequest::default())
            .await
            .expect("repeat")
    );
    for (filters, expected) in [
        (
            AgentFilters {
                name: Some(("Alpha Pony".into(), NameMatch::Exact)),
                ..AgentFilters::default()
            },
            vec!["agent-local-a"],
        ),
        (
            AgentFilters {
                name: Some(("PONY".into(), NameMatch::Substring)),
                ..AgentFilters::default()
            },
            vec!["agent-local-a"],
        ),
        (
            AgentFilters {
                query: Some("blue".into()),
                ..AgentFilters::default()
            },
            Vec::<&str>::new(),
        ),
        (
            AgentFilters {
                tags: vec!["x".into(), "common".into()],
                ..AgentFilters::default()
            },
            vec!["agent-local-a"],
        ),
        (
            AgentFilters {
                hidden: TriState::True,
                ..AgentFilters::default()
            },
            vec!["agent-local-b"],
        ),
        (
            AgentFilters {
                hidden: TriState::False,
                ..AgentFilters::default()
            },
            vec!["agent-local-a"],
        ),
    ] {
        let page = fixture
            .store
            .query_agents(filters, PageRequest::default())
            .await
            .expect("filter");
        assert_eq!(ids(&page.items), expected);
    }
    for limit in [QUERY_PAGE_ITEMS_MAX - 1, QUERY_PAGE_ITEMS_MAX] {
        assert!(
            fixture
                .store
                .query_agents(
                    AgentFilters::default(),
                    PageRequest {
                        limit: Some(limit),
                        after: None
                    }
                )
                .await
                .is_ok()
        );
    }
    for length in [QUERY_TAGS_MAX - 1, QUERY_TAGS_MAX] {
        assert!(
            fixture
                .store
                .query_agents(
                    AgentFilters {
                        tags: vec!["missing".into(); length],
                        ..AgentFilters::default()
                    },
                    PageRequest::default(),
                )
                .await
                .is_ok()
        );
    }
    assert_eq!(
        fixture
            .store
            .query_agents(
                AgentFilters {
                    tags: vec!["missing".into(); QUERY_TAGS_MAX + 1],
                    ..AgentFilters::default()
                },
                PageRequest::default(),
            )
            .await
            .expect_err("tags above")
            .kind(),
        StoreErrorKind::Limit
    );
    assert_eq!(
        fixture
            .store
            .query_agents(
                AgentFilters::default(),
                PageRequest {
                    limit: Some(QUERY_PAGE_ITEMS_MAX + 1),
                    after: None
                }
            )
            .await
            .expect_err("above")
            .kind(),
        StoreErrorKind::Limit
    );
}

pub(crate) async fn cursor_covers_all_body() {
    let fixture = fixture("query-conversations").await;
    super::review_evidence::cursor_real_body(&fixture).await;
    let all = fixture
        .store
        .query_conversations(
            &fixture.agents[0],
            ConversationFilters::default(),
            PageRequest::default(),
        )
        .await
        .expect("all");
    assert!(all.items.len() >= 4);
    let mut cursor = None;
    let mut visited = Vec::new();
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
            .expect("page");
        visited.extend(page.items.iter().map(|value| value.id.as_str().to_owned()));
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(visited.len(), all.items.len());
    assert_eq!(visited.iter().collect::<BTreeSet<_>>().len(), visited.len());
    let archived = fixture
        .store
        .query_conversations(
            &fixture.agents[0],
            ConversationFilters {
                archived: TriState::True,
                ..ConversationFilters::default()
            },
            PageRequest::default(),
        )
        .await
        .expect("archived");
    assert!(archived.items.is_empty());
    let hidden = fixture
        .store
        .query_conversations(
            &fixture.agents[0],
            ConversationFilters {
                hidden: TriState::True,
                tags: vec!["hidden".into()],
                ..ConversationFilters::default()
            },
            PageRequest::default(),
        )
        .await
        .expect("hidden");
    assert_eq!(hidden.items.len(), 1);
    let error = fixture
        .store
        .query_conversations(
            &fixture.agents[0],
            ConversationFilters::default(),
            PageRequest {
                limit: Some(1),
                after: Some(Cursor::new("unknown".into())),
            },
        )
        .await
        .expect_err("cursor");
    assert_eq!(error.kind(), StoreErrorKind::NotFound);
}

pub(crate) async fn scope_isolation_body() {
    let fixture = fixture("query-scope").await;
    for conversation in [&fixture.conversations[0]] {
        let left = fixture
            .store
            .query_conversation(&fixture.agents[0], conversation)
            .await
            .expect("left");
        let right = fixture
            .store
            .query_conversation(&fixture.agents[1], conversation)
            .await
            .expect("right");
        assert_eq!(left.agent_id, fixture.agents[0]);
        assert_eq!(right.agent_id, fixture.agents[1]);
    }
    let absent = conversation_id("absent");
    assert_eq!(
        fixture
            .store
            .query_conversation(&fixture.agents[0], &absent)
            .await
            .expect_err("absent")
            .kind(),
        StoreErrorKind::NotFound
    );
}

pub(crate) async fn messages_for_conversation_body() {
    let fixture = fixture("query-messages-conversation").await;
    super::review_evidence::message_options_exact_body(&fixture).await;
    let asc = fixture
        .store
        .query_messages_for_conversation(
            &fixture.agents[0],
            &fixture.conversations[0],
            MessageListOptions {
                order: MessageOrder::Ascending,
                ..MessageListOptions::default()
            },
        )
        .await
        .expect("asc");
    let desc = fixture
        .store
        .query_messages_for_conversation(
            &fixture.agents[0],
            &fixture.conversations[0],
            MessageListOptions::default(),
        )
        .await
        .expect("desc");
    assert_eq!(
        asc.iter().rev().map(|value| &value.id).collect::<Vec<_>>(),
        desc.iter().map(|value| &value.id).collect::<Vec<_>>()
    );
    assert!(asc.len() >= 4);
    let cursor = asc[1].id.clone();
    let before = fixture
        .store
        .query_messages_for_conversation(
            &fixture.agents[0],
            &fixture.conversations[0],
            MessageListOptions {
                order: MessageOrder::Ascending,
                before: Some(cursor.clone()),
                ..MessageListOptions::default()
            },
        )
        .await
        .expect("before");
    let after = fixture
        .store
        .query_messages_for_conversation(
            &fixture.agents[0],
            &fixture.conversations[0],
            MessageListOptions {
                order: MessageOrder::Ascending,
                after: Some(cursor),
                limit: Some(1),
                return_types: vec![
                    ReturnMessageType::Assistant,
                    ReturnMessageType::ApprovalRequest,
                ],
                ..MessageListOptions::default()
            },
        )
        .await
        .expect("after");
    assert_eq!(before.len(), 1);
    assert_eq!(after.len(), 1);
    assert!(matches!(
        after[0].message_type,
        ReturnMessageType::Assistant | ReturnMessageType::ApprovalRequest
    ));
    let unknown = fixture
        .store
        .query_messages_for_conversation(
            &fixture.agents[0],
            &fixture.conversations[0],
            MessageListOptions {
                before: Some(message_id("letta-msg-999")),
                ..MessageListOptions::default()
            },
        )
        .await
        .expect("unknown cursor ignored");
    assert_eq!(unknown, desc);
}

pub(crate) async fn messages_for_agent_body() {
    let fixture = fixture("query-messages-agent").await;
    let messages = fixture
        .store
        .query_messages_for_agent(
            &fixture.agents[0],
            MessageListOptions {
                order: MessageOrder::Ascending,
                ..MessageListOptions::default()
            },
        )
        .await
        .expect("messages");
    assert!(
        messages
            .iter()
            .all(|message| message.source.agent_id == fixture.agents[0])
    );
    assert!(
        messages
            .windows(2)
            .all(|pair| pair[0].timestamp_ms <= pair[1].timestamp_ms)
    );
    let repeat = fixture
        .store
        .query_messages_for_agent(
            &fixture.agents[0],
            MessageListOptions {
                order: MessageOrder::Ascending,
                ..MessageListOptions::default()
            },
        )
        .await
        .expect("repeat");
    assert_eq!(messages, repeat);
    let scopes = messages
        .iter()
        .map(|message| message.source.conversation_id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(scopes.len(), 3);
    assert!(
        fixture
            .conversations
            .iter()
            .all(|scope| scopes.contains(scope))
    );
    assert_eq!(
        messages
            .iter()
            .map(|message| &message.id)
            .collect::<BTreeSet<_>>()
            .len(),
        messages.len()
    );
    for message in messages {
        let resolved = fixture
            .store
            .query_message_by_projected_id(
                &message.source.agent_id,
                &message.source.conversation_id,
                &message.id,
            )
            .await
            .expect("carried scope resolves");
        assert_eq!(resolved.source, message.source);
    }
}

pub(crate) mod projection_body {
    use super::*;

    pub(crate) async fn same_source_message_body() {
        super::super::review_evidence::projection_exact_body();
        canonical_projection_stress();
        fallback_projection_stress();
        projection_count_boundaries();
        duplicate_source_conflict();
        let fixture = fixture("query-projection").await;
        let projections = fixture
            .store
            .query_messages_for_conversation(
                &fixture.agents[0],
                &fixture.conversations[0],
                MessageListOptions {
                    order: MessageOrder::Ascending,
                    ..MessageListOptions::default()
                },
            )
            .await
            .expect("list");
        for projected in projections {
            assert!(projected.id.as_str().starts_with("letta-msg-"));
            let by_projection = fixture
                .store
                .query_message_by_projected_id(
                    &fixture.agents[0],
                    &fixture.conversations[0],
                    &projected.id,
                )
                .await
                .expect("projected");
            let by_source = fixture
                .store
                .query_message_by_source_id(
                    &fixture.agents[0],
                    &fixture.conversations[0],
                    &projected.source.source_id,
                )
                .await
                .expect("source");
            assert_eq!(by_projection.source, by_source.source);
            assert_eq!(by_projection.message, by_source.message);
            assert_eq!(by_projection.source, projected.source);
        }
        assert_eq!(
            fixture
                .store
                .query_message_by_projected_id(
                    &fixture.agents[0],
                    &fixture.conversations[0],
                    &message_id("ui-msg-default-1")
                )
                .await
                .expect_err("wrong surface")
                .kind(),
            StoreErrorKind::NotFound
        );
    }

    fn canonical_projection_stress() {
        let agent = agent_id("agent-local-stress");
        let conversation = conversation_id("stress");
        let source = vec![
            projection_message("ui-msg-71", 12, 1.0),
            projection_message("ui-msg-72", 1, 2.0),
        ];
        let projected = super::super::projection_impl::messages(
            &source,
            &agent,
            &conversation,
            std::path::Path::new("projection-stress"),
        )
        .expect("canonical projection");
        assert_eq!(projected.len(), 13);
        assert_eq!(
            projected
                .iter()
                .map(|value| &value.id)
                .collect::<BTreeSet<_>>()
                .len(),
            13
        );
        for pair in projected.windows(2) {
            assert!(projected_number(&pair[0].id) < projected_number(&pair[1].id));
        }
        assert_eq!(
            projected_number(&projected[12].id) - projected_number(&projected[0].id),
            QUERY_PROJECTED_MESSAGES_MAX as u64
        );
        for value in &projected {
            let source_message = &source[value.source.source_ordinal];
            assert_eq!(value.source.agent_id, agent);
            assert_eq!(value.source.conversation_id, conversation);
            assert_eq!(value.source.source_id, source_message.id);
            assert_eq!(source_message, &source[value.source.source_ordinal]);
        }
    }

    fn fallback_projection_stress() {
        let agent = agent_id("agent-local-fallback");
        let left = conversation_id("fallback-left");
        let right = conversation_id("fallback-right");
        for source_id in ["pi-msg-30", "ui-msg-fixture"] {
            let source = vec![projection_message(source_id, 2, 1.0)];
            let first = super::super::projection_impl::messages(
                &source,
                &agent,
                &left,
                std::path::Path::new("fallback"),
            )
            .expect("first fallback");
            let repeat = super::super::projection_impl::messages(
                &source,
                &agent,
                &left,
                std::path::Path::new("fallback"),
            )
            .expect("repeat fallback");
            let other = super::super::projection_impl::messages(
                &source,
                &agent,
                &right,
                std::path::Path::new("fallback"),
            )
            .expect("other scope");
            assert_eq!(first, repeat);
            assert_ne!(first[0].id, other[0].id);
            assert!(projected_number(&first[0].id) > u64::MAX / 2);
            assert_eq!(first[0].source.source_id, source[0].id);
            assert_eq!(first[0].source.source_ordinal, 0);
        }
    }

    fn projection_count_boundaries() {
        let agent = agent_id("agent-local-boundary");
        let conversation = conversation_id("boundary");
        for count in [
            QUERY_PROJECTED_MESSAGES_MAX - 1,
            QUERY_PROJECTED_MESSAGES_MAX,
        ] {
            let source = vec![projection_message("ui-msg-81", count, 1.0)];
            let projected = super::super::projection_impl::messages(
                &source,
                &agent,
                &conversation,
                std::path::Path::new("boundary"),
            )
            .expect("accepted boundary");
            assert_eq!(projected.len(), count);
            assert_eq!(
                projected
                    .iter()
                    .map(|value| &value.id)
                    .collect::<BTreeSet<_>>()
                    .len(),
                count
            );
        }
        let source = vec![projection_message(
            "ui-msg-81",
            QUERY_PROJECTED_MESSAGES_MAX + 1,
            1.0,
        )];
        assert_eq!(
            super::super::projection_impl::messages(
                &source,
                &agent,
                &conversation,
                std::path::Path::new("boundary")
            )
            .expect_err("above boundary")
            .kind(),
            StoreErrorKind::Limit
        );
    }

    fn duplicate_source_conflict() {
        let agent = agent_id("agent-local-conflict");
        let conversation = conversation_id("conflict");
        let source = vec![
            projection_message("ui-msg-91", 1, 1.0),
            projection_message("ui-msg-91", 1, 2.0),
        ];
        let snapshot = source.clone();
        assert_eq!(
            super::super::projection_impl::messages(
                &source,
                &agent,
                &conversation,
                std::path::Path::new("conflict")
            )
            .expect_err("duplicate source")
            .kind(),
            StoreErrorKind::StorageConflict
        );
        assert_eq!(source, snapshot);
    }
}

pub(crate) async fn resume_tail_body() {
    let fixture = fixture("query-resume").await;
    for limit in [RESUME_TAIL_MESSAGES_MAX - 1, RESUME_TAIL_MESSAGES_MAX] {
        let tail = fixture
            .store
            .query_resume_tail(&fixture.agents[0], &fixture.conversations[0], limit)
            .await
            .expect("tail");
        assert_eq!(tail.conversation.agent_id, fixture.agents[0]);
        assert!(
            tail.messages
                .windows(2)
                .all(|pair| pair[0].timestamp_ms <= pair[1].timestamp_ms)
        );
    }
    for limit in [RESUME_TAIL_MESSAGES_MAX - 1, RESUME_TAIL_MESSAGES_MAX] {
        assert!(super::validate_resume_tail_limit(limit).is_ok());
    }
    assert_eq!(
        super::validate_resume_tail_limit(RESUME_TAIL_MESSAGES_MAX + 1)
            .expect_err("validator above")
            .kind(),
        StoreErrorKind::Limit
    );
    let small = fixture
        .store
        .query_resume_tail(&fixture.agents[0], &fixture.conversations[0], 2)
        .await
        .expect("small suffix");
    let all = fixture
        .store
        .query_messages_for_conversation(
            &fixture.agents[0],
            &fixture.conversations[0],
            MessageListOptions {
                order: MessageOrder::Ascending,
                ..MessageListOptions::default()
            },
        )
        .await
        .expect("all messages");
    assert_eq!(small.messages, all[all.len() - 2..]);
    let empty = fixture
        .store
        .query_resume_tail(&fixture.agents[0], &fixture.conversations[0], 0)
        .await
        .expect("empty");
    assert!(empty.messages.is_empty());
    assert_eq!(
        fixture
            .store
            .query_resume_tail(
                &fixture.agents[0],
                &fixture.conversations[0],
                RESUME_TAIL_MESSAGES_MAX + 1
            )
            .await
            .expect_err("above")
            .kind(),
        StoreErrorKind::Limit
    );
}

pub(crate) async fn transcript_search_body() {
    let fixture = fixture("query-search").await;
    let path = transcript_path(&fixture, &fixture.conversations[0]);
    let before = file_snapshot(&path);
    let hits = fixture
        .store
        .query_transcript_search(TranscriptSearch {
            query: "needle \"a\"".into(),
            agent_id: Some(fixture.agents[0].clone()),
            conversation_id: Some(fixture.conversations[0].clone()),
            include_hidden: false,
            limit: 10,
        })
        .await
        .expect("search");
    assert!(!hits.is_empty());
    assert!(
        hits.iter()
            .all(|hit| hit.source.agent_id == fixture.agents[0]
                && hit.source.conversation_id == fixture.conversations[0])
    );
    assert_eq!(before, file_snapshot(&path));
    verify_search_scopes(&fixture).await;
    super::search_review_evidence::search_blockers_body(&fixture, &path).await;
    verify_malformed_row_is_nonmutating(&fixture, &path).await;
    #[cfg(unix)]
    verify_symlink_rejected().await;
    assert_eq!(
        hits,
        fixture
            .store
            .query_transcript_search(TranscriptSearch {
                query: "needle \"a\"".into(),
                agent_id: Some(fixture.agents[0].clone()),
                conversation_id: Some(fixture.conversations[0].clone()),
                include_hidden: false,
                limit: 10
            })
            .await
            .expect("repeat")
    );
    let default_unscoped = fixture
        .store
        .query_transcript_search(TranscriptSearch {
            query: "needle".into(),
            agent_id: None,
            conversation_id: Some(conversation_id("default")),
            include_hidden: false,
            limit: 10,
        })
        .await
        .expect_err("scope");
    assert_eq!(default_unscoped.kind(), StoreErrorKind::Limit);
    for length in [QUERY_TEXT_BYTES_MAX - 1, QUERY_TEXT_BYTES_MAX] {
        assert!(
            fixture
                .store
                .query_transcript_search(TranscriptSearch {
                    query: "x".repeat(length),
                    agent_id: Some(fixture.agents[0].clone()),
                    conversation_id: Some(fixture.conversations[0].clone()),
                    include_hidden: false,
                    limit: 1,
                })
                .await
                .is_ok()
        );
    }
    for length in [QUERY_SCAN_ENTRIES_MAX - 1, QUERY_SCAN_ENTRIES_MAX] {
        assert!(super::scan::validate_scan_entries(length).is_ok());
    }
    assert_eq!(
        super::scan::validate_scan_entries(QUERY_SCAN_ENTRIES_MAX + 1)
            .expect_err("scan above")
            .kind(),
        StoreErrorKind::Limit
    );
    assert_eq!(
        fixture
            .store
            .query_transcript_search(TranscriptSearch {
                query: "x".repeat(QUERY_TEXT_BYTES_MAX + 1),
                agent_id: None,
                conversation_id: None,
                include_hidden: false,
                limit: 1
            })
            .await
            .expect_err("bound")
            .kind(),
        StoreErrorKind::Limit
    );
}

async fn verify_search_scopes(fixture: &Fixture) {
    let hidden_default = fixture
        .store
        .query_transcript_search(TranscriptSearch {
            query: "needle b".into(),
            agent_id: Some(fixture.agents[0].clone()),
            conversation_id: None,
            include_hidden: false,
            limit: 10,
        })
        .await
        .expect("hidden excluded");
    assert!(hidden_default.is_empty());
    let hidden_included = fixture
        .store
        .query_transcript_search(TranscriptSearch {
            query: "needle b".into(),
            agent_id: Some(fixture.agents[0].clone()),
            conversation_id: Some(fixture.conversations[1].clone()),
            include_hidden: true,
            limit: 10,
        })
        .await
        .expect("hidden included");
    assert!(!hidden_included.is_empty());
    assert!(
        hidden_included
            .iter()
            .all(|hit| hit.source.agent_id == fixture.agents[0]
                && hit.source.conversation_id == fixture.conversations[1])
    );
    let isolated = fixture
        .store
        .query_transcript_search(TranscriptSearch {
            query: "needle isolated".into(),
            agent_id: Some(fixture.agents[1].clone()),
            conversation_id: Some(fixture.conversations[0].clone()),
            include_hidden: false,
            limit: 10,
        })
        .await
        .expect("exact isolated scope");
    assert!(!isolated.is_empty());
    assert!(
        isolated
            .iter()
            .all(|hit| hit.source.agent_id == fixture.agents[1]
                && hit.source.conversation_id == fixture.conversations[0])
    );
}

async fn verify_malformed_row_is_nonmutating(fixture: &Fixture, path: &std::path::Path) {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("append malformed");
    file.write_all(b"{malformed-row\n")
        .expect("write malformed");
    file.sync_all().expect("sync malformed");
    let appended = file_snapshot(path);
    let hits = fixture
        .store
        .query_transcript_search(TranscriptSearch {
            query: "needle a".into(),
            agent_id: Some(fixture.agents[0].clone()),
            conversation_id: Some(fixture.conversations[0].clone()),
            include_hidden: false,
            limit: 10,
        })
        .await
        .expect("malformed skipped");
    assert!(!hits.is_empty());
    assert_eq!(appended, file_snapshot(path));
}

#[cfg(unix)]
async fn verify_symlink_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = fixture("query-search-symlink").await;
    let path = transcript_path(&fixture, &fixture.conversations[0]);
    let outside = fixture.root.path().join("outside-sentinel");
    std::fs::write(&outside, b"secret outside needle").expect("outside sentinel");
    let outside_before = file_snapshot(&outside);
    std::fs::remove_file(&path).expect("remove messages");
    symlink(&outside, &path).expect("messages symlink");
    let error = fixture
        .store
        .query_transcript_search(TranscriptSearch {
            query: "secret outside needle".into(),
            agent_id: Some(fixture.agents[0].clone()),
            conversation_id: Some(fixture.conversations[0].clone()),
            include_hidden: false,
            limit: 10,
        })
        .await
        .expect_err("symlink rejected");
    assert_eq!(error.kind(), StoreErrorKind::InvalidPath);
    assert_eq!(outside_before, file_snapshot(&outside));
}

pub(crate) fn runtime_subscribers_body() {
    let scope = RuntimeScope::new(agent_id("agent-local-a"), conversation_id("default"), None);
    let other = RuntimeScope::new(agent_id("agent-local-b"), conversation_id("default"), None);
    let connections = vec![
        connection("b", 2, true, true, scope.clone()),
        connection("a", 2, true, true, scope.clone()),
        connection("uninitialized", 1, false, true, scope.clone()),
        connection("dead", 0, true, false, scope.clone()),
        connection("other", 0, true, true, other),
    ];
    let subscribers = super::query_runtime_subscribers(&connections, &scope).expect("subscribers");
    assert_eq!(
        subscribers
            .iter()
            .map(|value| value.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(
        subscribers,
        super::query_runtime_subscribers(&connections, &scope).expect("repeat")
    );
    for length in [
        QUERY_RUNTIME_CONNECTIONS_MAX - 1,
        QUERY_RUNTIME_CONNECTIONS_MAX,
    ] {
        let mut boundary = vec![connections[4].clone(); length];
        boundary[length - 1] = connections[0].clone();
        assert_eq!(
            super::query_runtime_subscribers(&boundary, &scope)
                .expect("runtime boundary")
                .len(),
            1
        );
    }
    let above = vec![connections[0].clone(); QUERY_RUNTIME_CONNECTIONS_MAX + 1];
    assert_eq!(
        super::query_runtime_subscribers(&above, &scope)
            .expect_err("above")
            .kind(),
        StoreErrorKind::Limit
    );
}

fn ids(agents: &[lotta_domain::Agent]) -> Vec<&str> {
    agents.iter().map(|agent| agent.id.as_str()).collect()
}

fn connection(
    id: &str,
    ordinal: u64,
    initialized: bool,
    live: bool,
    scope: RuntimeScope,
) -> RuntimeConnectionSnapshot {
    RuntimeConnectionSnapshot {
        connection: RuntimeConnection {
            id: NonEmptyString::new(id).expect("id"),
            ordinal,
            initialized,
            subscriptions: BoundedVec::new(vec![scope]).expect("subscriptions"),
            event_seq: 0,
        },
        live,
    }
}
