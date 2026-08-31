use axum::http::HeaderValue;

mod cursor {
    #[test]
    fn rejects_non_cursor_ids() {
        assert!(crate::openai::cursor::parse("resp_missing").is_err());
    }
}

mod non_stored {
    use super::*;

    #[test]
    fn normalizes_string_input_and_literal_booleans() {
        let prepared = input::prepare(
            json!({"model":"memo","input":"hello","store":1,"stream":"true"}),
            &HeaderMap::new(),
        )
        .unwrap_or_else(|_| panic!("valid input"));
        assert!(!prepared.store);
        assert!(!prepared.streaming);
        assert_eq!(prepared.messages.len(), 1);
    }

    #[test]
    fn letta_key_precedes_openwebui() {
        let mut headers = HeaderMap::new();
        headers.insert("x-letta-chat-key", HeaderValue::from_static("explicit"));
        headers.insert("x-openwebui-chat-id", HeaderValue::from_static("fallback"));
        let prepared = input::prepare(
            json!({"model":"memo","input":"hello","stream":true}),
            &headers,
        )
        .unwrap_or_else(|_| panic!("valid input"));
        assert_eq!(prepared.chat_key.as_deref(), Some("explicit"));
    }
}

mod previous_response_id {
    use super::*;

    #[test]
    fn malformed_cursor_has_exact_not_found_code() {
        let response = errors::response_not_found("resp_missing", None);
        let value = serde_json::to_value(response)
            .unwrap_or_else(|error| panic!("error envelope: {error}"));
        assert_eq!(value["error"]["code"], "response_not_found");
    }

    #[test]
    fn unsupported_backend_has_exact_code() {
        let value = serde_json::to_value(errors::unsupported_backend())
            .unwrap_or_else(|error| panic!("error envelope: {error}"));
        assert_eq!(value["error"]["code"], "unsupported_backend");
    }
}

mod no_idempotency {
    use super::*;

    #[test]
    fn both_headers_are_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", HeaderValue::from_static("same"));
        headers.insert("x-idempotency-key", HeaderValue::from_static("other"));
        let prepared = input::prepare(json!({"model":"memo","input":"hello"}), &headers)
            .unwrap_or_else(|_| panic!("valid input"));
        assert!(prepared.chat_key.is_none());
    }
}

mod streaming {
    use super::output::{OutputSignal, signal_events};

    #[test]
    fn exact_text_event_names_are_projected() {
        let (events, _) = signal_events(&[OutputSignal::Text("hello".to_owned())]);
        let names = events
            .iter()
            .map(|event| event["type"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
                "response.output_text.done",
                "response.content_part.done",
                "response.output_item.done",
            ]
        );
    }
}
