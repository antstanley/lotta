use super::ollama::Ollama;
use crate::native::loopback::{Loopback, ResponseScript};
use lotta_runtime::ports::{
    ImagePolicy, ProviderContent, ProviderContentPart, ProviderEvent, ProviderPort,
    provider_event_channel,
};
use lotta_testkit::contract::fixtures::provider_request;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

async fn stream(body: &str) -> Vec<ProviderEvent> {
    let server = Loopback::scripted(ResponseScript::Complete(response(body))).await;
    let request = provider_request(CancellationToken::new());
    let (sink, mut receiver) = provider_event_channel(32, &request.cancellation).expect("channel");
    let adapter = Ollama::new(server.base(), "").expect("adapter");
    let stream = tokio::spawn(async move { adapter.stream(request, sink).await });
    let mut events = Vec::new();
    while let Some(event) = receiver.receive().await.expect("receive") {
        events.push(event);
    }
    stream.await.expect("stream task").expect("stream");
    let _ = server.recorded().await;
    events
}

#[tokio::test]
async fn more_than_256_ndjson_chunks_complete_without_artificial_event_limit() {
    let mut body = String::new();
    for _ in 0..300 {
        body.push_str("{\"message\":{\"content\":\"x\"},\"done\":false}\n");
    }
    body.push_str("{\"message\":{},\"done\":true}\n");
    let events = stream(&body).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::TextDelta { .. }))
            .count(),
        300
    );
    assert!(matches!(events.last(), Some(ProviderEvent::Stop { .. })));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProviderEvent::Error { .. }))
    );
}

#[tokio::test]
async fn two_index_zero_calls_across_chunks_are_distinct_and_single_lifecycle() {
    let body = concat!(
        "{\"message\":{\"tool_calls\":[{\"id\":\"vendor-a\",\"function\":{\"index\":0,\"name\":\"alpha\",\"arguments\":{}}}]},\"done\":false}\n",
        "{\"message\":{\"tool_calls\":[{\"id\":\"vendor-b\",\"function\":{\"index\":0,\"name\":\"beta\",\"arguments\":{}}}]},\"done\":false}\n",
        "{\"message\":{},\"done\":true}\n"
    );
    let events = stream(body).await;
    let starts = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolCallStart { call_id, name } => {
                Some((call_id.as_str().to_owned(), name.as_str().to_owned()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        [
            ("vendor-a".into(), "alpha".into()),
            ("vendor-b".into(), "beta".into())
        ]
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::ToolCallEnd { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn clean_eof_before_done_emits_one_protocol_error_and_returns_ok() {
    let events = stream("{\"message\":{\"content\":\"partial\"},\"done\":false}\n").await;
    assert!(matches!(events[0], ProviderEvent::TextDelta { .. }));
    assert!(matches!(events[1], ProviderEvent::Error { .. }));
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn supported_images_preserve_base64_and_message_order() {
    let server = Loopback::scripted(ResponseScript::Complete(response(
        "{\"message\":{},\"done\":true}\n",
    )))
    .await;
    let mut request = provider_request(CancellationToken::new());
    request.image_policy = ImagePolicy::Strict;
    request.deadline =
        lotta_runtime::ports::ProviderDeadline::new(Duration::from_secs(1)).expect("deadline");
    request.messages =
        lotta_runtime::ports::ProviderMessages::new(vec![lotta_runtime::ports::ProviderMessage {
            role: lotta_runtime::ports::ProviderMessageRole::User,
            content: ProviderContent::new(vec![
                ProviderContentPart::Text(
                    lotta_runtime::boundary::ProviderText::new("one".into()).expect("text"),
                ),
                ProviderContentPart::Image {
                    media_type: lotta_runtime::boundary::ProviderName::new("image/png".into())
                        .expect("media"),
                    bytes: lotta_runtime::boundary::ProviderImageBytes::new(b"PNG".to_vec())
                        .expect("bytes"),
                },
                ProviderContentPart::Text(
                    lotta_runtime::boundary::ProviderText::new("two".into()).expect("text"),
                ),
            ])
            .expect("content"),
            tool_call_id: None,
        }])
        .expect("messages");
    let (sink, mut receiver) = provider_event_channel(8, &request.cancellation).expect("channel");
    Ollama::with_image_support(server.base(), "", true)
        .expect("adapter")
        .stream(request, sink)
        .await
        .expect("stream");
    while receiver.receive().await.expect("receive").is_some() {}
    let recorded = server.recorded().await;
    let body: serde_json::Value = serde_json::from_slice(&recorded.body).expect("JSON");
    assert_eq!(body["messages"][0]["content"], "onetwo");
    assert_eq!(body["messages"][0]["images"], serde_json::json!(["UE5H"]));
}
