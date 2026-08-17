use super::anthropic::Anthropic;
use super::loopback::{Loopback, ResponseScript};
use super::openai_compatible::OpenAiCompatible;
use super::shared;
use lotta_runtime::ports::{ProviderError, ProviderEvent, ProviderPort, provider_event_channel};
use lotta_runtime::retry::RetryAfter;
use lotta_testkit::contract::fixtures::provider_request;
use reqwest::{StatusCode, header::HeaderMap};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
enum Dialect {
    OpenAi,
    Anthropic,
}

#[derive(Clone, Copy)]
enum Expected {
    Authentication,
    Authorization,
    Invalid,
    RateLimit,
    Quota,
    Timeout,
    ContextOverflow,
    Overloaded,
    Unavailable,
    Protocol,
    Cancelled,
    Unknown,
}

const CASES: [(u16, &str, &str, Expected); 12] = [
    (
        401,
        "authentication_error",
        "invalid_api_key",
        Expected::Authentication,
    ),
    (
        403,
        "permission_error",
        "permission_denied",
        Expected::Authorization,
    ),
    (
        400,
        "invalid_request_error",
        "invalid_request",
        Expected::Invalid,
    ),
    (429, "rate_limit_error", "rate_limit", Expected::RateLimit),
    (402, "billing_error", "quota_exceeded", Expected::Quota),
    (504, "timeout_error", "timeout", Expected::Timeout),
    (
        413,
        "request_too_large",
        "context_overflow",
        Expected::ContextOverflow,
    ),
    (529, "overloaded_error", "overloaded", Expected::Overloaded),
    (503, "api_error", "unavailable", Expected::Unavailable),
    (406, "protocol_error", "protocol", Expected::Protocol),
    (408, "cancelled", "cancelled", Expected::Cancelled),
    (418, "mystery", "mystery", Expected::Unknown),
];

fn assert_expected(error: &ProviderError, expected: Expected) {
    assert!(matches!(
        (error, expected),
        (ProviderError::Authentication(_), Expected::Authentication)
            | (ProviderError::Authorization(_), Expected::Authorization)
            | (ProviderError::InvalidRequest(_), Expected::Invalid)
            | (ProviderError::RateLimit(_), Expected::RateLimit)
            | (ProviderError::Quota(_), Expected::Quota)
            | (ProviderError::Timeout(_), Expected::Timeout)
            | (ProviderError::ContextOverflow(_), Expected::ContextOverflow)
            | (ProviderError::Overloaded(_), Expected::Overloaded)
            | (ProviderError::Unavailable(_), Expected::Unavailable)
            | (ProviderError::Protocol(_), Expected::Protocol)
            | (ProviderError::Cancelled(_), Expected::Cancelled)
            | (ProviderError::Unknown(_), Expected::Unknown)
    ));
}

fn response(dialect: Dialect, status: u16, kind: &str, code: &str) -> Vec<u8> {
    let status = format!("HTTP/1.1 {status} FIXTURE\r\n");
    let head = "content-type: application/json\r\nconnection: close\r\n\r\n";
    let error = serde_json::json!({
        "type": kind,
        "code": code,
        "message": "bounded vendor diagnostic",
    });
    let body = match dialect {
        Dialect::OpenAi => serde_json::json!({"error": error}),
        Dialect::Anthropic => serde_json::json!({"type": "error", "error": error}),
    };
    format!("{status}{head}{body}").into_bytes()
}

async fn public_error(dialect: Dialect, case: (u16, &str, &str, Expected)) {
    let (status, kind, code, expected) = case;
    let server = Loopback::scripted(ResponseScript::Complete(response(
        dialect, status, kind, code,
    )))
    .await;
    let adapter: Box<dyn ProviderPort> = match dialect {
        Dialect::OpenAi => {
            Box::new(OpenAiCompatible::new(server.base(), "error-credential").expect("openai"))
        }
        Dialect::Anthropic => {
            Box::new(Anthropic::new(server.base(), "error-credential").expect("anthropic"))
        }
    };
    let cancellation = CancellationToken::new();
    let (sink, mut receiver) = provider_event_channel(4, &cancellation).expect("channel");
    let result = adapter
        .stream(provider_request(CancellationToken::new()), sink)
        .await;
    assert!(result.is_ok(), "{result:?}");
    let event = receiver
        .receive()
        .await
        .expect("receive")
        .expect("error event");
    assert!(receiver.receive().await.expect("receive").is_none());
    match event {
        ProviderEvent::Error { error } => assert_expected(&error, expected),
        other => panic!("unexpected event: {other:?}"),
    }
    let _ = server.recorded().await;
}

#[tokio::test]
async fn openai_compatible_public_adapter_maps_all_twelve_error_paths() {
    for case in CASES {
        public_error(Dialect::OpenAi, case).await;
    }
}

#[tokio::test]
async fn anthropic_public_adapter_maps_all_twelve_error_paths() {
    for case in CASES {
        public_error(Dialect::Anthropic, case).await;
    }
}

#[test]
fn retry_after_seconds_milliseconds_and_http_date_are_normalized() {
    let mut seconds = HeaderMap::new();
    seconds.insert("retry-after", "2".parse().expect("header"));
    assert_eq!(
        shared::retry_after(&seconds),
        Some(RetryAfter::Milliseconds(2_000))
    );
    let mut milliseconds = HeaderMap::new();
    milliseconds.insert("retry-after-ms", "25".parse().expect("header"));
    assert_eq!(
        shared::retry_after(&milliseconds),
        Some(RetryAfter::Milliseconds(25))
    );
    let mut date = HeaderMap::new();
    date.insert(
        "retry-after",
        "Wed, 21 Oct 2099 07:28:00 GMT".parse().expect("header"),
    );
    assert!(matches!(
        shared::retry_after(&date),
        Some(RetryAfter::Milliseconds(value)) if value > 0
    ));
}

#[test]
fn rate_limit_and_quota_are_distinct() {
    let headers = HeaderMap::new();
    let rate = shared::map_error(StatusCode::TOO_MANY_REQUESTS, "error", "x", "x", &headers);
    assert!(matches!(rate, ProviderError::RateLimit(_)));
    let quota = shared::map_error(StatusCode::PAYMENT_REQUIRED, "error", "x", "x", &headers);
    assert!(matches!(quota, ProviderError::Quota(_)));
}
