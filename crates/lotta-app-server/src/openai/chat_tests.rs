use axum::{body::to_bytes as test_to_bytes, http::HeaderValue as TestHeaderValue};

fn request(messages: &Value, stream: bool) -> Value {
    json!({"model":"memo", "messages":messages, "stream":stream})
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
        let OutcomeClaim::Owner(owner) = cache.claim("same".to_owned()).await else {
            panic!("owner claim");
        };
        let OutcomeClaim::Existing(duplicate) = cache.claim("same".to_owned()).await else {
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
    async fn evicts_failed_outcome_for_retry() {
        let cache = OutcomeCache::new();
        let OutcomeClaim::Owner(owner) = cache.claim("retry".to_owned()).await else {
            panic!("owner claim");
        };
        cache.evict_failed("retry", &owner).await;
        assert!(matches!(
            cache.claim("retry".to_owned()).await,
            OutcomeClaim::Owner(_)
        ));
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
    async fn idempotency_below_at_above_is_fifo_and_observable() {
        let cache = OutcomeCache::new();
        for index in 0..=crate::bounds::CHAT_IDEMPOTENCY_OUTCOMES_MAX {
            assert!(matches!(
                cache.claim(format!("key-{index}")).await,
                OutcomeClaim::Owner(_)
            ));
        }
        assert!(matches!(
            cache.claim("key-0".to_owned()).await,
            OutcomeClaim::Owner(_)
        ));
        assert!(matches!(
            cache
                .claim(format!(
                    "key-{}",
                    crate::bounds::CHAT_IDEMPOTENCY_OUTCOMES_MAX
                ))
                .await,
            OutcomeClaim::Existing(_)
        ));
    }

    #[tokio::test]
    async fn chat_keys_below_at_above_evict_oldest() {
        let cache = ChatKeyCache::new();
        for index in 0..=crate::bounds::OPENAI_CHAT_KEYS_MAX {
            assert!(matches!(
                cache.claim(format!("key-{index}")).await,
                ChatKeyClaim::Owner(_)
            ));
        }
        assert!(matches!(
            cache.claim("key-0".to_owned()).await,
            ChatKeyClaim::Owner(_)
        ));
        assert!(matches!(
            cache
                .claim(format!("key-{}", crate::bounds::OPENAI_CHAT_KEYS_MAX))
                .await,
            ChatKeyClaim::Existing(_)
        ));
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
