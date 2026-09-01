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
    generation: u64,
    websocket_url: String,
    token: String,
    channels_root: PathBuf,
}

struct StartupPreparation {
    routes: Vec<RoutedRuntime>,
    channels_request: String,
    publication_requests: std::collections::BTreeSet<String>,
    runtime_starts: std::collections::BTreeMap<String, crate::control_plane::RuntimeKey>,
}

type RoutedRuntime = lotta_domain::ChannelRoute;

/// Runs the supervised child over management stdin/stdout and public Runtime WebSocket.
///
/// # Errors
/// Returns a scrubbed protocol, bootstrap, runtime, or WebSocket failure.
pub async fn run() -> Result<(), HostError> {
    let context = crate::adapter::HostContext::production();
    run_composed(context).await
}

/// Runs the real child host with an explicitly composed adapter factory context.
///
/// This is the production integration boundary used by dedicated-listener tests and
/// by future concrete adapter composition; it does not replace either transport.
///
/// # Errors
/// Returns the same bounded host failures as [`run`].
pub async fn run_composed(context: crate::adapter::HostContext) -> Result<(), HostError> {
    run_composed_io(context, stdin(), stdout(), true).await
}

/// Runs the production host composition over caller-owned NDJSON management streams.
///
/// This embedding boundary exists so a composition root can supply adapter factories while
/// retaining the real authenticated Runtime WebSocket and host session loop. Process-level cwd
/// enforcement remains the responsibility of an out-of-process launcher; filesystem isolation
/// probes and every protocol check still execute here.
///
/// # Errors
/// Returns the same bounded host failures as [`run`].
pub async fn run_composed_with_management<R, W>(
    context: crate::adapter::HostContext,
    management_input: R,
    management_output: W,
) -> Result<(), HostError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    run_composed_io(context, management_input, management_output, false).await
}

async fn run_composed_io<R, W>(
    context: crate::adapter::HostContext,
    management_input: R,
    management_output: W,
    verify_cwd: bool,
) -> Result<(), HostError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut management_input = BufReader::new(management_input);
    let bootstrap = read_bootstrap(&mut management_input).await?;
    if verify_cwd {
        verify_working_root(&bootstrap.channels_root)?;
    }
    let isolation = probe_isolation(&bootstrap.channels_root)?;
    let request = authenticated_request(&bootstrap)?;
    let (socket, _) = connect_with_backoff(request).await?;
    let (mut ws_writer, mut ws_reader) = socket.split();
    let mut management_output = management_output;
    write_line(
        &mut management_output,
        &ChildFrame::Ready {
            metadata: crate::control_plane::FrameMetadata::new(bootstrap.generation),
            correlation_id: bootstrap.correlation.clone(),
            owner: bootstrap.owner.clone(),
            pid: process::id(),
            channels_probe_success: isolation.0,
            parent_sibling_probe_denied: isolation.1,
        },
    )
    .await?;
    let startup = prepare_startup(
        &mut management_input,
        &mut management_output,
        &mut ws_writer,
        &bootstrap,
    )
    .await?;
    let StartupPreparation {
        routes,
        channels_request,
        publication_requests,
        runtime_starts,
    } = startup;
    let hub =
        crate::adapter::AdapterHub::compose(&routes, &context).map_err(|_| HostError::Runtime)?;
    let session = HostSession {
        bootstrap: &bootstrap,
        routes,
        channels_request: &channels_request,
        publication_requests,
        runtime_starts,
        hub,
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

async fn prepare_startup<R, O, W>(
    input: &mut BufReader<R>,
    output: &mut O,
    writer: &mut W,
    bootstrap: &Bootstrap,
) -> Result<StartupPreparation, HostError>
where
    R: tokio::io::AsyncRead + Unpin,
    O: tokio::io::AsyncWrite + Unpin,
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    let channels_request = random_request_id("channels")?;
    write_line(
        output,
        &ChildFrame::Channels {
            metadata: crate::control_plane::FrameMetadata::new(bootstrap.generation),
            request_id: channels_request.clone(),
            owner: bootstrap.owner.clone(),
        },
    )
    .await?;
    let routes = load_routed_runtimes(&bootstrap.channels_root)?;
    let publication_requests = publish_runtime_tools(output, bootstrap, &routes).await?;
    await_startup_management(input, bootstrap, &channels_request, &publication_requests).await?;
    let mut started_runtimes = std::collections::BTreeSet::new();
    let mut runtime_starts = std::collections::BTreeMap::new();
    for route in &routes {
        let runtime = runtime_key(route);
        if started_runtimes.insert(runtime.clone()) {
            let request_id = send_runtime_start(writer, route, &bootstrap.channels_root).await?;
            runtime_starts.insert(request_id, runtime);
        }
    }
    Ok(StartupPreparation {
        routes,
        channels_request,
        publication_requests,
        runtime_starts,
    })
}

