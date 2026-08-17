use super::anthropic::{Anthropic, AnthropicState, handle_event as handle_anthropic_event};
use super::loopback::{Loopback, ResponseScript};
use super::openai_compatible::{OpenAiState, handle_value as handle_openai_value};
use lotta_runtime::boundary::ProviderName;
use lotta_runtime::ports::{ProviderEvent, ProviderPort, provider_event_channel};
use lotta_testkit::contract::fixtures::provider_request;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const SIGNED_THINKING: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: text/event-stream\r\n",
    "connection: close\r\n\r\n",
    "event: message_start\n",
    r#"data: {"type":"message_start","message":{"id":"message-id","model":"model-id","usage":{"input_tokens":1}}}"#,
    "\n\n",
    "event: content_block_delta\n",
    r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"thought"}}"#,
    "\n\n",
    "event: content_block_delta\n",
    r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"secret-signature"}}"#,
    "\n\n",
    "event: content_block_stop\n",
    r#"data: {"type":"content_block_stop","index":0}"#,
    "\n\n",
    "event: message_delta\n",
    r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#,
    "\n\n",
    "event: message_stop\n",
    r#"data: {"type":"message_stop"}"#,
    "\n\n",
);

async fn receive_all(
    receiver: &mut lotta_runtime::ports::ProviderEventReceiver,
    count: usize,
) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    for _ in 0..count {
        events.push(receiver.receive().await.expect("receive").expect("event"));
    }
    events
}

#[tokio::test]
async fn openai_reasoning_fields_use_pinned_precedence() {
    let cancellation = CancellationToken::new();
    let (sink, mut receiver) = provider_event_channel(8, &cancellation).expect("channel");
    let mut state = OpenAiState::default();
    handle_openai_value(
        &json!({"choices":[{"index":0,"delta":{
            "content":"visible", "reasoning_content":"first",
            "reasoning":"second", "reasoning_text":"third"
        }}]}),
        &mut state,
        &sink,
    )
    .await
    .expect("value");
    let events = receive_all(&mut receiver, 2).await;
    assert!(matches!(
        &events[0],
        ProviderEvent::TextDelta { text } if text.as_str() == "visible"
    ));
    assert!(matches!(
        &events[1],
        ProviderEvent::ReasoningDelta { text } if text.as_str() == "first"
    ));
}

#[tokio::test]
async fn anthropic_thinking_redaction_and_signature_preserve_event_kinds() {
    let cancellation = CancellationToken::new();
    let (sink, mut receiver) = provider_event_channel(8, &cancellation).expect("channel");
    let mut state = AnthropicState::default();
    handle_anthropic_event(
        "content_block_start",
        &json!({"index":0,"content_block":{"type":"redacted_thinking","data":"opaque"}}),
        &mut state,
        &sink,
    )
    .await
    .expect("redacted");
    handle_anthropic_event(
        "content_block_delta",
        &json!({"index":1,"delta":{"type":"thinking_delta","thinking":"thought"}}),
        &mut state,
        &sink,
    )
    .await
    .expect("thinking");
    handle_anthropic_event(
        "content_block_delta",
        &json!({"index":1,"delta":{"type":"signature_delta","signature":"metadata-only"}}),
        &mut state,
        &sink,
    )
    .await
    .expect("signature");
    let events = receive_all(&mut receiver, 2).await;
    assert!(matches!(events[0], ProviderEvent::RedactedReasoning { .. }));
    assert!(matches!(
        &events[1],
        ProviderEvent::ReasoningDelta { text } if text.as_str() == "thought"
    ));
    assert!(
        state
            .signatures
            .values()
            .any(|value| value == "metadata-only")
    );
}

#[tokio::test]
async fn anthropic_public_signed_thinking_omits_empty_secret_metadata() {
    let server = Loopback::scripted(ResponseScript::Complete(SIGNED_THINKING.into())).await;
    let adapter = Anthropic::new(server.base(), "credential").expect("adapter");
    let cancellation = CancellationToken::new();
    let (sink, mut receiver) = provider_event_channel(16, &cancellation).expect("channel");
    let result = adapter
        .stream(provider_request(CancellationToken::new()), sink)
        .await;
    assert!(result.is_ok(), "{result:?}");
    let mut events = Vec::new();
    while let Some(event) = receiver.receive().await.expect("receive") {
        events.push(event);
    }
    let metadata = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ProviderMetadata { metadata } => Some(metadata),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(metadata.len(), 1, "{events:?}");
    let id = ProviderName::new("id".to_owned()).expect("id key");
    let model = ProviderName::new("model".to_owned()).expect("model key");
    let signature = ProviderName::new("signature".to_owned()).expect("signature key");
    assert_eq!(metadata[0].get(&id), Some(&json!("message-id")));
    assert_eq!(metadata[0].get(&model), Some(&json!("model-id")));
    assert_eq!(metadata[0].get(&signature), None);
    assert!(!format!("{events:?}").contains("secret-signature"));
    let _ = server.recorded().await;
}
