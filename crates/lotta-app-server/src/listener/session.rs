use super::{
    AppServerError, Arc, Heartbeat, ListenerState, Message, OutboundBatch,
    WS_OUTBOUND_FRAMES_PER_CONNECTION_MAX, WebSocket, channel_runtime_command_allowed, close_code,
    dispatch_output, dispatch_typed_failure, dispatch_value, event_sink,
    handle_channel_tool_response, handle_external_frame, initialize_socket_liveness, lock_router,
    mpsc, publish_channel_runtime_tool, reject_stale_channel_socket, route_command, send_close,
    send_outbound_batch,
};

pub(super) async fn serve_socket(
    mut socket: WebSocket,
    state: Arc<ListenerState>,
    reconnect_identity: Option<crate::ws::connection::ReconnectIdentity>,
    channel_principal: Option<crate::auth::channel_session::ChannelPrincipal>,
) {
    let (sender, mut receiver) = mpsc::channel(WS_OUTBOUND_FRAMES_PER_CONNECTION_MAX);
    let Ok(connection_id) = open_connection(&state, sender, reconnect_identity.as_ref()) else {
        return;
    };
    let (mut heartbeat, mut interval, mut channel_revision) =
        initialize_socket_liveness(&state).await;
    if reject_stale_channel_socket(&mut socket, &state, channel_principal.as_ref()).await {
        close_connection(&state, connection_id).await;
        return;
    }
    loop {
        tokio::select! {
            () = channel_authority_changed(&mut channel_revision) => {
                let admitted = state.channel_session.as_ref().zip(channel_principal.as_ref())
                    .is_some_and(|(auth, principal)| auth.admits(principal));
                if !admitted {
                    send_close(&mut socket, close_code::POLICY, "channel authority revoked").await;
                    break;
                }
            }
            () = state.shutdown.cancelled() => {
                send_close(&mut socket, close_code::AWAY, "server shutdown").await;
                break;
            }
            Some(batch) = receiver.recv() => {
                if send_outbound_batch(&mut socket, batch).await.is_err() {
                    break;
                }
            }
            _ = interval.tick() => {
                if state.channel_session.as_ref().zip(channel_principal.as_ref())
                    .is_some_and(|(auth, principal)| !auth.admits(principal))
                {
                    send_close(&mut socket, close_code::POLICY, "channel authority expired").await;
                    break;
                }
                if heartbeat.is_expired(state.clock.as_ref()) {
                    send_close(&mut socket, close_code::AWAY, "heartbeat expired").await;
                    break;
                }
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                let keep_open = handle_incoming(
                    incoming,
                    &mut socket,
                    &mut heartbeat,
                    &state,
                    connection_id,
                    channel_principal.as_ref(),
                ).await;
                if !keep_open {
                    break;
                }
            }
        }
    }
    close_connection(&state, connection_id).await;
}

pub(super) async fn channel_authority_changed(
    revision: &mut Option<tokio::sync::watch::Receiver<u64>>,
) {
    if let Some(revision) = revision {
        let _ = revision.changed().await;
    } else {
        std::future::pending::<()>().await;
    }
}

pub(super) fn open_connection(
    state: &ListenerState,
    sender: mpsc::Sender<OutboundBatch>,
    reconnect_identity: Option<&crate::ws::connection::ReconnectIdentity>,
) -> Result<crate::ws::ConnectionId, AppServerError> {
    let id = {
        let mut router = lock_router(&state.runtime_router)?;
        router.connections.open_authenticated(reconnect_identity)?
    };
    if let Err(error) = prepare_outbound(state, id, sender) {
        lock_router(&state.runtime_router)?.connections.close(id);
        return Err(error);
    }
    if let Err(error) = lock_router(&state.runtime_router)?
        .connections
        .initialize(id)
    {
        remove_outbound(state, id)?;
        lock_router(&state.runtime_router)?.connections.close(id);
        return Err(error);
    }
    state.introspection.register_authenticated(id);
    Ok(id)
}

pub(super) fn prepare_outbound(
    state: &ListenerState,
    id: crate::ws::ConnectionId,
    sender: mpsc::Sender<OutboundBatch>,
) -> Result<(), AppServerError> {
    let mut outbound = state
        .outbound
        .lock()
        .map_err(|_| AppServerError::Internal)?;
    outbound
        .try_reserve(1)
        .map_err(|_| AppServerError::Unavailable)?;
    outbound.insert(id, sender);
    Ok(())
}