async fn await_startup_management<R>(
    input: &mut BufReader<R>,
    bootstrap: &Bootstrap,
    channels_request: &str,
    publication_requests: &std::collections::BTreeSet<String>,
) -> Result<(), HostError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut channels_pending = true;
    let mut publications_pending = publication_requests.clone();
    while channels_pending || !publications_pending.is_empty() {
        let frame = tokio::time::timeout(
            std::time::Duration::from_millis(crate::control_plane::CONTROL_TIMEOUT_MS_MAX),
            read_line::<_, ParentFrame>(input),
        )
        .await
        .map_err(|_| HostError::Bootstrap)??
        .ok_or(HostError::Bootstrap)?;
        validate_parent_frame(&frame, bootstrap)?;
        match frame {
            ParentFrame::ChannelsResult {
                correlation_id,
                channels,
                ..
            } if channels_pending
                && correlation_id == channels_request
                && channels.len() <= crate::control_plane::CHANNEL_STATE_ROWS_MAX =>
            {
                channels_pending = false;
            }
            ParentFrame::RuntimeToolsPublished { correlation_id, .. }
                if publications_pending.remove(&correlation_id) => {}
            _ => return Err(HostError::Bootstrap),
        }
    }
    Ok(())
}

struct HostSession<'a> {
    bootstrap: &'a Bootstrap,
    routes: Vec<RoutedRuntime>,
    channels_request: &'a str,
    publication_requests: std::collections::BTreeSet<String>,
    runtime_starts: std::collections::BTreeMap<String, crate::control_plane::RuntimeKey>,
    hub: crate::adapter::AdapterHub,
}

fn take_deferred_input(
    deferred: &mut std::collections::VecDeque<crate::adapter::InboundChannelMessage>,
    started: &std::collections::BTreeSet<(String, String)>,
    hub: &crate::adapter::AdapterHub,
) -> Result<Option<crate::adapter::InboundChannelMessage>, HostError> {
    let Some(index) = deferred.iter().position(|message| {
        started.contains(&(
            message.route.agent_id.as_str().to_owned(),
            message.route.conversation_id.as_str().to_owned(),
        )) && !hub.runtime_is_active(&message.route)
    }) else {
        return Ok(None);
    };
    deferred.remove(index).ok_or(HostError::Runtime).map(Some)
}

