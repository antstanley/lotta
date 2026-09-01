//! Real supervised channel-host client for the public Runtime WebSocket.

use crate::control_plane::{ChildFrame, ControlError, ParentFrame, read_line, write_line};
use futures_util::{SinkExt, StreamExt};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    process,
};
use tokio::io::{BufReader, stdin, stdout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

/// Bounded WebSocket reconnect attempts before the supervised process exits.
pub const CHANNEL_WS_RECONNECT_ATTEMPTS_MAX: usize = 3;
/// Initial WebSocket reconnect delay.
pub const CHANNEL_WS_RECONNECT_BACKOFF_MS: u64 = 100;
/// Maximum bytes read from a pending adapter input fixture.
pub const CHANNEL_PENDING_INPUT_BYTES_MAX: usize = 1024 * 1024;
/// Maximum bytes retained in the local `MessageChannel` outbox.
pub const CHANNEL_OUTBOX_BYTES_MAX: u64 = 16 * 1024 * 1024;

/// Stable child-host failure.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Management protocol failed.
    #[error("channel host control protocol failed")]
    Control(#[from] ControlError),
    /// Bootstrap was absent or invalid.
    #[error("channel host bootstrap failed")]
    Bootstrap,
    /// Dedicated App Server WebSocket failed.
    #[error("channel host websocket failed")]
    WebSocket,
    /// Canonical routed runtime state was malformed.
    #[error("channel host runtime state failed")]
    Runtime,
}

struct Bootstrap {
    correlation: String,
    owner: String,
    websocket_url: String,
    token: String,
    channels_root: PathBuf,
}

#[derive(Clone)]
struct RoutedRuntime {
    agent_id: String,
    conversation_id: String,
}

/// Runs the supervised child over management stdin/stdout and public Runtime WebSocket.
///
/// # Errors
/// Returns a scrubbed protocol, bootstrap, runtime, or WebSocket failure.
pub async fn run() -> Result<(), HostError> {
    let mut management_input = BufReader::new(stdin());
    let bootstrap = read_bootstrap(&mut management_input).await?;
    verify_working_root(&bootstrap.channels_root)?;
    let request = authenticated_request(&bootstrap)?;
    let (socket, _) = connect_with_backoff(request).await?;
    let (mut ws_writer, mut ws_reader) = socket.split();
    let mut management_output = stdout();
    write_line(
        &mut management_output,
        &ChildFrame::Ready {
            correlation_id: bootstrap.correlation.clone(),
            owner: bootstrap.owner.clone(),
            pid: process::id(),
        },
    )
    .await?;
    let channels_request = random_request_id("channels")?;
    write_line(
        &mut management_output,
        &ChildFrame::Channels {
            request_id: channels_request.clone(),
            owner: bootstrap.owner.clone(),
        },
    )
    .await?;
    let route = load_routed_runtime(&bootstrap.channels_root)?;
    if let Some(route) = &route {
        send_runtime_start(&mut ws_writer, route, &bootstrap.channels_root).await?;
    }
    run_session(
        &bootstrap,
        &mut management_input,
        &mut management_output,
        &mut ws_writer,
        &mut ws_reader,
        route,
        &channels_request,
    )
    .await
}

async fn run_session<MI, MO, W, R>(
    bootstrap: &Bootstrap,
    management_input: &mut BufReader<MI>,
    management_output: &mut MO,
    ws_writer: &mut W,
    ws_reader: &mut R,
    route: Option<RoutedRuntime>,
    channels_request: &str,
) -> Result<(), HostError>
where
    MI: tokio::io::AsyncRead + Unpin,
    MO: tokio::io::AsyncWrite + Unpin,
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
    R: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut started = false;
    loop {
        tokio::select! {
            frame = read_line::<_, ParentFrame>(management_input) => {
                let Some(frame) = frame? else { return Ok(()); };
                if handle_management(
                    frame,
                    management_output,
                    ws_writer,
                    bootstrap,
                    channels_request,
                ).await? {
                    return Ok(());
                }
            }
            incoming = ws_reader.next() => {
                let message = incoming.ok_or(HostError::WebSocket)?
                    .map_err(|_| HostError::WebSocket)?;
                match message {
                    Message::Ping(payload) => ws_writer.send(Message::Pong(payload)).await
                        .map_err(|_| HostError::WebSocket)?,
                    Message::Text(text) => {
                        let value: serde_json::Value = serde_json::from_str(&text)
                            .map_err(|_| HostError::WebSocket)?;
                        if value["type"] == "runtime_start_response" && !started {
                            started = true;
                            if let Some(route) = &route {
                                send_pending_input(
                                    ws_writer,
                                    route,
                                    &bootstrap.channels_root,
                                )
                                .await?;
                            }
                        }
                        handle_message_channel_call(
                            ws_writer,
                            &bootstrap.channels_root,
                            &value,
                        )
                        .await?;
                    }
                    Message::Close(_) => return Err(HostError::WebSocket),
                    Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
                }
            }
        }
    }
}

