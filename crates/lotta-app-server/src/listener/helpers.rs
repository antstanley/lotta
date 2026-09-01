use super::{
    AppServerError, Body, CloseFrame, IntoResponse, Message, PreparedServer, Request, Response,
    SocketAddr, WebSocket,
};

pub(super) async fn send_close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    let _ = socket.send(Message::Close(Some(frame))).await;
}

pub(super) async fn not_found(_: Request<Body>) -> Response {
    AppServerError::NotFound.into_response()
}

pub(super) async fn method_not_allowed() -> Response {
    AppServerError::MethodNotAllowed.into_response()
}

#[cfg(test)]
pub(super) async fn test_forbidden() -> Response {
    AppServerError::Forbidden.into_response()
}

#[cfg(test)]
pub(super) async fn test_unavailable() -> Response {
    AppServerError::Unavailable.into_response()
}

#[cfg(test)]
pub(super) async fn test_internal() -> Response {
    AppServerError::Internal.into_response()
}

pub(super) fn format_bind_address(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub(super) fn resolved_urls(
    prepared: &PreparedServer,
    address: SocketAddr,
) -> (String, String, Option<String>) {
    let host = if prepared.host.contains(':') {
        format!("[{}]", prepared.host)
    } else {
        prepared.host.clone()
    };
    let base = format!("ws://{host}:{}", address.port());
    let websocket = format!("{base}{}", prepared.websocket_path);
    let openai = prepared
        .openai_api
        .then(|| format!("http://{host}:{}/v1", address.port()));
    (base, websocket, openai)
}