async fn handle_adapter_inbound<W>(
    writer: &mut W,
    hub: &mut crate::adapter::AdapterHub,
    deferred: &mut std::collections::VecDeque<crate::adapter::InboundChannelMessage>,
    started: &std::collections::BTreeSet<(String, String)>,
    inbound: crate::adapter::InboundChannelMessage,
) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    let runtime = (
        inbound.route.agent_id.as_str().to_owned(),
        inbound.route.conversation_id.as_str().to_owned(),
    );
    if !started.contains(&runtime) || hub.runtime_is_active(&inbound.route) {
        if deferred.len() >= crate::adapter::CHANNEL_ADAPTER_INPUT_CORRELATIONS_MAX {
            return Err(HostError::Runtime);
        }
        deferred.push_back(inbound);
        return Ok(());
    }
    dispatch_adapter_input(writer, hub, &inbound).await
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
        routes,
        channels_request,
        publication_requests,
        mut runtime_starts,
        mut hub,
    } = session;
    let mut started = std::collections::BTreeSet::new();
    let mut deferred = std::collections::VecDeque::new();
    loop {
        if let Some(message) = take_deferred_input(&mut deferred, &started, &hub)? {
            dispatch_adapter_input(ws_writer, &mut hub, &message).await?;
            continue;
        }
        tokio::select! {
            frame = read_line::<_, ParentFrame>(management_input) => {
                let Some(frame) = frame? else { return Ok(()); };
                if handle_management(
                    frame,
                    management_output,
                    ws_writer,
                    bootstrap,
                    channels_request,
                    &publication_requests,
                    &routes,
                ).await? {
                    return Ok(());
                }
            }
            inbound = hub.receive_inbound() => {
                let Some(inbound) = inbound else { continue; };
                handle_adapter_inbound(
                    ws_writer, &mut hub, &mut deferred, &started, inbound,
                ).await?;
            }
            incoming = ws_reader.next() => {
                let message = incoming.ok_or(HostError::WebSocket)?
                    .map_err(|_| HostError::WebSocket)?;
                handle_websocket_message(
                    ws_writer,
                    message,
                    &routes,
                    &mut hub,
                    &mut started,
                    &mut runtime_starts,
                ).await?;
            }
        }
    }
}

async fn handle_websocket_message<W>(
    writer: &mut W,
    message: Message,
    routes: &[RoutedRuntime],
    hub: &mut crate::adapter::AdapterHub,
    started: &mut std::collections::BTreeSet<(String, String)>,
    runtime_starts: &mut std::collections::BTreeMap<String, crate::control_plane::RuntimeKey>,
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
            if value["type"] == "runtime_start_response" {
                let request_id = value["request_id"].as_str().ok_or(HostError::Runtime)?;
                let expected = runtime_starts
                    .remove(request_id)
                    .ok_or(HostError::Runtime)?;
                let actual = runtime_value_key(&value)?;
                if expected.agent_id != actual.0
                    || expected.conversation_id != actual.1
                    || value.get("success") == Some(&serde_json::Value::Bool(false))
                    || value.get("error").is_some_and(|error| !error.is_null())
                    || !started.insert(actual)
                {
                    return Err(HostError::Runtime);
                }
                return Ok(());
            }
            if hub
                .accept_runtime_event(&value)
                .map_err(|_| HostError::Runtime)?
            {
                return Ok(());
            }
            handle_message_channel_call(writer, &value, routes, hub).await
        }
        Message::Close(_) => Err(HostError::WebSocket),
        Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => Ok(()),
    }
}

fn runtime_value_key(value: &serde_json::Value) -> Result<(String, String), HostError> {
    let runtime = &value["runtime"];
    let agent = runtime["agent_id"].as_str().ok_or(HostError::Runtime)?;
    let conversation = runtime["conversation_id"]
        .as_str()
        .ok_or(HostError::Runtime)?;
    Ok((agent.to_owned(), conversation.to_owned()))
}

