use super::anthropic::Anthropic;
use super::loopback::{Loopback, ResponseScript};
use super::openai_compatible::OpenAiCompatible;
use lotta_runtime::boundary::{ProviderImageBytes, ProviderName, ProviderText};
use lotta_runtime::ports::{
    ImagePolicy, ProviderContent, ProviderContentPart, ProviderEvent, ProviderMessage,
    ProviderMessageRole, ProviderMessages, ProviderPort, ProviderRequest, provider_event_channel,
};
use lotta_testkit::contract::fixtures::provider_request;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

const OPENAI_SUCCESS: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: text/event-stream\r\n",
    "connection: close\r\n\r\n",
    r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);
const ANTHROPIC_SUCCESS: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: text/event-stream\r\n",
    "connection: close\r\n\r\n",
    "event: message_delta\n",
    r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#,
    "\n\n",
    "event: message_stop\n",
    r#"data: {"type":"message_stop"}"#,
    "\n\n",
);

#[derive(Clone, Copy)]
enum Dialect {
    OpenAi,
    Anthropic,
}

fn request(policy: ImagePolicy) -> ProviderRequest {
    let mut request = provider_request(CancellationToken::new());
    request.image_policy = policy;
    request.messages = ProviderMessages::new(vec![ProviderMessage {
        role: ProviderMessageRole::User,
        content: content(),
        tool_call_id: None,
    }])
    .expect("messages");
    request
}

fn content() -> ProviderContent {
    ProviderContent::new(vec![
        ProviderContentPart::Text(ProviderText::new("before".to_owned()).expect("text")),
        ProviderContentPart::Image {
            media_type: ProviderName::new("image/png".to_owned()).expect("media"),
            bytes: ProviderImageBytes::new(vec![0x89, 0x50, 0x4e, 0x47]).expect("bytes"),
        },
        ProviderContentPart::Text(ProviderText::new("after".to_owned()).expect("text")),
    ])
    .expect("content")
}

fn success(dialect: Dialect) -> Vec<u8> {
    match dialect {
        Dialect::OpenAi => OPENAI_SUCCESS.into(),
        Dialect::Anthropic => ANTHROPIC_SUCCESS.into(),
    }
}

fn adapter(dialect: Dialect, server: &Loopback, supported: bool) -> Box<dyn ProviderPort> {
    match dialect {
        Dialect::OpenAi => Box::new(
            OpenAiCompatible::with_capabilities(
                server.base(),
                "credential",
                super::openai_compatible::MaxTokensField::MaxTokens,
                supported,
            )
            .expect("openai"),
        ),
        Dialect::Anthropic => Box::new(
            Anthropic::with_image_support(server.base(), "credential", supported)
                .expect("anthropic"),
        ),
    }
}

async fn execute(
    dialect: Dialect,
    policy: ImagePolicy,
    supported: bool,
) -> (
    Result<(), lotta_runtime::RuntimeError>,
    Loopback,
    Vec<ProviderEvent>,
) {
    let server = Loopback::scripted(ResponseScript::Complete(success(dialect))).await;
    let adapter = adapter(dialect, &server, supported);
    let cancellation = CancellationToken::new();
    let (sink, mut receiver) = provider_event_channel(16, &cancellation).expect("channel");
    let result = adapter.stream(request(policy), sink).await;
    let mut events = Vec::new();
    while let Some(event) = receiver.receive().await.expect("receive") {
        events.push(event);
    }
    (result, server, events)
}

fn content_array(body: &Value) -> &[Value] {
    body.pointer("/messages/0/content")
        .and_then(Value::as_array)
        .expect("content array")
}

fn assert_drop_order(body: &Value) {
    let content = content_array(body);
    assert_eq!(content.len(), 2);
    assert_eq!(
        content[0].get("text").and_then(Value::as_str),
        Some("before")
    );
    assert_eq!(
        content[1].get("text").and_then(Value::as_str),
        Some("after")
    );
}

fn assert_supported_image(dialect: Dialect, body: &Value) {
    let content = content_array(body);
    assert_eq!(content.len(), 3);
    match dialect {
        Dialect::OpenAi => {
            assert_eq!(content[1]["type"], "image_url");
            assert_eq!(
                content[1]["image_url"]["url"],
                "data:image/png;base64,iVBORw=="
            );
        }
        Dialect::Anthropic => {
            assert_eq!(content[1]["type"], "image");
            assert_eq!(content[1]["source"]["type"], "base64");
            assert_eq!(content[1]["source"]["media_type"], "image/png");
            assert_eq!(content[1]["source"]["data"], "iVBORw==");
        }
    }
}

async fn assert_public_policy(dialect: Dialect) {
    let (result, server, events) = execute(dialect, ImagePolicy::Strict, false).await;
    assert!(result.is_err());
    assert!(events.is_empty());
    server.assert_no_request();

    let (result, server, _) = execute(dialect, ImagePolicy::Drop, false).await;
    assert!(result.is_ok(), "{result:?}");
    let recorded = server.recorded().await;
    let body: Value = serde_json::from_slice(&recorded.body).expect("drop body");
    assert_drop_order(&body);

    let (result, server, _) = execute(dialect, ImagePolicy::Strict, true).await;
    assert!(result.is_ok(), "{result:?}");
    let recorded = server.recorded().await;
    let body: Value = serde_json::from_slice(&recorded.body).expect("supported body");
    assert_supported_image(dialect, &body);
}

#[tokio::test]
async fn openai_public_strict_drop_and_supported_image_policy() {
    assert_public_policy(Dialect::OpenAi).await;
}

#[tokio::test]
async fn anthropic_public_strict_drop_and_supported_image_policy() {
    assert_public_policy(Dialect::Anthropic).await;
}
