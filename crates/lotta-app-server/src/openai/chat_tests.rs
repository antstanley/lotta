use axum::{body::to_bytes as test_to_bytes, http::HeaderValue as TestHeaderValue};

fn request(messages: &Value, stream: bool) -> Value {
    json!({"model":"memo", "messages":messages, "stream":stream})
}

fn outcome_key(value: impl Into<String>) -> OutcomeKey {
    OutcomeKey {
        agent_id: "agent".to_owned(),
        chat: ChatIdentity::Persistent("chat".to_owned()),
        idempotency_key: value.into(),
    }
}

fn chat_scope(value: impl Into<String>) -> ChatScopeKey {
    ChatScopeKey {
        agent_id: "agent".to_owned(),
        chat_id: value.into(),
    }
}

fn header(name: &'static str, value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        name,
        TestHeaderValue::from_str(value).unwrap_or_else(|error| panic!("header: {error}")),
    );
    headers
}

mod keying {
    use super::*;

    #[test]
    fn chat_key_pins_conversation() {
        let prepared = prepare(
            request(&json!([{"role":"user", "content":"hi"}]), false),
            &header(CHAT_KEY_HEADER, " chat-1 "),
        )
        .unwrap_or_else(|_| panic!("prepared request"));
        assert_eq!(prepared.chat_key.as_deref(), Some("chat-1"));
    }

    #[test]
    fn openwebui_id_accepted_for_streaming() {
        let prepared = prepare(
            request(&json!([{"role":"user", "content":"hi"}]), true),
            &header(OPENWEBUI_HEADER, "webui-1"),
        )
        .unwrap_or_else(|_| panic!("prepared request"));
        assert_eq!(prepared.chat_key.as_deref(), Some("webui-1"));
    }

    #[test]
    fn explicit_key_precedes_openwebui() {
        let mut headers = header(CHAT_KEY_HEADER, "explicit");
        headers.insert(OPENWEBUI_HEADER, TestHeaderValue::from_static("webui"));
        let prepared = prepare(
            request(&json!([{"role":"user", "content":"hi"}]), true),
            &headers,
        )
        .unwrap_or_else(|_| panic!("prepared request"));
        assert_eq!(prepared.chat_key.as_deref(), Some("explicit"));
    }
}

mod input_selection {
    use super::*;

    fn transcript() -> Value {
        json!([
            {"role":"system", "content":"ignored"},
            {"role":"user", "content":"first"},
            {"role":"assistant", "content":[{"type":"output_text", "text":"reply"}]},
            {"role":"user", "content":[
                {"type":"text", "text":"newest"},
                {"type":"image_url", "image_url":{"url":"https://example.com/image.png"}}
            ]}
        ])
    }

    #[test]
    fn stateful_sends_only_newest_usable_user_input() {
        let prepared = prepare(
            request(&transcript(), false),
            &header(CHAT_KEY_HEADER, "key"),
        )
        .unwrap_or_else(|_| panic!("prepared request"));
        assert_eq!(prepared.messages.len(), 1);
        assert_eq!(prepared.messages[0].role, "user");
        assert_eq!(prepared.messages[0].content.len(), 2);
    }

    #[test]
    fn headerless_replays_user_assistant_order_and_content() {
        let prepared = prepare(request(&transcript(), false), &HeaderMap::new())
            .unwrap_or_else(|_| panic!("prepared request"));
        let roles = prepared
            .messages
            .iter()
            .map(|message| message.role)
            .collect::<Vec<_>>();
        assert_eq!(roles, ["user", "assistant", "user"]);
        assert_eq!(prepared.messages[2].content[1]["type"], "image");
    }
}

mod idempotency {
    use super::*;