async fn handle_management<W, O>(
    frame: ParentFrame,
    output: &mut O,
    ws_writer: &mut W,
    bootstrap: &Bootstrap,
    channels_request: &str,
    publication_requests: &std::collections::BTreeSet<String>,
    routes: &[RoutedRuntime],
) -> Result<bool, HostError>
where
    O: tokio::io::AsyncWrite + Unpin,
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    validate_parent_frame(&frame, bootstrap)?;
    match frame {
        ParentFrame::Shutdown { request_id, .. } => {
            let mut released = std::collections::BTreeSet::new();
            for route in routes {
                let runtime = runtime_key(route);
                if !released.insert(runtime.clone()) {
                    continue;
                }
                write_line(
                    output,
                    &ChildFrame::ReleaseRuntimeTools {
                        metadata: crate::control_plane::FrameMetadata::new(bootstrap.generation),
                        request_id: random_request_id("release-tools")?,
                        owner: bootstrap.owner.clone(),
                        runtime,
                    },
                )
                .await?;
            }
            // Acknowledge the management request before touching the independent
            // Runtime transport. A dropped WebSocket must not erase a graceful ack.
            write_line(
                output,
                &ChildFrame::ShutdownComplete {
                    metadata: crate::control_plane::FrameMetadata::new(bootstrap.generation),
                    correlation_id: request_id,
                    owner: bootstrap.owner.clone(),
                },
            )
            .await?;
            ws_writer
                .send(Message::Close(None))
                .await
                .map_err(|_| HostError::WebSocket)?;
            Ok(true)
        }
        ParentFrame::ChannelsResult {
            correlation_id,
            channels,
            ..
        } if correlation_id == channels_request && channels.len() <= 256 => Ok(false),
        ParentFrame::RuntimeToolsPublished { correlation_id, .. }
            if publication_requests.contains(&correlation_id) =>
        {
            Ok(false)
        }
        ParentFrame::RuntimeToolsPublished { .. }
        | ParentFrame::RuntimeToolsReleased { .. }
        | ParentFrame::Bootstrap { .. }
        | ParentFrame::ChannelsResult { .. } => Err(HostError::Bootstrap),
    }
}

fn validate_parent_frame(frame: &ParentFrame, bootstrap: &Bootstrap) -> Result<(), HostError> {
    let (metadata, owner) = match frame {
        ParentFrame::Bootstrap {
            metadata, owner, ..
        }
        | ParentFrame::ChannelsResult {
            metadata, owner, ..
        }
        | ParentFrame::RuntimeToolsPublished {
            metadata, owner, ..
        }
        | ParentFrame::RuntimeToolsReleased {
            metadata, owner, ..
        }
        | ParentFrame::Shutdown {
            metadata, owner, ..
        } => (*metadata, owner),
    };
    if owner != &bootstrap.owner || !valid_metadata(metadata, bootstrap.generation) {
        return Err(HostError::Bootstrap);
    }
    Ok(())
}

fn valid_metadata(metadata: crate::control_plane::FrameMetadata, generation: u64) -> bool {
    metadata.version == crate::control_plane::CONTROL_PROTOCOL_VERSION
        && metadata.generation == generation
        && metadata.capability == crate::control_plane::ManagementCapability::ChannelManagement
        && metadata.timeout_ms > 0
        && metadata.timeout_ms <= crate::control_plane::CONTROL_TIMEOUT_MS_MAX
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
            metadata,
            request_id,
            owner,
            websocket_url,
            token,
            channels_root,
        } => {
            let root = PathBuf::from(channels_root);
            if !valid_metadata(metadata, metadata.generation)
                || token.is_empty()
                || owner.is_empty()
                || !websocket_url.starts_with("ws://127.0.0.1:")
                || !root.is_absolute()
            {
                return Err(HostError::Bootstrap);
            }
            Ok(Bootstrap {
                correlation: request_id,
                owner,
                generation: metadata.generation,
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
    routes: &[RoutedRuntime],
) -> Result<std::collections::BTreeSet<String>, HostError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut requests = std::collections::BTreeSet::new();
    let mut runtimes = std::collections::BTreeSet::new();
    for route in routes {
        let runtime = runtime_key(route);
        if !runtimes.insert(runtime.clone()) {
            continue;
        }
        if requests.len() >= crate::control_plane::CONTROL_CORRELATIONS_MAX {
            return Err(HostError::Runtime);
        }
        let request_id = random_request_id("publish-tools")?;
        write_line(
            output,
            &ChildFrame::PublishRuntimeTools {
                metadata: crate::control_plane::FrameMetadata::new(bootstrap.generation),
                request_id: request_id.clone(),
                owner: bootstrap.owner.clone(),
                runtime,
                tools: vec![crate::control_plane::RuntimeTool {
                    name: "ChannelHostAvailability".into(),
                    description: "Report bounded channel adapter availability".into(),
                    parameters: serde_json::json!({
                        "type": "object", "properties": {}, "additionalProperties": false
                    }),
                }],
            },
        )
        .await?;
        requests.insert(request_id);
    }
    Ok(requests)
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
) -> Result<String, HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    let root = root.to_str().ok_or(HostError::Runtime)?;
    let request_id = random_request_id("runtime-start")?;
    send_json(
        writer,
        serde_json::json!({
            "type": "runtime_start",
            "request_id": request_id.clone(),
            "agent_id": runtime.agent_id,
            "conversation_id": runtime.conversation_id,
            "cwd": root,
            "mode": "strict",
            "workspace_sandbox": {"root": root, "isolation_root": root},
            "external_tools": [message_channel_descriptor()],
            "client_info": {"name": "lotta-channel-host"}
        }),
    )
    .await?;
    Ok(request_id)
}

