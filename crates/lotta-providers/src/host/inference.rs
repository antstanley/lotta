use super::protocol::{HostAuth, HostOptions};
use super::provider::HostProvider;
use super::tests::{load_host_case, owner, test_config};
use crate::connections::{ConnectionManager, ProviderAuthStore};
use lotta_runtime::ports::provider_event_channel;
use lotta_testkit::contract::fixtures::provider_request;
use lotta_testkit::fixtures::providers::replay_provider;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn public_nonfixture_stream_uses_real_pi_translation() {
    let mut case = load_host_case("openai-compatible/happy-tool");
    let raw = case.raw_stream.as_bytes().to_vec();
    let (base_url, server) = serve_once(raw).await;
    let (config, scratch) = test_config();
    let options = HostOptions {
        base_url: Some(format!("{base_url}/v1")),
        ..HostOptions::default()
    };
    let provider = HostProvider::spawn(
        config,
        owner(),
        HostAuth::ApiKey {
            value: "fixture-credential".into(),
        },
        options,
    )
    .await
    .unwrap();
    provider
        .register_adapter(openai_descriptor())
        .await
        .unwrap();
    case.request.model.provider_id = lotta_domain::NonEmptyString::new("host-live").unwrap();
    case.request.model.handle =
        lotta_domain::NonEmptyString::new("host-live/fixture-model").unwrap();
    case.expected_trace = case.host_expected_trace.take().unwrap();
    replay_provider(&provider, case).await.unwrap();
    server.await.unwrap();
    provider.shutdown().unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

#[tokio::test]
async fn public_load_hosted_streams_persisted_credential_through_real_pi() {
    let raw = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hosted\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    )
    .as_bytes()
    .to_vec();
    let (base_url, server) = serve_once_authenticated(raw, "persisted-host-secret").await;
    let (config, scratch) = test_config();
    let paths = lotta_store::StorePaths::new(
        std::env::temp_dir().join(format!("lotta-hosted-manager-{}", std::process::id())),
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(paths.root());
    let descriptor = openai_descriptor();
    let auth = serde_json::to_vec(&json!({
        "version": 1,
        "providers": {"host-live": {
            "id": "local-provider-host-live",
            "name": "host-live",
            "provider_type": "host-live",
            "provider_category": "byok",
            "auth": {"type": "api", "key": "persisted-host-secret"},
            "base_url": format!("{base_url}/v1"),
            "host_descriptor": descriptor,
            "created_at": "2026-08-14T00:00:00.000Z",
            "updated_at": "2026-08-14T00:00:00.000Z"
        }}
    }))
    .unwrap();
    lotta_store::atomic_write(
        &paths.provider_auth(),
        &auth,
        lotta_store::WriteMode::ProviderAuth,
    )
    .unwrap();
    let manager = ConnectionManager::load_hosted(
        ProviderAuthStore::new(paths.clone()),
        config,
        owner(),
        std::collections::BTreeMap::new(),
    )
    .unwrap();
    let mut request = provider_request(tokio_util::sync::CancellationToken::new());
    request.model.provider_id = lotta_domain::NonEmptyString::new("host-live").unwrap();
    request.model.handle = lotta_domain::NonEmptyString::new("host-live/fixture-model").unwrap();
    let (sink, mut receiver) = provider_event_channel(16, &request.cancellation).unwrap();
    manager
        .stream("local-provider-host-live", request, sink)
        .await
        .unwrap();
    let mut normalized = Vec::new();
    while let Some(event) = receiver.receive().await.unwrap() {
        normalized.push(format!("{event:?}"));
    }
    assert!(normalized.iter().any(|event| event.contains("hosted")));
    assert!(normalized.iter().any(|event| event.contains("Stop")));
    server.await.unwrap();
    std::fs::remove_dir_all(paths.root()).unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

fn openai_descriptor() -> serde_json::Value {
    json!({
        "id":"host-live","name":"Live fixture","owner":"host-test",
        "adapter":{"type":"pi_ai_api","api":"openai-completions"},
        "models":[{
            "id":"fixture-model","name":"Fixture Model","api":"openai-completions",
            "provider":"host-live","reasoning":false,"input":["text","image"],
            "contextWindow":8192,"maxTokens":1024,
            "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"tiers":[]}
        }]
    })
}

async fn serve_once_authenticated(
    raw: Vec<u8>,
    secret: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        assert!(request_text.to_ascii_lowercase().contains(&format!(
            "authorization: bearer {}",
            secret.to_ascii_lowercase()
        )));
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            raw.len()
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(&raw).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn serve_once(raw: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        assert!(request.starts_with(b"POST /v1/chat/completions HTTP/1.1\r\n"));
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            raw.len()
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(&raw).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0, "request closed before body completed");
        bytes.extend_from_slice(&chunk[..read]);
        let Some(header_end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            })
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        if bytes.len() >= header_end + 4 + length {
            return bytes;
        }
    }
}