    #[tokio::test]
    async fn shares_in_flight_and_replays_settled_success() {
        let cache = OutcomeCache::new();
        let OutcomeClaim::Owner(owner) = cache.claim(outcome_key("same")).await else {
            panic!("owner claim");
        };
        let OutcomeClaim::Existing(duplicate) = cache.claim(outcome_key("same")).await else {
            panic!("duplicate claim");
        };
        assert!(Arc::ptr_eq(&owner, &duplicate));
        owner
            .settle(TurnOutcome {
                text: "answer".to_owned(),
                usage: Usage::default(),
                error: None,
            })
            .await;
        assert_eq!(duplicate.wait().await.text, "answer");
    }

    #[tokio::test]
    async fn failed_eviction_precedes_waiter_wakeup_and_retry_claim() {
        let cache = Arc::new(OutcomeCache::new());
        let key = outcome_key("retry");
        let OutcomeClaim::Owner(owner) = cache.claim(key.clone()).await else {
            panic!("owner claim");
        };
        let OutcomeClaim::Existing(waiter) = cache.claim(key.clone()).await else {
            panic!("waiter claim");
        };
        let removed = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        cache.set_failed_eviction_barrier(Some((
            Arc::clone(&removed),
            Arc::clone(&release),
        )));
        let observed_removal = removed.notified();
        let eviction_cache = Arc::clone(&cache);
        let eviction_key = key.clone();
        let eviction_owner = Arc::clone(&owner);
        let eviction = tokio::spawn(async move {
            eviction_cache
                .evict_failed_and_settle(&eviction_key, &eviction_owner, failed_outcome())
                .await;
        });
        observed_removal.await;

        let OutcomeClaim::Owner(retry) = cache.claim(key).await else {
            panic!("retry must become owner while old failure remains unpublished");
        };
        assert!(!Arc::ptr_eq(&owner, &retry));
        assert!(tokio::time::timeout(std::time::Duration::from_millis(20), waiter.wait())
            .await
            .is_err());
        release.notify_one();
        eviction.await.unwrap();
        assert!(waiter.wait().await.error.is_some());
    }

    #[test]
    fn headerless_exact_retries_share_fingerprint_but_distinct_requests_do_not() {
        let first = prepare(
            request(&json!([{"role":"user", "content":"same"}]), false),
            &HeaderMap::new(),
        )
        .unwrap_or_else(|_| panic!("first request"));
        let retry = prepare(
            request(&json!([{"role":"user", "content":"same"}]), true),
            &HeaderMap::new(),
        )
        .unwrap_or_else(|_| panic!("retry request"));
        let distinct = prepare(
            request(&json!([{"role":"user", "content":"different"}]), false),
            &HeaderMap::new(),
        )
        .unwrap_or_else(|_| panic!("distinct request"));
        assert_eq!(first.fingerprint, retry.fingerprint);
        assert_ne!(first.fingerprint, distinct.fingerprint);
    }

    #[test]
    fn standard_header_precedes_x_alias() {
        let mut headers = header(X_IDEMPOTENCY_HEADER, "fallback");
        headers.insert(IDEMPOTENCY_HEADER, TestHeaderValue::from_static("standard"));
        let prepared = prepare(
            request(&json!([{"role":"user", "content":"hi"}]), false),
            &headers,
        )
        .unwrap_or_else(|_| panic!("prepared request"));
        assert_eq!(prepared.idempotency_key.as_deref(), Some("standard"));
    }
}

mod cache_bounds {
    use super::*;

    #[tokio::test]
    async fn idempotency_never_evicts_active_and_reuses_settled_fifo() {
        let cache = OutcomeCache::new();
        let mut owners = Vec::new();
        for index in 0..crate::bounds::CHAT_IDEMPOTENCY_OUTCOMES_MAX {
            let OutcomeClaim::Owner(owner) = cache.claim(outcome_key(format!("key-{index}"))).await else {
                panic!("owner below/at bound");
            };
            owners.push(owner);
        }
        assert_eq!(cache.owner_count().await, owners.len());
        assert!(matches!(
            cache.claim(outcome_key("overflow-active")).await,
            OutcomeClaim::Full
        ));
        assert!(matches!(
            cache.claim(outcome_key("key-0")).await,
            OutcomeClaim::Existing(_)
        ));
        owners[0]
            .settle(TurnOutcome {
                text: "done".to_owned(),
                usage: Usage::default(),
                error: None,
            })
            .await;
        assert!(matches!(
            cache.claim(outcome_key("overflow-safe")).await,
            OutcomeClaim::Owner(_)
        ));
        assert!(matches!(
            cache.claim(outcome_key("key-0")).await,
            OutcomeClaim::Full
        ));
    }