async fn dispatch_adapter_input<W>(
    writer: &mut W,
    hub: &mut crate::adapter::AdapterHub,
    message: &crate::adapter::InboundChannelMessage,
) -> Result<(), HostError>
where
    W: futures_util::Sink<Message> + Unpin,
    W::Error: std::fmt::Debug,
{
    let request_id = random_request_id("input")?;
    hub.record_input(request_id.clone(), &message.route)
        .map_err(|_| HostError::Runtime)?;
    let outcome = send_json(
        writer,
        serde_json::json!({
            "type": "input",
            "request_id": request_id.clone(),
            "runtime": {
                "agent_id": message.route.agent_id,
                "conversation_id": message.route.conversation_id
            },
            "payload": message.payload
        }),
    )
    .await;
    if outcome.is_err() {
        let _ = hub.cancel_input(&request_id);
    }
    outcome
}

async fn handle_message_channel_call<W>(
    writer: &mut W,
    value: &serde_json::Value,
    routes: &[RoutedRuntime],
    hub: &crate::adapter::AdapterHub,
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
    let runtime = runtime_value_key(value)?;
    let active = hub
        .active_route_key(&runtime.0, &runtime.1)
        .ok_or(HostError::Runtime)?
        .clone();
    let route = routes
        .iter()
        .find(|route| crate::adapter::CanonicalRouteKey::from_route(route) == active)
        .ok_or(HostError::Runtime)?;
    let outcome = hub.deliver(route, request_id, tool_call_id, message).await;
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

fn load_routed_runtimes(root: &Path) -> Result<Vec<RoutedRuntime>, HostError> {
    crate::state_store::ChannelStateStore::from_root(root)
        .restorable_routes()
        .map_err(|_| HostError::Runtime)
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

fn probe_isolation(root: &Path) -> Result<(bool, bool), HostError> {
    let name = format!(".isolation-probe-{}", process::id());
    let allowed = root.join(&name);
    std::fs::write(&allowed, b"probe").map_err(|_| HostError::Bootstrap)?;
    std::fs::remove_file(&allowed).map_err(|_| HostError::Bootstrap)?;
    let parent = root.parent().ok_or(HostError::Bootstrap)?;
    let denied = parent.join("backend").join(name);
    let parent_sibling_probe_denied = match std::fs::write(&denied, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(denied);
            false
        }
        Err(_) => true,
    };
    if !parent_sibling_probe_denied {
        return Err(HostError::Bootstrap);
    }
    Ok((true, true))
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