async fn handle_management<W, O>(
    frame: ParentFrame,
    output: &mut O,
    ws_writer: &mut W,
    bootstrap: &Bootstrap,
    channels_request: &str,
) -> Result<bool, HostError>
where
    O: tokio::io::AsyncWrite + Unpin,
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    match frame {
        ParentFrame::Shutdown { request_id } => {
            let _ = ws_writer.send(Message::Close(None)).await;
            write_line(
                output,
                &ChildFrame::ShutdownComplete {
                    correlation_id: request_id,
                    owner: bootstrap.owner.clone(),
                },
            )
            .await?;
            Ok(true)
        }
        ParentFrame::ChannelsResult {
            correlation_id,
            channels,
        } if correlation_id == channels_request && channels.len() <= 256 => Ok(false),
        ParentFrame::RuntimeToolsPublished { .. }
        | ParentFrame::RuntimeToolsReleased { .. }
        | ParentFrame::Bootstrap { .. }
        | ParentFrame::ChannelsResult { .. } => Err(HostError::Bootstrap),
    }
}

fn authenticated_request(
    bootstrap: &Bootstrap,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, HostError> {
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
    request.headers_mut().insert(
        "x-lotta-reconnect-id",
        format!("channel-{}", process::id())
            .parse()
            .map_err(|_| HostError::WebSocket)?,
    );
    Ok(request)
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
            let root = PathBuf::from(channels_root);
            if token.is_empty()
                || !websocket_url.starts_with("ws://127.0.0.1:")
                || !root.is_absolute()
            {
                return Err(HostError::Bootstrap);
            }
            Ok(Bootstrap {
                correlation: request_id,
                owner,
                websocket_url,
                token,
                channels_root: root,
            })
        }
        _ => Err(HostError::Bootstrap),
    }
}

async fn send_runtime_start<W>(
    writer: &mut W,
    runtime: &RoutedRuntime,
    root: &Path,
) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    let root = root.to_str().ok_or(HostError::Runtime)?;
    send_json(
        writer,
        serde_json::json!({
            "type": "runtime_start",
            "request_id": random_request_id("runtime-start")?,
            "agent_id": runtime.agent_id,
            "conversation_id": runtime.conversation_id,
            "cwd": root,
            "mode": "strict",
            "workspace_sandbox": {"root": root, "isolation_root": root},
            "external_tools": [message_channel_descriptor()],
            "recover_approvals": true,
            "force_device_status": true,
            "client_info": {"name": "lotta-channel-host"}
        }),
    )
    .await
}

async fn send_pending_input<W>(
    writer: &mut W,
    runtime: &RoutedRuntime,
    root: &Path,
) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    let path = root.join("pending-runtime-input.json");
    if !path.exists() {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| HostError::Runtime)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > CHANNEL_PENDING_INPUT_BYTES_MAX as u64
    {
        return Err(HostError::Runtime);
    }
    let bytes = std::fs::read(path).map_err(|_| HostError::Runtime)?;
    let payload: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| HostError::Runtime)?;
    let request_id = random_request_id("input")?;
    send_json(
        writer,
        serde_json::json!({
            "type": "input",
            "request_id": request_id,
            "runtime": {
                "agent_id": runtime.agent_id,
                "conversation_id": runtime.conversation_id
            },
            "payload": payload
        }),
    )
    .await
}

async fn handle_message_channel_call<W>(
    writer: &mut W,
    root: &Path,
    value: &serde_json::Value,
) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    if value["type"] != "runtime_external_tool_call_request"
        || value["tool_name"] != "MessageChannel"
    {
        return Ok(());
    }
    let request_id = value["request_id"].as_str().ok_or(HostError::Runtime)?;
    let message = value["input"]["message"]
        .as_str()
        .ok_or(HostError::Runtime)?;
    append_outbox(root, message)?;
    send_json(
        writer,
        serde_json::json!({
            "type": "runtime_external_tool_call_response",
            "request_id": request_id,
            "runtime": value["runtime"].clone(),
            "tool_call_id": value["tool_call_id"].clone(),
            "result": {"content": [{"type": "text", "text": "delivered"}], "is_error": false}
        }),
    )
    .await
}

