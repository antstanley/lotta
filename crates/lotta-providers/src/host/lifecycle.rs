use super::protocol::{HostAuth, HostOptions};
use super::provider::HostProvider;
use super::tests::{load_host_case, owner, test_config};
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{ProviderPort, provider_event_channel};
use lotta_testkit::fixtures::providers::replay_provider;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn cancellation_reaps_child_and_followup_stream_succeeds() {
    let healthy = load_host_case("openai-compatible/happy-tool");
    let (base_url, accepted, server) = two_request_server(healthy.raw_stream.clone()).await;
    let provider = provider(&base_url).await;
    let mut cancelled = load_host_case("openai-compatible/happy-tool");
    point_at_host(&mut cancelled);
    let token = cancelled.request.cancellation.clone();
    let (sink, _receiver) = provider_event_channel(16, &token).unwrap();
    let cancel = async move {
        accepted.await.unwrap();
        token.cancel();
    };
    let (result, ()) = tokio::join!(provider.stream(cancelled.request, sink), cancel);
    assert!(matches!(result, Err(RuntimeError::Cancelled { .. })));

    let mut followup = healthy;
    point_at_host(&mut followup);
    followup.expected_trace = followup.host_expected_trace.take().unwrap();
    replay_provider(&provider, followup).await.unwrap();
    server.await.unwrap();
    provider.shutdown().unwrap();
}

async fn provider(base_url: &str) -> HostProvider {
    let (config, _) = test_config();
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
    provider.register_adapter(descriptor()).await.unwrap();
    provider
}

fn point_at_host(case: &mut lotta_testkit::fixtures::providers::ProviderCase) {
    case.request.model.provider_id = lotta_domain::NonEmptyString::new("host-life").unwrap();
    case.request.model.handle =
        lotta_domain::NonEmptyString::new("host-life/fixture-model").unwrap();
}

fn descriptor() -> serde_json::Value {
    json!({
        "id":"host-life","name":"Lifecycle","owner":"host-test",
        "adapter":{"type":"pi_ai_api","api":"openai-completions"},
        "models":[{
            "id":"fixture-model","name":"Fixture Model","api":"openai-completions",
            "provider":"host-life","reasoning":false,"input":["text","image"],
            "contextWindow":8192,"maxTokens":1024,
            "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"tiers":[]}
        }]
    })
}

async fn two_request_server(
    raw: String,
) -> (
    String,
    tokio::sync::oneshot::Receiver<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_request(&mut first).await;
        accepted_tx.send(()).unwrap();
        wait_for_close(&mut first).await;
        let (mut second, _) = listener.accept().await.unwrap();
        read_request(&mut second).await;
        write_response(&mut second, raw.as_bytes()).await;
    });
    (format!("http://{address}"), accepted_rx, server)
}

async fn read_request(socket: &mut TcpStream) {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.windows(4).any(|value| value == b"\r\n\r\n") {
            return;
        }
    }
}

async fn wait_for_close(socket: &mut TcpStream) {
    let mut byte = [0_u8; 1];
    let closed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if socket.read(&mut byte).await.unwrap() == 0 {
                return;
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "cancelled child was not reaped");
}

async fn write_response(socket: &mut TcpStream, raw: &[u8]) {
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        raw.len()
    );
    socket.write_all(head.as_bytes()).await.unwrap();
    socket.write_all(raw).await.unwrap();
    socket.shutdown().await.unwrap();
}
