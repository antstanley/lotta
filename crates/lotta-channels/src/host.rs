//! Minimal compatibility channel host using only public WebSocket and management protocols.

use crate::control_plane::{
    ChildFrame, ControlError, ParentFrame, RuntimeKey, RuntimeTool, read_line, write_line,
};
use futures_util::{SinkExt, StreamExt};
use std::process;
use tokio::io::{BufReader, stdin, stdout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

/// Bounded WebSocket reconnect attempts before the supervised process exits.
pub const CHANNEL_WS_RECONNECT_ATTEMPTS_MAX: usize = 3;
/// Initial WebSocket reconnect delay.
pub const CHANNEL_WS_RECONNECT_BACKOFF_MS: u64 = 100;

/// Stable child-host failure.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Management protocol failed.
    #[error("channel host control protocol failed")]
    Control(#[from] ControlError),
    /// Bootstrap was absent or invalid.
    #[error("channel host bootstrap failed")]
    Bootstrap,
    /// Dedicated App Server WebSocket could not authenticate.
    #[error("channel host websocket failed")]
    WebSocket,
}

struct Bootstrap {
    correlation: String,
    owner: String,
    websocket_url: String,
    token: String,
    channels_root: String,
}

/// Runs the real supervised child mode over inherited stdin/stdout and public WebSocket.
///
/// # Errors
/// Returns a scrubbed protocol, bootstrap, or WebSocket failure.
pub async fn run() -> Result<(), HostError> {
    let mut input = BufReader::new(stdin());
    let bootstrap = read_bootstrap(&mut input).await?;
    verify_working_root(&bootstrap.channels_root)?;
    let mut request = bootstrap
        .websocket_url
        .as_str()
        .into_client_request()
        .map_err(|_| HostError::WebSocket)?;
    let authorization = format!("Bearer {}", bootstrap.token);
    request.headers_mut().insert(
        "Authorization",
        authorization.parse().map_err(|_| HostError::WebSocket)?,
    );
    let (socket, _) = connect_with_backoff(request).await?;
    let (mut ws_writer, mut ws_reader) = socket.split();
    let mut output = stdout();
    write_line(
        &mut output,
        &ChildFrame::Ready {
            correlation_id: bootstrap.correlation.clone(),
            owner: bootstrap.owner.clone(),
            pid: process::id(),
        },
    )
    .await?;
    publish_message_channel(&mut output, &bootstrap.owner).await?;

    loop {
        tokio::select! {
            frame = read_line::<_, ParentFrame>(&mut input) => {
                let Some(frame) = frame? else { break; };
                if let ParentFrame::Shutdown { request_id } = frame {
                    let _ = ws_writer.send(Message::Close(None)).await;
                    write_line(&mut output, &ChildFrame::ShutdownComplete {
                        correlation_id: request_id, owner: bootstrap.owner.clone(),
                    }).await?;
                    break;
                }
            }
            incoming = ws_reader.next() => {
                match incoming {
                    Some(Ok(Message::Ping(payload))) => {
                        ws_writer
                            .send(Message::Pong(payload))
                            .await
                            .map_err(|_| HostError::WebSocket)?;
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return Err(HostError::WebSocket),
                    Some(Ok(Message::Text(_) | Message::Binary(_) | Message::Pong(_)
                        | Message::Frame(_))) => {}
                }
            }
        }
    }
    Ok(())
}

async fn read_bootstrap<R>(input: &mut BufReader<R>) -> Result<Bootstrap, HostError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(frame) = read_line::<_, ParentFrame>(input).await? else {
        return Err(HostError::Bootstrap);
    };
    match frame {
        ParentFrame::Bootstrap {
            request_id,
            owner,
            websocket_url,
            token,
            channels_root,
        } => {
            if token.is_empty() || !websocket_url.starts_with("ws://127.0.0.1:") {
                return Err(HostError::Bootstrap);
            }
            Ok(Bootstrap {
                correlation: request_id,
                owner,
                websocket_url,
                token,
                channels_root,
            })
        }
        _ => Err(HostError::Bootstrap),
    }
}

async fn connect_with_backoff(
    request: tokio_tungstenite::tungstenite::http::Request<()>,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    ),
    HostError,
> {
    let mut delay = CHANNEL_WS_RECONNECT_BACKOFF_MS;
    for attempt in 0..CHANNEL_WS_RECONNECT_ATTEMPTS_MAX {
        match connect_async(clone_request(&request)?).await {
            Ok(connection) => return Ok(connection),
            Err(_) if attempt + 1 < CHANNEL_WS_RECONNECT_ATTEMPTS_MAX => {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                delay = delay.saturating_mul(2);
            }
            Err(_) => return Err(HostError::WebSocket),
        }
    }
    Err(HostError::WebSocket)
}

fn clone_request(
    request: &tokio_tungstenite::tungstenite::http::Request<()>,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, HostError> {
    let mut copy = request
        .uri()
        .to_string()
        .into_client_request()
        .map_err(|_| HostError::WebSocket)?;
    *copy.headers_mut() = request.headers().clone();
    Ok(copy)
}

async fn publish_message_channel<W>(output: &mut W, owner: &str) -> Result<(), HostError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let frame = ChildFrame::PublishRuntimeTools {
        request_id: "startup-message-channel".into(),
        owner: owner.into(),
        runtime: RuntimeKey {
            agent_id: "channel-host".into(),
            conversation_id: "channel-tools".into(),
        },
        tools: vec![RuntimeTool {
            name: "channel_send".into(),
            description: "Send a reply through MessageChannel".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"message": {"type": "string"}},
                "required": ["message"]
            }),
        }],
    };
    write_line(output, &frame).await.map_err(HostError::from)
}

fn verify_working_root(expected: &str) -> Result<(), HostError> {
    let current = std::env::current_dir().map_err(|_| HostError::Bootstrap)?;
    let expected = std::fs::canonicalize(expected).map_err(|_| HostError::Bootstrap)?;
    if current == expected {
        Ok(())
    } else {
        Err(HostError::Bootstrap)
    }
}
