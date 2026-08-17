use super::anthropic::Anthropic;
use super::loopback::{Loopback, ResponseScript};
use super::openai_compatible::OpenAiCompatible;
use lotta_runtime::ports::{ProviderError, ProviderEvent, ProviderPort, provider_event_channel};
use lotta_testkit::contract::fixtures::provider_request;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const SUCCESS_HEAD: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
const ERROR_HEAD: &[u8] =
    b"HTTP/1.1 503 Unavailable\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n";

#[derive(Clone, Copy)]
enum Dialect {
    OpenAi,
    Anthropic,
}

fn adapter(dialect: Dialect, server: &Loopback) -> Box<dyn ProviderPort> {
    match dialect {
        Dialect::OpenAi => {
            Box::new(OpenAiCompatible::new(server.base(), "lifecycle-credential").expect("openai"))
        }
        Dialect::Anthropic => {
            Box::new(Anthropic::new(server.base(), "lifecycle-credential").expect("anthropic"))
        }
    }
}

async fn collect(dialect: Dialect, script: ResponseScript, cancel: bool) -> Vec<ProviderEvent> {
    let server = Loopback::scripted(script).await;
    let token = CancellationToken::new();
    let mut request = provider_request(token.clone());
    request.deadline =
        lotta_runtime::ports::ProviderDeadline::new(Duration::from_millis(20)).expect("deadline");
    let channel_token = CancellationToken::new();
    let (sink, mut receiver) = provider_event_channel(16, &channel_token).expect("channel");
    let port = adapter(dialect, &server);
    let stream = port.stream(request, sink);
    tokio::pin!(stream);
    if cancel {
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        token.cancel();
    }
    let mut events = Vec::new();
    loop {
        tokio::select! {
            result = &mut stream => {
                assert!(result.is_ok(), "adapter lifecycle result: {result:?}");
                while let Ok(Some(event)) = receiver.receive().await {
                    events.push(event);
                }
                break;
            }
            event = receiver.receive() => {
                if let Ok(Some(event)) = event { events.push(event); }
            }
        }
    }
    events
}

fn assert_one_error(events: &[ProviderEvent], expected: fn(&ProviderError) -> bool) {
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        matches!(&events[0], ProviderEvent::Error { error } if expected(error)),
        "{events:?}"
    );
}

#[tokio::test]
async fn cancel_while_waiting_for_headers_is_typed_once_for_both_adapters() {
    for dialect in [Dialect::OpenAi, Dialect::Anthropic] {
        let events = collect(
            dialect,
            ResponseScript::WaitForNotify {
                prefix: SUCCESS_HEAD.to_vec(),
                notify: Arc::new(Notify::new()),
            },
            true,
        )
        .await;
        assert_one_error(&events, |error| {
            matches!(error, ProviderError::Cancelled(_))
        });
    }
}

#[tokio::test]
async fn deadline_while_waiting_for_headers_is_timeout_once_for_both_adapters() {
    tokio::time::pause();
    for dialect in [Dialect::OpenAi, Dialect::Anthropic] {
        let future = collect(
            dialect,
            ResponseScript::WaitForNotify {
                prefix: Vec::new(),
                notify: Arc::new(Notify::new()),
            },
            false,
        );
        tokio::pin!(future);
        tokio::time::advance(Duration::from_millis(21)).await;
        let events = future.await;
        assert_one_error(&events, |error| matches!(error, ProviderError::Timeout(_)));
    }
}

#[tokio::test]
async fn deadline_while_waiting_for_success_body_is_timeout_once() {
    tokio::time::pause();
    for dialect in [Dialect::OpenAi, Dialect::Anthropic] {
        let future = collect(
            dialect,
            ResponseScript::WaitForNotify {
                prefix: SUCCESS_HEAD.to_vec(),
                notify: Arc::new(Notify::new()),
            },
            false,
        );
        tokio::pin!(future);
        tokio::time::advance(Duration::from_millis(21)).await;
        assert_one_error(&future.await, |error| {
            matches!(error, ProviderError::Timeout(_))
        });
    }
}

#[tokio::test]
async fn error_body_stall_is_timeout_and_truncation_is_unavailable() {
    tokio::time::pause();
    for dialect in [Dialect::OpenAi, Dialect::Anthropic] {
        let future = collect(
            dialect,
            ResponseScript::WaitForNotify {
                prefix: ERROR_HEAD.to_vec(),
                notify: Arc::new(Notify::new()),
            },
            false,
        );
        tokio::pin!(future);
        tokio::time::advance(Duration::from_millis(21)).await;
        assert_one_error(&future.await, |error| {
            matches!(error, ProviderError::Timeout(_))
        });
        let mut truncated = ERROR_HEAD.to_vec();
        truncated.extend_from_slice(
            br#"{"error":{"type":"api_error","code":"unavailable","message":"cut""#,
        );
        tokio::time::resume();
        let events = collect(dialect, ResponseScript::Truncate(truncated), false).await;
        tokio::time::pause();
        assert_one_error(&events, |error| {
            matches!(error, ProviderError::Unavailable(_))
        });
    }
}
