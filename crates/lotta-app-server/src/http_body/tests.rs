use super::BoundedJson;
use crate::bounds::HTTP_BODY_BYTES_MAX;
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::DefaultBodyLimit,
    http::{Request, StatusCode},
    routing::post,
};
use futures_util::stream;
use serde::{Deserialize, Deserializer};
use std::{
    convert::Infallible,
    sync::atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;

static BELOW_PROBE: AtomicUsize = AtomicUsize::new(0);
static AT_PROBE: AtomicUsize = AtomicUsize::new(0);
static ABOVE_PROBE: AtomicUsize = AtomicUsize::new(0);
static MALFORMED_PROBE: AtomicUsize = AtomicUsize::new(0);

macro_rules! payload {
    ($name:ident, $probe:ident) => {
        struct $name;
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let _: serde_json::Value = Deserialize::deserialize(deserializer)?;
                $probe.fetch_add(1, Ordering::SeqCst);
                Ok(Self)
            }
        }
    };
}
payload!(BelowPayload, BELOW_PROBE);
payload!(AtPayload, AT_PROBE);
payload!(AbovePayload, ABOVE_PROBE);
payload!(MalformedPayload, MALFORMED_PROBE);

async fn below(BoundedJson(_): BoundedJson<BelowPayload>) -> StatusCode {
    StatusCode::OK
}
async fn at(BoundedJson(_): BoundedJson<AtPayload>) -> StatusCode {
    StatusCode::OK
}
async fn above(BoundedJson(_): BoundedJson<AbovePayload>) -> StatusCode {
    StatusCode::OK
}
async fn malformed(BoundedJson(_): BoundedJson<MalformedPayload>) -> StatusCode {
    StatusCode::OK
}

fn body(total: usize, valid: bool) -> Body {
    const CHUNK: usize = 64 * 1024;
    let source = stream::unfold(0usize, move |sent| async move {
        if sent == total {
            return None;
        }
        let len = (total - sent).min(CHUNK);
        let mut bytes = vec![b' '; len];
        if valid && sent == 0 {
            bytes[0] = b'"';
        }
        if valid && sent + len == total {
            bytes[len - 1] = b'"';
        }
        Some((Ok::<Bytes, Infallible>(bytes.into()), sent + len))
    });
    Body::from_stream(source)
}

async fn post_body(path: &str, body: Body, probe: &AtomicUsize) -> (StatusCode, String, usize) {
    probe.store(0, Ordering::SeqCst);
    let app = Router::new()
        .route("/below", post(below))
        .route("/at", post(at))
        .route("/above", post(above))
        .route("/malformed", post(malformed))
        .layer(DefaultBodyLimit::max(HTTP_BODY_BYTES_MAX));
    let request = Request::post(path).body(body).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    (
        status,
        String::from_utf8(bytes.to_vec()).unwrap(),
        probe.load(Ordering::SeqCst),
    )
}

#[tokio::test]
async fn lazy_json_below_cap_reaches_parser_once() {
    let actual = post_body("/below", body(HTTP_BODY_BYTES_MAX - 1, true), &BELOW_PROBE).await;
    assert_eq!((actual.0, actual.2), (StatusCode::OK, 1));
}

#[tokio::test]
async fn lazy_json_at_cap_reaches_parser_once() {
    let actual = post_body("/at", body(HTTP_BODY_BYTES_MAX, true), &AT_PROBE).await;
    assert_eq!((actual.0, actual.2), (StatusCode::OK, 1));
}

#[tokio::test]
async fn lazy_json_above_cap_returns_stable_413_without_parser() {
    let actual = post_body("/above", body(HTTP_BODY_BYTES_MAX + 1, true), &ABOVE_PROBE).await;
    assert_eq!(
        actual,
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            r#"{"code":"payload_too_large","error":"payload too large"}"#.into(),
            0
        )
    );
}

#[tokio::test]
async fn malformed_below_cap_returns_stable_400() {
    let actual = post_body("/malformed", body(1024, false), &MALFORMED_PROBE).await;
    assert_eq!(
        actual,
        (
            StatusCode::BAD_REQUEST,
            r#"{"code":"request_malformed","error":"malformed request"}"#.into(),
            0
        )
    );
}
