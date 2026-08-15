use chrono::Duration as ChronoDuration;
use futures_util::{SinkExt, StreamExt};
use lotta_domain::Timestamp;
use lotta_testkit::clock::FakeClock;
use std::sync::Arc;
use tokio::time::{Duration, advance, timeout};
use tokio_tungstenite::{WebSocketStream, connect_async, tungstenite::Message};

use super::{ListenerHandle, start_listener_for_test};
use crate::config::ServerArgs;

const INTERVAL_MS: u64 = 10;

type Client = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn clock() -> Arc<FakeClock> {
    let timestamp = Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z").unwrap();
    Arc::new(FakeClock::new(timestamp))
}

async fn listener(clock: Arc<FakeClock>) -> ListenerHandle {
    let prepared = ServerArgs {
        listen_enabled: true,
        ..ServerArgs::default()
    }
    .prepare()
    .unwrap();
    start_listener_for_test(prepared, clock, 4096, INTERVAL_MS)
        .await
        .unwrap()
}

async fn connect(handle: &ListenerHandle) -> Client {
    connect_async(handle.websocket_url()).await.unwrap().0
}

async fn tick() {
    advance(Duration::from_millis(INTERVAL_MS)).await;
    tokio::task::yield_now().await;
}

async fn next(socket: &mut Client) -> Message {
    socket
        .next()
        .await
        .unwrap_or_else(|| panic!("socket ended"))
        .unwrap_or_else(|error| panic!("receive: {error}"))
}

#[tokio::test(start_paused = true)]
async fn actual_ping_and_explicit_pong_refresh_heartbeat() {
    let clock = clock();
    let handle = listener(clock.clone()).await;
    let mut socket = connect(&handle).await;
    tick().await;
    assert!(matches!(next(&mut socket).await, Message::Ping(_)));
    clock.advance(ChronoDuration::milliseconds(80_000)).unwrap();
    socket.send(Message::Pong(Vec::new().into())).await.unwrap();
    tokio::task::yield_now().await;
    tick().await;
    assert!(matches!(next(&mut socket).await, Message::Ping(_)));
    clock.advance(ChronoDuration::milliseconds(90_001)).unwrap();
    tick().await;
    loop {
        match socket.next().await {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(u16::from(frame.code), 1001);
                assert_eq!(frame.reason, "heartbeat expired");
                break;
            }
            Some(Ok(Message::Ping(_))) => {}
            Some(Err(_)) | None => break,
            other => panic!("expected close or EOF, got {other:?}"),
        }
    }
    drop(socket);
    timeout(Duration::from_secs(1), handle.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn half_open_socket_expires_without_reading_or_pong() {
    let clock = clock();
    let handle = listener(clock.clone()).await;
    let mut socket = connect(&handle).await;
    clock.advance(ChronoDuration::milliseconds(90_001)).unwrap();
    tick().await;
    loop {
        match next(&mut socket).await {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 1001);
                assert_eq!(frame.reason, "heartbeat expired");
                break;
            }
            Message::Ping(_) => {}
            other => panic!("unexpected message: {other:?}"),
        }
    }
    handle.wait().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_closes_two_active_sockets_and_wait_completes() {
    let clock = clock();
    let mut handle = listener(clock).await;
    let mut first = connect(&handle).await;
    let mut second = connect(&handle).await;
    handle.shutdown();
    let observe = async |socket: &mut Client| loop {
        match timeout(Duration::from_secs(1), socket.next())
            .await
            .unwrap()
        {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(u16::from(frame.code), 1001);
                break;
            }
            Some(Ok(Message::Ping(_))) => {}
            Some(Err(_)) | None => break,
            other => panic!("expected shutdown close or EOF, got {other:?}"),
        }
    };
    tokio::join!(observe(&mut first), observe(&mut second));
    timeout(Duration::from_secs(1), handle.wait())
        .await
        .unwrap()
        .unwrap();
}
