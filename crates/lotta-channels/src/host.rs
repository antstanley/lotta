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
/// Maximum wait for one future-adapter outbound completion.
pub const CHANNEL_ADAPTER_DELIVERY_TIMEOUT_MS: u64 = 30_000;

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

type RoutedRuntime = lotta_domain::ChannelRoute;

/// Runs the supervised child over management stdin/stdout and public Runtime WebSocket.
///
/// # Errors
/// Returns a scrubbed protocol, bootstrap, runtime, or WebSocket failure.
pub async fn run() -> Result<(), HostError> {
    run_inner(None).await
}

/// Runs the actual Runtime WebSocket client with a bounded in-memory adapter port.
///
/// # Errors
/// Returns a scrubbed protocol, bootstrap, runtime, adapter, or WebSocket failure.
pub async fn run_with_adapter_port(
    adapter: crate::adapter::ChannelAdapterPort,
) -> Result<(), HostError> {
    run_inner(Some(adapter)).await
}

async fn run_inner(adapter: Option<crate::adapter::ChannelAdapterPort>) -> Result<(), HostError> {
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
    let publication_request =
        publish_runtime_tools(&mut management_output, &bootstrap, route.as_ref()).await?;
    if let Some(route) = &route {
        send_runtime_start(&mut ws_writer, route, &bootstrap.channels_root).await?;
    }
    let session = HostSession {
        bootstrap: &bootstrap,
        route,
        channels_request: &channels_request,
        publication_request,
        adapter,
    };
    run_session(
        session,
        &mut management_input,
        &mut management_output,
        &mut ws_writer,
        &mut ws_reader,
    )
    .await
}

struct HostSession<'a> {
    bootstrap: &'a Bootstrap,
    route: Option<RoutedRuntime>,
    channels_request: &'a str,
    publication_request: Option<String>,
    adapter: Option<crate::adapter::ChannelAdapterPort>,
}

async fn run_session<MI, MO, W, R>(
    session: HostSession<'_>,
    management_input: &mut BufReader<MI>,
    management_output: &mut MO,
    ws_writer: &mut W,
    ws_reader: &mut R,
) -> Result<(), HostError>
where
    MI: tokio::io::AsyncRead + Unpin,
    MO: tokio::io::AsyncWrite + Unpin,
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
    R: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let HostSession {
        bootstrap,
        route,
        channels_request,
        publication_request,
        adapter,
    } = session;
    let (mut adapter_input, adapter_output) = match adapter {
        Some(port) => (Some(port.inbound), Some(port.outbound)),
        None => (None, None),
    };
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
                    publication_request.as_deref(),
                    route.as_ref(),
                ).await? {
                    return Ok(());
                }
            }
            inbound = receive_adapter_input(&mut adapter_input), if started => {
                let Some(inbound) = inbound else { adapter_input = None; continue; };
                send_adapter_input(ws_writer, inbound).await?;
            }
            incoming = ws_reader.next() => {
                let message = incoming.ok_or(HostError::WebSocket)?
                    .map_err(|_| HostError::WebSocket)?;
                handle_websocket_message(
                    ws_writer,
                    message,
                    route.as_ref(),
                    adapter_output.as_ref(),
                    &mut started,
                )
                .await?;
            }
        }
    }
}

async fn handle_websocket_message<W>(
    writer: &mut W,
    message: Message,
    route: Option<&RoutedRuntime>,
    adapter: Option<&tokio::sync::mpsc::Sender<crate::adapter::MessageChannelDelivery>>,
    started: &mut bool,
) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    match message {
        Message::Ping(payload) => writer
            .send(Message::Pong(payload))
            .await
            .map_err(|_| HostError::WebSocket),
        Message::Text(text) => {
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|_| HostError::WebSocket)?;
            *started |= value["type"] == "runtime_start_response";
            handle_message_channel_call(writer, &value, route, adapter).await
        }
        Message::Close(_) => Err(HostError::WebSocket),
        Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => Ok(()),
    }
}

async fn handle_management<W, O>(
    frame: ParentFrame,
    output: &mut O,
    ws_writer: &mut W,
    bootstrap: &Bootstrap,
    channels_request: &str,
    publication_request: Option<&str>,
    route: Option<&RoutedRuntime>,
) -> Result<bool, HostError>
where
    O: tokio::io::AsyncWrite + Unpin,
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    match frame {
        ParentFrame::Shutdown { request_id } => {
            if let Some(route) = route {
                write_line(
                    output,
                    &ChildFrame::ReleaseRuntimeTools {
                        request_id: random_request_id("release-tools")?,
                        owner: bootstrap.owner.clone(),
                        runtime: runtime_key(route),
                    },
                )
                .await?;
            }
            let close = ws_writer
                .send(Message::Close(None))
                .await
                .map_err(|_| HostError::WebSocket);
            write_line(
                output,
                &ChildFrame::ShutdownComplete {
                    correlation_id: request_id,
                    owner: bootstrap.owner.clone(),
                },
            )
            .await?;
            close?;
            Ok(true)
        }
        ParentFrame::ChannelsResult {
            correlation_id,
            channels,
        } if correlation_id == channels_request && channels.len() <= 256 => Ok(false),
        ParentFrame::RuntimeToolsPublished { correlation_id }
            if publication_request == Some(correlation_id.as_str()) =>
        {
            Ok(false)
        }
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

async fn publish_runtime_tools<W>(
    output: &mut W,
    bootstrap: &Bootstrap,
    route: Option<&RoutedRuntime>,
) -> Result<Option<String>, HostError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let Some(route) = route else {
        return Ok(None);
    };
    let request_id = random_request_id("publish-tools")?;
    write_line(
        output,
        &ChildFrame::PublishRuntimeTools {
            request_id: request_id.clone(),
            owner: bootstrap.owner.clone(),
            runtime: runtime_key(route),
            tools: vec![crate::control_plane::RuntimeTool {
                name: "ChannelHostAvailability".into(),
                description: "Report bounded channel adapter availability".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            }],
        },
    )
    .await?;
    Ok(Some(request_id))
}

