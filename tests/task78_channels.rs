use futures_util::{SinkExt as _, StreamExt as _};
use lotta_app_server::auth::channel_session::ChannelSessionAuthenticator;
use lotta_channels::{
    supervisor::{ChannelLaunchConfig, ChannelSupervisor},
    topology::ChannelStore,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio_tungstenite::tungstenite::Message;

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

fn canonical_store() -> (std::path::PathBuf, ChannelStore) {
    let root = std::env::temp_dir().join(format!(
        "lotta-task78-built-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let store = ChannelStore::under_letta_home(&root).unwrap();
    let channel = store.root().join("telegram");
    std::fs::create_dir(&channel).unwrap();
    std::fs::write(channel.join("config.yaml"), "token: redacted\n").unwrap();
    std::fs::write(
        channel.join("accounts.json"),
        concat!(
            r#"{"accounts":[{"channel_id":"telegram","account_id":"main","#,
            r#""enabled":true,"configured":true,"running":false,"dm_policy":"pairing","#,
            r#""allowed_users":[],"config":{},"created_at":"2026-01-01T00:00:00Z","#,
            r#""updated_at":"2026-01-01T00:00:00Z"}]}"#,
        ),
    )
    .unwrap();
    std::fs::write(
        channel.join("routing.yaml"),
        concat!(
            r#"{"routes":[{"channel_id":"telegram","account_id":"main","chat_id":"chat","#,
            r#""agent_id":"agent-local-a","conversation_id":"conversation-a","enabled":true,"#,
            r#""created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}]}"#,
        ),
    )
    .unwrap();
    (root, store)
}

#[tokio::test]
async fn built_lotta_channel_host_authenticates_starts_runtime_and_reaps() {
    let (root, store) = canonical_store();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let authenticated = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&authenticated);
    let server = tokio::spawn(async move {
        for _generation in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4_096];
            let length = loop {
                let length = stream.peek(&mut request).await.unwrap();
                if request[..length]
                    .windows(4)
                    .any(|window| window == b"\r\n\r\n")
                {
                    break length;
                }
                tokio::task::yield_now().await;
            };
            observed.store(
                String::from_utf8_lossy(&request[..length])
                    .to_ascii_lowercase()
                    .contains("authorization: bearer "),
                Ordering::SeqCst,
            );
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(message)) = socket.next().await {
                match message {
                    Message::Text(text) => {
                        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
                        assert_eq!(frame["type"], "runtime_start");
                        assert!(frame.get("recover_approvals").is_none());
                        assert!(frame.get("force_device_status").is_none());
                        socket
                            .send(Message::Text(
                                serde_json::json!({
                                    "type": "runtime_start_response",
                                    "request_id": frame["request_id"],
                                    "runtime": {
                                        "agent_id": "agent-local-a",
                                        "conversation_id": "conversation-a"
                                    }
                                })
                                .to_string()
                                .into(),
                            ))
                            .await
                            .unwrap();
                    }
                    Message::Close(_) => break,
                    Message::Ping(payload) => {
                        socket.send(Message::Pong(payload)).await.unwrap();
                    }
                    Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
                }
            }
        }
    });
    let authenticator = ChannelSessionAuthenticator::new();
    authenticator
        .bind_listener("127.0.0.1", "/channel-runtime", "task78-built-listener")
        .unwrap();
    let registry = Arc::new(lotta_tools::ToolRegistry::new([]).unwrap());
    let tools = Arc::new(lotta_tools::external::ChannelExternalToolManager::new(
        registry,
    ));
    let supervisor = ChannelSupervisor::start(ChannelLaunchConfig {
        executable: std::path::PathBuf::from(env!("CARGO_BIN_EXE_lotta")),
        store,
        websocket_url: format!("ws://127.0.0.1:{}/channel-runtime", address.port()),
        owner_prefix: "task78-built".into(),
        authenticator,
        tools,
    })
    .await
    .unwrap();
    assert!(supervisor.pid().is_some());
    assert!(authenticated.load(Ordering::SeqCst));
    let runtime = lotta_channels::control_plane::RuntimeKey {
        agent_id: "agent-local-a".into(),
        conversation_id: "conversation-a".into(),
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while supervisor.runtime_tools(&runtime).is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let generation_one = supervisor.pid().unwrap();
    assert!(
        std::process::Command::new("/bin/kill")
            .args(["-KILL", &generation_one.to_string()])
            .status()
            .unwrap()
            .success()
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if supervisor.pid().is_some_and(|pid| pid != generation_one)
                && supervisor.runtime_tools(&runtime).is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    supervisor.shutdown().await.unwrap();
    server.await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
