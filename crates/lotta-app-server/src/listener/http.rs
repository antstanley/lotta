use super::{
    AppServerError, Arc, Body, HTTP_BODY_BYTES_MAX, HeaderMap, Heartbeat, IntoResponse, Json,
    ListenerState, Next, Request, Response, Router, State, WebSocket, WebSocketUpgrade,
    WebSocketUpgradeRejection, close_code, get, method_not_allowed, middleware, not_found, origin,
    post, publish_expired_subscription_updates, send_close, serve_socket,
};

#[cfg(test)]
use super::{test_forbidden, test_internal, test_unavailable};

pub(super) fn build_router(path: &str, state: Arc<ListenerState>) -> Router {
    if state.channel_host_protocol_only {
        return Router::new()
            .route(path, get(upgrade).fallback(method_not_allowed))
            .with_state(state)
            .fallback(not_found)
            .layer(axum::extract::DefaultBodyLimit::max(HTTP_BODY_BYTES_MAX));
    }
    let protected = protected_http_router(&state);
    let router = Router::new()
        .route("/", get(upgrade).fallback(method_not_allowed))
        .route(
            "/healthz",
            get(|| async { "ok\n" }).fallback(method_not_allowed),
        )
        .route(
            "/readyz",
            get(|| async { "ok\n" }).fallback(method_not_allowed),
        )
        .merge(protected);
    let router = if path == "/" {
        router
    } else {
        router.route(path, get(upgrade).fallback(method_not_allowed))
    };
    #[cfg(test)]
    let router = router
        .route("/__test/forbidden", get(test_forbidden))
        .route("/__test/unavailable", get(test_unavailable))
        .route("/__test/internal", get(test_internal));
    router
        .with_state(state)
        .fallback(not_found)
        .layer(axum::extract::DefaultBodyLimit::max(HTTP_BODY_BYTES_MAX))
}

pub(super) fn protected_http_router(state: &Arc<ListenerState>) -> Router<Arc<ListenerState>> {
    let router = Router::new().route(
        "/app-server-info",
        get(app_server_info).fallback(method_not_allowed),
    );
    let router = if state.openai_api {
        router
            .route(
                "/v1/models",
                get(openai_models).fallback(openai_method_not_allowed),
            )
            .route(
                "/v1/chat/completions",
                post(openai_chat_completions).fallback(openai_method_not_allowed),
            )
            .route(
                "/v1/responses",
                post(openai_responses).fallback(openai_method_not_allowed),
            )
    } else {
        router
    };
    router.layer(middleware::from_fn_with_state(
        Arc::clone(state),
        authorize_http,
    ))
}

pub(super) async fn authorize_http(
    State(state): State<Arc<ListenerState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    match authorize_listener_headers(&state, request.headers()) {
        Ok(()) => next.run(request).await,
        Err(error) if request.uri().path().starts_with("/v1/") => crate::openai::errors::response(
            error.status(),
            crate::openai::errors::authentication_error(),
        ),
        Err(error) => error.into_response(),
    }
}

pub(super) async fn app_server_info(
    State(state): State<Arc<ListenerState>>,
) -> Json<crate::ws::introspection::AppServerInfoResponseMessage> {
    Json(state.introspection.response("http-info"))
}

pub(super) async fn openai_models(State(state): State<Arc<ListenerState>>) -> Response {
    match crate::openai::models::list(&state.agents).await {
        Ok(response) => response.into_response(),
        Err(error) => error.into_response(),
    }
}

pub(super) async fn openai_chat_completions(
    State(state): State<Arc<ListenerState>>,
    headers: HeaderMap,
    body: crate::openai::chat::ChatJson,
) -> Response {
    crate::openai::chat::complete(Arc::clone(&state.openai_chat), headers, body).await
}

pub(super) async fn openai_responses(
    State(state): State<Arc<ListenerState>>,
    headers: HeaderMap,
    body: crate::openai::responses::ResponsesJson,
) -> Response {
    crate::openai::responses::respond(Arc::clone(&state.openai_responses), headers, body).await
}

pub(super) async fn openai_method_not_allowed() -> Response {
    crate::openai::errors::response(
        axum::http::StatusCode::METHOD_NOT_ALLOWED,
        crate::openai::errors::invalid_request("method not allowed"),
    )
}