fn runtime_key(route: &RoutedRuntime) -> crate::control_plane::RuntimeKey {
    crate::control_plane::RuntimeKey {
        agent_id: route.agent_id.as_str().to_owned(),
        conversation_id: route.conversation_id.as_str().to_owned(),
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
            "client_info": {"name": "lotta-channel-host"}
        }),
    )
    .await
}

async fn receive_adapter_input(
    receiver: &mut Option<tokio::sync::mpsc::Receiver<crate::adapter::InboundChannelMessage>>,
) -> Option<crate::adapter::InboundChannelMessage> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn send_adapter_input<W>(
    writer: &mut W,
    message: crate::adapter::InboundChannelMessage,
) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    let request_id = random_request_id("input")?;
    send_json(
        writer,
        serde_json::json!({
            "type": "input",
            "request_id": request_id,
            "runtime": {
                "agent_id": message.route.agent_id,
                "conversation_id": message.route.conversation_id
            },
            "payload": message.payload
        }),
    )
    .await
}

async fn handle_message_channel_call<W>(
    writer: &mut W,
    value: &serde_json::Value,
    route: Option<&RoutedRuntime>,
    adapter: Option<&tokio::sync::mpsc::Sender<crate::adapter::MessageChannelDelivery>>,
) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    if value["type"] != "runtime_external_tool_call_request" {
        return Ok(());
    }
    if value["tool_name"] == "ChannelHostAvailability" {
        return send_unavailable_tool_result(writer, value).await;
    }
    if value["tool_name"] != "MessageChannel" {
        return Ok(());
    }
    let request_id = value["request_id"].as_str().ok_or(HostError::Runtime)?;
    let tool_call_id = value["tool_call_id"].as_str().ok_or(HostError::Runtime)?;
    let message = value["input"]["message"]
        .as_str()
        .ok_or(HostError::Runtime)?;
    let outcome = deliver_outbound(route, adapter, request_id, tool_call_id, message).await;
    let mut response = serde_json::json!({
        "type": "runtime_external_tool_call_response",
        "request_id": request_id,
        "runtime": value["runtime"].clone(),
        "tool_call_id": tool_call_id
    });
    match outcome {
        crate::adapter::MessageChannelResult::Delivered => {
            response["result"] = serde_json::json!({
                "content": [{"type": "text", "text": "delivered"}],
                "is_error": false
            });
        }
        crate::adapter::MessageChannelResult::Unavailable => {
            response["error"] = serde_json::json!("channel_adapter_unavailable");
        }
        crate::adapter::MessageChannelResult::Failed(reason) => {
            response["error"] = serde_json::json!(reason);
        }
    }
    send_json(writer, response).await
}

async fn send_unavailable_tool_result<W>(
    writer: &mut W,
    value: &serde_json::Value,
) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    let request_id = value["request_id"].as_str().ok_or(HostError::Runtime)?;
    let tool_call_id = value["tool_call_id"].as_str().ok_or(HostError::Runtime)?;
    send_json(
        writer,
        serde_json::json!({
            "type": "runtime_external_tool_call_response",
            "request_id": request_id,
            "runtime": value["runtime"].clone(),
            "tool_call_id": tool_call_id,
            "error": "channel_adapter_unavailable"
        }),
    )
    .await
}

async fn deliver_outbound(
    route: Option<&RoutedRuntime>,
    adapter: Option<&tokio::sync::mpsc::Sender<crate::adapter::MessageChannelDelivery>>,
    request_id: &str,
    tool_call_id: &str,
    message: &str,
) -> crate::adapter::MessageChannelResult {
    let (Some(route), Some(adapter)) = (route, adapter) else {
        return crate::adapter::MessageChannelResult::Unavailable;
    };
    let (delivery, completion) = crate::adapter::delivery(
        route.clone(),
        request_id.to_owned(),
        tool_call_id.to_owned(),
        message.to_owned(),
    );
    if adapter.try_send(delivery).is_err() {
        return crate::adapter::MessageChannelResult::Unavailable;
    }
    tokio::time::timeout(
        std::time::Duration::from_millis(CHANNEL_ADAPTER_DELIVERY_TIMEOUT_MS),
        completion,
    )
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or(crate::adapter::MessageChannelResult::Unavailable)
}

fn load_routed_runtime(root: &Path) -> Result<Option<RoutedRuntime>, HostError> {
    crate::state_store::ChannelStateStore::from_root(root)
        .restorable_routes()
        .map_err(|_| HostError::Runtime)
        .map(|routes| routes.into_iter().next())
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
