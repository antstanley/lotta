use futures_util::{SinkExt, StreamExt};
use lotta_domain::Timestamp;
use lotta_testkit::clock::FakeClock;
use serde_json::Value;
use std::sync::Arc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::start_listener_for_test;
use crate::config::ServerArgs;

const FRAME_CAP: usize = 1024;
const LONG_PING_MS: u64 = 3_600_000;

fn clock() -> Arc<FakeClock> {
    let timestamp = Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z")
        .unwrap_or_else(|error| panic!("timestamp: {error}"));
    Arc::new(FakeClock::new(timestamp))
}

async fn listener(frame_cap: usize) -> super::ListenerHandle {
    let args = ServerArgs {
        listen_enabled: true,
        ..ServerArgs::default()
    };
    let prepared = args
        .prepare()
        .unwrap_or_else(|error| panic!("prepare: {error}"));
    start_listener_for_test(prepared, clock(), frame_cap, LONG_PING_MS)
        .await
        .unwrap_or_else(|error| panic!("listener: {error}"))
}

async fn next_message<S>(socket: &mut S) -> Message
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    socket
        .next()
        .await
        .unwrap_or_else(|| panic!("socket ended"))
        .unwrap_or_else(|error| panic!("websocket receive: {error}"))
}

#[tokio::test]
async fn exact_frame_cap_returns_protocol_error_and_socket_stays_usable() {
    let handle = listener(FRAME_CAP).await;
    let (mut socket, _) = connect_async(handle.websocket_url())
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"));
    socket
        .send(Message::Text("x".repeat(FRAME_CAP).into()))
        .await
        .unwrap();
    let Message::Text(body) = next_message(&mut socket).await else {
        panic!("expected text protocol error");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["code"],
        "malformed_json"
    );
    socket.send(Message::Text("{}".into())).await.unwrap();
    socket.send(Message::Ping(Vec::new().into())).await.unwrap();
    assert!(matches!(next_message(&mut socket).await, Message::Pong(_)));
    handle.wait().await.unwrap();
}

#[tokio::test]
async fn frame_cap_plus_one_closes_with_1009() {
    let handle = listener(FRAME_CAP).await;
    let (mut socket, _) = connect_async(handle.websocket_url()).await.unwrap();
    socket
        .send(Message::Text("x".repeat(FRAME_CAP + 1).into()))
        .await
        .unwrap();
    let Message::Close(Some(frame)) = next_message(&mut socket).await else {
        panic!("expected close frame");
    };
    assert_eq!(u16::from(frame.code), 1009);
    assert_eq!(frame.reason, "message too large");
    handle.wait().await.unwrap();
}

#[tokio::test]
async fn nested_request_id_257_returns_correlated_error_and_remains_usable() {
    let handle = listener(4096).await;
    let (mut socket, _) = connect_async(handle.websocket_url()).await.unwrap();
    let outer = "outer-request";
    let text = serde_json::json!({
        "type": "input",
        "request_id": outer,
        "payload": {
            "kind": "approval_response",
            "request_id": "n".repeat(257),
        },
    })
    .to_string();
    socket.send(Message::Text(text.into())).await.unwrap();
    let Message::Text(body) = next_message(&mut socket).await else {
        panic!("expected protocol error");
    };
    let value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["type"], "protocol_error");
    assert_eq!(value["code"], "request_id_too_long");
    assert_eq!(value["message"], "request_id is too long");
    assert_eq!(value["request_id"], outer);
    socket.send(Message::Ping(Vec::new().into())).await.unwrap();
    assert!(matches!(next_message(&mut socket).await, Message::Pong(_)));
    handle.wait().await.unwrap();
}

#[test]
fn websocket_error_classifier_maps_concrete_classes() {
    use axum::extract::ws::close_code;
    use tungstenite::{Error, error::ProtocolError};
    let classify = |error| super::websocket_error_close(axum::Error::new(error)).0;
    assert_eq!(
        classify(Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)),
        Some(close_code::PROTOCOL)
    );
    assert_eq!(
        classify(Error::Utf8("bad".into())),
        Some(close_code::INVALID)
    );
    assert_eq!(
        classify(Error::Capacity(
            tungstenite::error::CapacityError::MessageTooLong {
                size: 2,
                max_size: 1
            }
        )),
        Some(close_code::SIZE)
    );
    assert_eq!(classify(Error::Io(std::io::Error::other("closed"))), None);
}