pub(super) fn authorize_listener_headers(
    state: &ListenerState,
    headers: &HeaderMap,
) -> Result<(), AppServerError> {
    if let Some(authenticator) = &state.channel_session {
        authenticator.authenticate(headers)?;
        return Ok(());
    }
    state.auth.authorize(headers, state.clock.as_ref())?;
    origin::enforce(headers, &state.auth)?;
    Ok(())
}

pub(super) async fn upgrade(
    State(state): State<Arc<ListenerState>>,
    headers: HeaderMap,
    websocket: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if let Err(error) = authorize_listener_headers(&state, &headers) {
        return error.into_response();
    }
    let Ok(websocket) = websocket else {
        return AppServerError::Malformed.into_response();
    };
    let channel_principal = state
        .channel_session
        .as_ref()
        .and_then(|authenticator| authenticator.authenticate(&headers).ok());
    let reconnect_identity =
        authenticated_reconnect_identity(&state, &headers, channel_principal.as_ref());
    let connection = state.clone();
    websocket
        .max_frame_size(state.limits.frame_bytes)
        .max_message_size(state.limits.frame_bytes)
        .on_failed_upgrade(|_| {})
        .on_upgrade(move |socket| {
            serve_socket(socket, connection, reconnect_identity, channel_principal)
        })
        .into_response()
}

/// Maximum queued outbound frames for one live connection.
pub const WS_OUTBOUND_FRAMES_PER_CONNECTION_MAX: usize = 256;

pub(super) const RECONNECT_CLIENT_ID_HEADER: &str = "x-lotta-reconnect-id";
pub(crate) const RECONNECT_CLIENT_ID_BYTES_MAX: usize = 256;

pub(super) fn authenticated_reconnect_identity(
    state: &ListenerState,
    headers: &HeaderMap,
    channel: Option<&crate::auth::channel_session::ChannelPrincipal>,
) -> Option<crate::ws::connection::ReconnectIdentity> {
    if let Some(channel) = channel {
        let principal = channel.reconnect_principal();
        return Some(crate::ws::connection::ReconnectIdentity {
            listener_instance: state.listener_instance.clone(),
            client_id: principal.clone(),
            principal,
        });
    }
    authenticated_client_reconnect_identity(state, headers)
}

pub(super) fn authenticated_client_reconnect_identity(
    state: &ListenerState,
    headers: &HeaderMap,
) -> Option<crate::ws::connection::ReconnectIdentity> {
    let client_id = headers
        .get(RECONNECT_CLIENT_ID_HEADER)?
        .to_str()
        .ok()?
        .trim();
    if client_id.is_empty()
        || client_id.len() > RECONNECT_CLIENT_ID_BYTES_MAX
        || !client_id.is_ascii()
        || client_id.bytes().any(|byte| byte.is_ascii_control())
    {
        return None;
    }
    let principal = state.auth.principal(headers).ok()?;
    Some(crate::ws::connection::ReconnectIdentity {
        listener_instance: state.listener_instance.clone(),
        principal,
        client_id: client_id.to_owned(),
    })
}

pub(super) type ChannelRevision = Option<tokio::sync::watch::Receiver<u64>>;

pub(super) async fn initialize_socket_liveness(
    state: &ListenerState,
) -> (Heartbeat, tokio::time::Interval, ChannelRevision) {
    publish_expired_subscription_updates(state).await;
    let heartbeat = Heartbeat::new(state.clock.as_ref());
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(
        state.limits.ping_interval_ms,
    ));
    interval.tick().await;
    let revision = state
        .channel_session
        .as_ref()
        .map(crate::auth::channel_session::ChannelSessionAuthenticator::subscribe);
    (heartbeat, interval, revision)
}

pub(super) async fn reject_stale_channel_socket(
    socket: &mut WebSocket,
    state: &ListenerState,
    principal: Option<&crate::auth::channel_session::ChannelPrincipal>,
) -> bool {
    let authority_invalid = state
        .channel_session
        .as_ref()
        .zip(principal)
        .is_some_and(|(auth, principal)| !auth.admits(principal));
    if authority_invalid {
        send_close(socket, close_code::POLICY, "channel authority revoked").await;
    }
    authority_invalid
}