fn append_outbox(root: &Path, message: &str) -> Result<(), HostError> {
    use std::io::Write as _;
    let path = root.join("message-channel-outbox.jsonl");
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(HostError::Runtime);
    }
    if path
        .metadata()
        .is_ok_and(|metadata| metadata.len() > CHANNEL_OUTBOX_BYTES_MAX)
    {
        return Err(HostError::Runtime);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| HostError::Runtime)?;
    serde_json::to_writer(&mut file, &serde_json::json!({"message": message}))
        .map_err(|_| HostError::Runtime)?;
    file.write_all(b"\n").map_err(|_| HostError::Runtime)
}

fn load_routed_runtime(root: &Path) -> Result<Option<RoutedRuntime>, HostError> {
    for entry in std::fs::read_dir(root).map_err(|_| HostError::Runtime)? {
        let entry = entry.map_err(|_| HostError::Runtime)?;
        if !entry.file_type().map_err(|_| HostError::Runtime)?.is_dir() {
            continue;
        }
        let path = entry.path().join("routing.yaml");
        if !path.exists() {
            continue;
        }
        let text = std::fs::read_to_string(path).map_err(|_| HostError::Runtime)?;
        if let Some(route) = json_route(&text)? {
            return Ok(Some(route));
        }
        for record in text.split("\n  - ") {
            if yaml_value(record, "enabled").as_deref() != Some("true") {
                continue;
            }
            let agent_id = yaml_value(record, "agentId").or_else(|| yaml_value(record, "agent_id"));
            let conversation_id = yaml_value(record, "conversationId")
                .or_else(|| yaml_value(record, "conversation_id"));
            if let (Some(agent_id), Some(conversation_id)) = (agent_id, conversation_id) {
                return Ok(Some(RoutedRuntime {
                    agent_id,
                    conversation_id,
                }));
            }
        }
    }
    Ok(None)
}

fn json_route(text: &str) -> Result<Option<RoutedRuntime>, HostError> {
    if !text.trim_start().starts_with('{') {
        return Ok(None);
    }
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| HostError::Runtime)?;
    let routes = value
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .ok_or(HostError::Runtime)?;
    for route in routes {
        if route.get("enabled").and_then(serde_json::Value::as_bool) != Some(true) {
            continue;
        }
        let agent_id = route
            .get("agentId")
            .or_else(|| route.get("agent_id"))
            .and_then(serde_json::Value::as_str);
        let conversation_id = route
            .get("conversationId")
            .or_else(|| route.get("conversation_id"))
            .and_then(serde_json::Value::as_str);
        if let (Some(agent_id), Some(conversation_id)) = (agent_id, conversation_id) {
            return Ok(Some(RoutedRuntime {
                agent_id: agent_id.to_owned(),
                conversation_id: conversation_id.to_owned(),
            }));
        }
    }
    Ok(None)
}

fn yaml_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| {
            let (candidate, value) = line.trim().split_once(':')?;
            (candidate == key).then(|| value.trim().trim_matches(['\'', '"']).to_owned())
        })
        .filter(|value| !value.is_empty() && value.len() <= 256)
}

fn message_channel_descriptor() -> serde_json::Value {
    serde_json::json!({
        "name": "MessageChannel",
        "description": "Send a visible reply through the originating channel",
        "parameters": {
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"],
            "additionalProperties": false
        }
    })
}

async fn send_json<W>(writer: &mut W, value: serde_json::Value) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    writer
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|_| HostError::WebSocket)
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

fn verify_working_root(expected: &Path) -> Result<(), HostError> {
    let current = std::env::current_dir().map_err(|_| HostError::Bootstrap)?;
    let expected = std::fs::canonicalize(expected).map_err(|_| HostError::Bootstrap)?;
    (current == expected)
        .then_some(())
        .ok_or(HostError::Bootstrap)
}

fn random_request_id(prefix: &str) -> Result<String, HostError> {
    let mut random = [0_u8; 12];
    getrandom::fill(&mut random).map_err(|_| HostError::Runtime)?;
    let mut value = String::with_capacity(prefix.len() + 25);
    value.push_str(prefix);
    value.push('-');
    for byte in random {
        write!(&mut value, "{byte:02x}").map_err(|_| HostError::Runtime)?;
    }
    Ok(value)
}