    #[tokio::test]
    async fn chat_keys_never_evict_allocating_and_reuse_settled_fifo() {
        let cache = ChatKeyCache::new();
        let mut owners = Vec::new();
        for index in 0..crate::bounds::OPENAI_CHAT_KEYS_MAX {
            let ChatKeyClaim::Owner(owner) = cache.claim(chat_scope(format!("key-{index}"))).await else {
                panic!("owner below/at bound");
            };
            owners.push(owner);
        }
        assert_eq!(cache.owner_count().await, owners.len());
        assert!(matches!(
            cache.claim(chat_scope("overflow-active")).await,
            ChatKeyClaim::Full
        ));
        assert!(matches!(
            cache.claim(chat_scope("key-0")).await,
            ChatKeyClaim::Existing(_)
        ));
        let conversation = lotta_domain::ConversationId::accept("conversation-cache-test")
            .unwrap_or_else(|error| panic!("conversation id: {error}"));
        owners[0].settle(Ok(conversation)).await;
        assert!(matches!(
            cache.claim(chat_scope("overflow-safe")).await,
            ChatKeyClaim::Owner(_)
        ));
        assert!(matches!(
            cache.claim(chat_scope("key-0")).await,
            ChatKeyClaim::Full
        ));
    }

    #[test]
    fn structural_keys_do_not_have_delimiter_collisions() {
        let left = OutcomeKey {
            agent_id: "a:b".to_owned(),
            chat: ChatIdentity::Persistent("c".to_owned()),
            idempotency_key: "d".to_owned(),
        };
        let right = OutcomeKey {
            agent_id: "a".to_owned(),
            chat: ChatIdentity::Persistent("b:c".to_owned()),
            idempotency_key: "d".to_owned(),
        };
        assert_ne!(left, right);
    }
}

mod responses {
    use super::*;

    #[tokio::test]
    async fn json_completion_has_pinned_shape() {
        let cell = Arc::new(OutcomeCell::new());
        cell.settle(TurnOutcome {
            text: "hello".to_owned(),
            usage: Usage::default(),
            error: None,
        })
        .await;
        let response = render(cell, false, "memo", false, 1).await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = test_to_bytes(response.into_body(), 16_384)
            .await
            .unwrap_or_else(|error| panic!("body: {error}"));
        let value: Value =
            serde_json::from_slice(&body).unwrap_or_else(|error| panic!("JSON: {error}"));
        assert_eq!(value["object"], "chat.completion");
        assert_eq!(value["choices"][0]["message"]["content"], "hello");
    }

    #[tokio::test]
    async fn late_join_sse_replays_every_settled_chunk_in_order() {
        let cell = Arc::new(OutcomeCell::new());
        cell.publish("one".to_owned());
        cell.publish("-two".to_owned());
        cell.publish("-three".to_owned());
        cell.settle(TurnOutcome {
            text: "one-two-three".to_owned(),
            usage: Usage::default(),
            error: None,
        })
        .await;
        let response = render(cell, false, "memo", true, 1).await;
        let body = test_to_bytes(response.into_body(), 16_384).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let first = text.find("\"content\":\"one\"").unwrap();
        let second = text.find("\"content\":\"-two\"").unwrap();
        let third = text.find("\"content\":\"-three\"").unwrap();
        assert!(first < second && second < third);
        assert!(text.ends_with("data: [DONE]\n\n"));
    }

