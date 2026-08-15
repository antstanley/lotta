use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lotta_domain::{Clock, DomainError, Timestamp};

use super::{ListenerHandle, resolved_urls, start_listener};
use crate::config::ServerArgs;

const UPGRADE: &str = "Connection: Upgrade\r\nUpgrade: websocket\r\n\
Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";

struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56Z").unwrap()
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

fn args(listen: Option<&str>) -> ServerArgs {
    ServerArgs {
        listen: listen.map(str::to_owned),
        listen_enabled: true,
        ..ServerArgs::default()
    }
}

async fn launch(args: ServerArgs) -> ListenerHandle {
    start_listener(args.prepare().unwrap(), Arc::new(TestClock))
        .await
        .unwrap()
}

fn request(address: SocketAddr, target: &str, headers: &str) -> u16 {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let text = format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n");
    stream.write_all(text.as_bytes()).unwrap();
    let mut bytes = [0_u8; 512];
    let count = stream.read(&mut bytes).unwrap();
    let response = std::str::from_utf8(&bytes[..count]).unwrap();
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}

async fn status(handle: &ListenerHandle, target: &str, headers: &str) -> u16 {
    let address = handle.address();
    let target = target.to_owned();
    let headers = headers.to_owned();
    tokio::task::spawn_blocking(move || request(address, &target, &headers))
        .await
        .unwrap()
}

async fn stop(handle: ListenerHandle) {
    handle.wait().await.unwrap();
}

fn unique_token_file() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "lotta-listener-{}-{nonce}.token",
        std::process::id()
    ))
}

async fn launch_with_token() -> (ListenerHandle, PathBuf) {
    let path = unique_token_file();
    fs::write(&path, "correct-token\n").unwrap();
    let mut configured = args(Some("ws://127.0.0.1:0"));
    configured.ws_auth = Some("capability-token".into());
    configured.ws_token_file = Some(path.clone());
    (launch(configured).await, path)
}

#[tokio::test(flavor = "multi_thread")]
async fn bare_listen_resolves_actual_port_and_default_websocket() {
    let handle = launch(args(None)).await;
    let port = handle.address().port();
    assert_ne!(port, 0);
    assert_eq!(handle.base_url(), format!("ws://127.0.0.1:{port}"));
    assert_eq!(handle.websocket_url(), format!("ws://127.0.0.1:{port}/ws"));
    assert_eq!(handle.openai_url(), None);
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn omitted_url_port_uses_actual_ephemeral_port() {
    let handle = launch(args(Some("ws://127.0.0.1/custom"))).await;
    assert_ne!(handle.address().port(), 0);
    assert!(handle.websocket_url().ends_with("/custom"));
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_custom_path_is_exact() {
    let handle = launch(args(Some("ws://127.0.0.1:0/exact/path"))).await;
    assert_eq!(
        handle.websocket_url(),
        format!("ws://127.0.0.1:{}/exact/path", handle.address().port())
    );
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn url_root_advertises_ws_while_root_upgrade_is_accepted() {
    let handle = launch(args(Some("ws://127.0.0.1:0/"))).await;
    assert!(handle.websocket_url().ends_with("/ws"));
    assert_eq!(status(&handle, "/", UPGRADE).await, 101);
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_enabled_url_is_exact() {
    let mut configured = args(Some("ws://127.0.0.1:0"));
    configured.openai_api = true;
    let handle = launch(configured).await;
    assert_eq!(
        handle.openai_url(),
        Some(format!("http://127.0.0.1:{}/v1", handle.address().port()).as_str())
    );
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ipv4_urls_preserve_ipv4_authority() {
    let handle = launch(args(Some("ws://127.0.0.1:0/ws"))).await;
    assert!(handle.base_url().starts_with("ws://127.0.0.1:"));
    stop(handle).await;
}

#[test]
fn ipv6_urls_bracket_the_authority() {
    let prepared = args(Some("ws://[::1]:0/custom")).prepare().unwrap();
    let address: SocketAddr = "[::1]:43210".parse().unwrap();
    let (base, websocket, openai) = resolved_urls(&prepared, address);
    assert_eq!(base, "ws://[::1]:43210");
    assert_eq!(websocket, "ws://[::1]:43210/custom");
    assert_eq!(openai, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthenticated_loopback_upgrades_ws_without_origin() {
    let handle = launch(args(None)).await;
    assert_eq!(status(&handle, "/ws", UPGRADE).await, 101);
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_path_upgrades_and_default_ws_is_not_found() {
    let handle = launch(args(Some("ws://127.0.0.1:0/custom"))).await;
    assert_eq!(status(&handle, "/custom", UPGRADE).await, 101);
    assert_eq!(status(&handle, "/ws", UPGRADE).await, 404);
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn health_and_readiness_endpoints_return_ok() {
    let handle = launch(args(None)).await;
    assert_eq!(status(&handle, "/healthz", "").await, 200);
    assert_eq!(status(&handle, "/readyz", "").await, 200);
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_upgrade_returns_bad_request() {
    let handle = launch(args(None)).await;
    assert_eq!(status(&handle, "/ws", "Connection: Upgrade\r\n").await, 400);
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unrelated_path_returns_not_found() {
    let handle = launch(args(None)).await;
    assert_eq!(status(&handle, "/missing", UPGRADE).await, 404);
    stop(handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn capability_token_accepts_valid_authorization_with_or_without_origin() {
    let (handle, path) = launch_with_token().await;
    let auth = format!("{UPGRADE}Authorization: Bearer correct-token\r\n");
    assert_eq!(status(&handle, "/ws", &auth).await, 101);
    let origin = format!("{auth}Origin: http://localhost\r\n");
    assert_eq!(status(&handle, "/ws", &origin).await, 101);
    stop(handle).await;
    fs::remove_file(path).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn capability_token_rejects_wrong_and_missing_authorization() {
    let (handle, path) = launch_with_token().await;
    let wrong = format!("{UPGRADE}Authorization: Bearer wrong-token\r\n");
    assert_eq!(status(&handle, "/ws", &wrong).await, 401);
    assert_eq!(status(&handle, "/ws", UPGRADE).await, 401);
    stop(handle).await;
    fs::remove_file(path).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn origin_without_auth_policy_is_unauthorized() {
    let handle = launch(args(None)).await;
    let headers = format!("{UPGRADE}Origin: http://localhost\r\n");
    assert_eq!(status(&handle, "/ws", &headers).await, 401);
    stop(handle).await;
}
