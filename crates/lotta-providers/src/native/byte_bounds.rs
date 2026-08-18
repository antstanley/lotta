use super::anthropic::Anthropic;
use super::loopback::{Loopback, ResponseScript};
use super::openai_compatible::OpenAiCompatible;
use super::sse::{SseError, SseParser};
use lotta_runtime::boundary::ProviderText;
use lotta_runtime::bounds::{PROVIDER_REQUEST_BYTES_MAX, PROVIDER_RESPONSE_EVENT_BYTES_MAX};
use lotta_runtime::ports::{ProviderPort, provider_event_channel};
use lotta_testkit::contract::fixtures::provider_request;
use tokio_util::sync::CancellationToken;

const OPENAI_SUCCESS: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: text/event-stream\r\n",
    "connection: close\r\n\r\n",
    r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
    "\n\ndata: [DONE]\n\n",
);
const ANTHROPIC_SUCCESS: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: text/event-stream\r\n",
    "connection: close\r\n\r\n",
    "event: message_stop\n",
    r#"data: {"type":"message_stop"}"#,
    "\n\n",
);

#[derive(Clone, Copy)]
enum Dialect {
    OpenAi,
    Anthropic,
}

fn assert_sse_bound() {
    let maximum = PROVIDER_RESPONSE_EVENT_BYTES_MAX.value;
    for size in [maximum - 1, maximum] {
        let mut parser = SseParser::new();
        let line = format!("data: {}\r\n\r\n", "x".repeat(size));
        let events = parser.push(line.as_bytes()).expect("within bound");
        assert_eq!(events[0].data.len(), size);
    }
    let mut rejected = SseParser::new();
    let line = format!("data: {}\n", "x".repeat(maximum + 1));
    assert_eq!(rejected.push(line.as_bytes()), Err(SseError::Limit));
}

async fn execute(dialect: Dialect, size: usize) -> (bool, Loopback) {
    let response = match dialect {
        Dialect::OpenAi => OPENAI_SUCCESS,
        Dialect::Anthropic => ANTHROPIC_SUCCESS,
    };
    let server = Loopback::scripted(ResponseScript::Complete(response.into())).await;
    let adapter: Box<dyn ProviderPort> = match dialect {
        Dialect::OpenAi => {
            Box::new(OpenAiCompatible::new(server.base(), "credential").expect("openai"))
        }
        Dialect::Anthropic => {
            Box::new(Anthropic::new(server.base(), "credential").expect("anthropic"))
        }
    };
    let mut request = provider_request(CancellationToken::new());
    request.system_prompt = Some(ProviderText::new("x".repeat(size)).expect("text"));
    request.context = Some(lotta_runtime::ports::ProviderContext {
        catalog_max: Some(u64::from(u32::MAX)),
        ..Default::default()
    });
    let cancellation = CancellationToken::new();
    let (sink, _receiver) = provider_event_channel(8, &cancellation).expect("channel");
    let expected = request.validate_bytes().is_ok();
    let result = adapter.stream(request, sink).await;
    assert_eq!(result.is_ok(), expected, "adapter execution: {result:?}");
    (expected, server)
}

async fn assert_public_outbound_bound(dialect: Dialect) {
    let maximum = PROVIDER_REQUEST_BYTES_MAX.value;
    let mut request = provider_request(CancellationToken::new());
    request.system_prompt = Some(ProviderText::new(String::new()).expect("empty"));
    let empty = request.normalized_wire_bytes().expect("empty size");
    let at = maximum - empty;

    assert!(request_with_size(at - 1).validate_bytes().is_ok());
    let (ok, server) = execute(dialect, at).await;
    assert!(ok, "size {at}");
    let _recorded = server.recorded().await;
    assert!(request_with_size(at + 1).validate_bytes().is_err());
}

fn request_with_size(size: usize) -> lotta_runtime::ports::ProviderRequest {
    let mut request = provider_request(CancellationToken::new());
    request.system_prompt = Some(ProviderText::new("x".repeat(size)).expect("text"));
    request.context = Some(lotta_runtime::ports::ProviderContext {
        catalog_max: Some(u64::from(u32::MAX)),
        ..Default::default()
    });
    request
}

#[test]
fn openai_inbound_sse_below_at_above_exact_bound() {
    assert_sse_bound();
}

#[test]
fn anthropic_inbound_sse_below_at_above_exact_bound() {
    assert_sse_bound();
}

#[tokio::test]
async fn openai_public_outbound_serialized_request_below_at_above_exact_bound() {
    assert_public_outbound_bound(Dialect::OpenAi).await;
}

#[tokio::test]
async fn anthropic_public_outbound_serialized_request_below_at_above_exact_bound() {
    assert_public_outbound_bound(Dialect::Anthropic).await;
}

#[tokio::test]
async fn public_adapters_reject_oversized_inbound_event_once() {
    for dialect in [Dialect::OpenAi, Dialect::Anthropic] {
        let maximum = PROVIDER_RESPONSE_EVENT_BYTES_MAX.value;
        let head = concat!(
            "HTTP/1.1 200 OK\r\n",
            "content-type: text/event-stream\r\n",
            "connection: close\r\n\r\n",
        );
        let response = format!("{head}data: {}\n", "x".repeat(maximum + 1));
        let server = Loopback::scripted(ResponseScript::Complete(response.into())).await;
        let adapter: Box<dyn ProviderPort> = match dialect {
            Dialect::OpenAi => {
                Box::new(OpenAiCompatible::new(server.base(), "credential").expect("openai"))
            }
            Dialect::Anthropic => {
                Box::new(Anthropic::new(server.base(), "credential").expect("anthropic"))
            }
        };
        let cancellation = CancellationToken::new();
        let (sink, mut receiver) = provider_event_channel(4, &cancellation).expect("channel");
        let result = adapter
            .stream(provider_request(CancellationToken::new()), sink)
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(matches!(
            receiver.receive().await.expect("receive"),
            Some(lotta_runtime::ports::ProviderEvent::Error { .. })
        ));
        assert!(receiver.receive().await.expect("receive").is_none());
        let _ = server.recorded().await;
    }
}

#[test]
fn framing_bytes_are_not_counted_as_vendor_event_payload() {
    let maximum = PROVIDER_RESPONSE_EVENT_BYTES_MAX.value;
    let line = format!("data: {}\r\n\r\n", "x".repeat(maximum));
    assert_eq!(line.len(), maximum + "data: \r\n\r\n".len());
}