pub(super) fn remove_outbound(
    state: &ListenerState,
    id: crate::ws::ConnectionId,
) -> Result<(), AppServerError> {
    state
        .outbound
        .lock()
        .map_err(|_| AppServerError::Internal)?
        .remove(&id);
    Ok(())
}

pub(super) async fn publish_expired_subscription_updates(state: &ListenerState) {
    let updates = if let Ok(mut router) = state.runtime_router.lock() {
        router.connections.expire_suspended();
        let scopes = router.connections.take_expired_subscriptions();
        scopes
            .into_iter()
            .map(|scope| {
                let count = router.connections.subscription_count_for(&scope);
                (scope, count)
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    publish_subscription_updates(state, updates).await;
}

pub(super) async fn publish_shutdown_subscription_updates(state: &ListenerState) {
    let updates = if let Ok(mut router) = state.runtime_router.lock() {
        router
            .connections
            .shutdown()
            .into_iter()
            .map(|scope| (scope, 0))
            .collect()
    } else {
        Vec::new()
    };
    publish_subscription_updates(state, updates).await;
}

pub(super) async fn publish_subscription_updates(
    state: &ListenerState,
    updates: Vec<(lotta_domain::RuntimeScope, usize)>,
) {
    for (scope, count) in updates {
        if state
            .runtime_service
            .runtime_subscription_changed(scope.clone(), count)
            .await
            .is_err()
            && let Ok(mut router) = state.runtime_router.lock()
        {
            router.connections.retry_subscription_update(scope);
        }
    }
}

pub(super) async fn close_connection(state: &ListenerState, id: crate::ws::ConnectionId) {
    state.external_tools.disconnect(id);
    state.terminals.disconnect(id);
    state.files.disconnect(id);
    state.introspection.unregister(id);
    // Device work is cancellation-bound: cancel first, then reap its joins.
    state.devices.disconnect(id).await;
    let updates = if let Ok(mut router) = state.runtime_router.lock() {
        let mut scopes = router.connections.subscriptions_of(id);
        if state.shutdown.is_cancelled() {
            router.connections.close(id);
        } else {
            router.connections.suspend(id);
        }
        for expired in router.connections.take_expired_subscriptions() {
            if !scopes.contains(&expired) {
                scopes.push(expired);
            }
        }
        scopes
            .into_iter()
            .map(|scope| {
                let count = router.connections.subscription_count_for(&scope);
                (scope, count)
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let _ = remove_outbound(state, id);
    publish_subscription_updates(state, updates).await;
}

pub(super) async fn handle_incoming(
    incoming: Option<Result<Message, axum::Error>>,
    socket: &mut WebSocket,
    heartbeat: &mut Heartbeat,
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    channel_principal: Option<&crate::auth::channel_session::ChannelPrincipal>,
) -> bool {
    match incoming {
        Some(Ok(Message::Pong(_))) => {
            heartbeat.record_pong(state.clock.as_ref());
            true
        }
        Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await.is_ok(),
        Some(Ok(Message::Text(text))) => {
            if state.channel_host_protocol_only
                && crate::framing::decode_text(&text).is_ok_and(|frame| {
                    let kind = frame.value.get("type").and_then(serde_json::Value::as_str);
                    !kind.is_some_and(
                        crate::auth::channel_session::ChannelSessionAuthenticator::command_allowed,
                    ) && kind != Some("runtime_external_tool_call_response")
                })
            {
                send_close(socket, close_code::POLICY, "runtime plane only").await;
                return false;
            }
            handle_text(&text, state, connection_id, channel_principal).await
        }
        Some(Ok(Message::Binary(_))) => {
            send_close(socket, close_code::UNSUPPORTED, "binary unsupported").await;
            false
        }
        Some(Ok(Message::Close(_))) | None => false,
        Some(Err(error)) => {
            if let Some((code, reason)) = websocket_error_close(error) {
                send_close(socket, code, reason).await;
            }
            false
        }
    }
}

pub(super) fn websocket_error_close(error: axum::Error) -> Option<(u16, &'static str)> {
    let inner = error.into_inner();
    let Some(error) = inner.downcast_ref::<tungstenite::Error>() else {
        return Some((close_code::ERROR, "internal websocket error"));
    };
    match error {
        tungstenite::Error::Capacity(_) => Some((close_code::SIZE, "message too large")),
        tungstenite::Error::Protocol(_) => Some((close_code::PROTOCOL, "websocket protocol error")),
        tungstenite::Error::Utf8(_) => Some((close_code::INVALID, "invalid UTF-8")),
        tungstenite::Error::Io(_)
        | tungstenite::Error::Tls(_)
        | tungstenite::Error::WriteBufferFull(_) => None,
        _ => Some((close_code::ERROR, "internal websocket error")),
    }
}

pub(super) fn channel_principal_admitted(
    state: &ListenerState,
    principal: Option<&crate::auth::channel_session::ChannelPrincipal>,
) -> bool {
    state
        .channel_session
        .as_ref()
        .is_none_or(|authenticator| principal.is_some_and(|value| authenticator.admits(value)))
}

pub(super) async fn handle_text(
    text: &str,
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    channel_principal: Option<&crate::auth::channel_session::ChannelPrincipal>,
) -> bool {
    if !channel_principal_admitted(state, channel_principal) {
        return false;
    }
    let frame = match crate::framing::decode_text(text) {
        Ok(frame) => frame,
        Err(error) => return dispatch_value(state, connection_id, &error).is_ok(),
    };
    if state.channel_host_protocol_only
        && !frame
            .value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(crate::auth::channel_session::ChannelSessionAuthenticator::command_allowed)
    {
        return dispatch_typed_failure(state, connection_id, &frame).is_ok();
    }
    let command = match crate::ws::command::decode(&frame) {
        Ok(Some(command)) => command,
        Ok(None)
            if state.channel_host_protocol_only
                && frame.value["type"] == "runtime_external_tool_call_response" =>
        {
            return handle_channel_tool_response(state, channel_principal, &frame);
        }
        Ok(None) if state.channel_host_protocol_only => {
            return dispatch_typed_failure(state, connection_id, &frame).is_ok();
        }
        Ok(None) => return handle_external_frame(state, connection_id, &frame),
        Err(error) => return dispatch_value(state, connection_id, &error).is_ok(),
    };
    if state.channel_host_protocol_only
        && !channel_runtime_command_allowed(state, &command, &frame.value)
    {
        return dispatch_typed_failure(state, connection_id, &frame).is_ok();
    }
    if state.channel_host_protocol_only
        && !publish_channel_runtime_tool(state, connection_id, &command, channel_principal)
    {
        return dispatch_typed_failure(state, connection_id, &frame).is_ok();
    }
    let routed = route_command(
        state.runtime_router.clone(),
        state.runtime_service.clone(),
        connection_id,
        command,
    )
    .await;
    let Ok((output, deferred)) = routed else {
        return dispatch_typed_failure(state, connection_id, &frame).is_ok();
    };
    if dispatch_output(state, connection_id, &output).is_err() {
        return false;
    }
    if crate::ws::router::acknowledge_recovery(state.runtime_service.as_ref(), &output)
        .await
        .is_err()
    {
        return false;
    }
    match deferred {
        Some(deferred) => spawn_deferred_turn(state, connection_id, &frame, deferred).await,
        None => true,
    }
}

pub(super) async fn spawn_deferred_turn(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
    deferred: crate::ws::router::DeferredInput,
) -> bool {
    let sink = event_sink(state);
    let Ok(command) = serde_json::from_value(frame.value.clone()) else {
        return dispatch_typed_failure(state, connection_id, frame).is_ok();
    };
    let controller = state.turn_controller.clone();
    let task_state = state.clone();
    let failure_frame = frame.clone();
    let turn_cancellation = state.turns.cancellation();
    let turn = Box::pin(async move {
        let result = if controller.is_control_continuation(&deferred) {
            task_state
                .runtime_service
                .continue_input(deferred.scope, deferred.continuation, sink)
                .await
        } else {
            controller
                .submit_turn(command, deferred, turn_cancellation, sink)
                .await
        };
        if result.is_err() {
            let _ = dispatch_typed_failure(&task_state, connection_id, &failure_frame);
        }
    });
    state.turns.spawn(turn).await.is_ok()
}