    #[tokio::test]
    async fn each_http_attempt_gets_a_fresh_completion_identity() {
        let cell = Arc::new(OutcomeCell::new());
        cell.settle(TurnOutcome {
            text: "hello".to_owned(),
            usage: Usage::default(),
            error: None,
        })
        .await;
        let first = render(Arc::clone(&cell), true, "memo", false, 1).await;
        let second = render(cell, false, "memo", false, 999).await;
        let first: Value = serde_json::from_slice(
            &test_to_bytes(first.into_body(), 16_384).await.unwrap(),
        )
        .unwrap();
        let second: Value = serde_json::from_slice(
            &test_to_bytes(second.into_body(), 16_384).await.unwrap(),
        )
        .unwrap();
        assert_ne!(first["id"], second["id"]);
        assert_eq!(first["created"], 1);
        assert_eq!(second["created"], 999);
    }

    #[tokio::test]
    async fn sse_completion_terminates_with_done() {
        let cell = Arc::new(OutcomeCell::new());
        cell.settle(TurnOutcome {
            text: "hello".to_owned(),
            usage: Usage::default(),
            error: None,
        })
        .await;
        let response = render(cell, false, "memo", true, 1).await;
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let body = test_to_bytes(response.into_body(), 16_384)
            .await
            .unwrap_or_else(|error| panic!("body: {error}"));
        let text =
            String::from_utf8(body.to_vec()).unwrap_or_else(|error| panic!("SSE UTF-8: {error}"));
        assert!(text.ends_with("data: [DONE]\n\n"));
        assert!(text.contains("\"finish_reason\":\"stop\""));
        let chunks = text
            .split("\n\n")
            .filter_map(|block| block.strip_prefix("data: "))
            .filter(|data| *data != "[DONE]")
            .map(|data| serde_json::from_str::<Value>(data).expect("SSE chunk JSON"))
            .collect::<Vec<_>>();
        let id = chunks.first().expect("initial chunk")["id"].clone();
        assert!(chunks.iter().all(|chunk| chunk["id"] == id));
        assert!(chunks.iter().all(|chunk| chunk["created"] == 1));
    }

    #[test]
    fn only_literal_true_enables_streaming() {
        for value in [json!(false), json!(null), json!(1), json!("true"), json!({})] {
            let prepared = prepare(
                json!({"model":"memo","messages":[{"role":"user","content":"hi"}],"stream":value}),
                &HeaderMap::new(),
            )
            .unwrap_or_else(|_| panic!("non-true stream value"));
            assert!(!prepared.streaming);
        }
        let prepared = prepare(
            json!({"model":"memo","messages":[{"role":"user","content":"hi"}],"stream":true}),
            &HeaderMap::new(),
        )
        .unwrap_or_else(|_| panic!("true stream value"));
        assert!(prepared.streaming);
    }

    #[test]
    fn every_user_and_assistant_content_array_is_bounded_before_selection() {
        let at = vec![json!({"type":"output_text","text":"x"}); OPENAI_CHAT_CONTENT_PARTS_MAX];
        assert!(prepare(
            request(
                &json!([
                    {"role":"assistant","content":at},
                    {"role":"user","content":"latest"}
                ]),
                false,
            ),
            &header(CHAT_KEY_HEADER, "stateful"),
        )
        .is_ok());
        let above = vec![json!({"type":"output_text","text":"x"}); OPENAI_CHAT_CONTENT_PARTS_MAX + 1];
        assert!(prepare(
            request(
                &json!([
                    {"role":"assistant","content":above},
                    {"role":"user","content":"latest"}
                ]),
                false,
            ),
            &header(CHAT_KEY_HEADER, "stateful"),
        )
        .is_err());
    }

    #[test]
    fn no_usable_user_content_is_rejected() {
        let result = prepare(
            request(&json!([{"role":"system", "content":"only system"}]), false),
            &HeaderMap::new(),
        );
        assert!(result.is_err());
    }
}
