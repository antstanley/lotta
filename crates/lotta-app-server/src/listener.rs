use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{State, WebSocketUpgrade, ws::WebSocket},
    http::{HeaderMap, Request},
    response::{IntoResponse, Response},
    routing::get,
};
use lotta_domain::Clock;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

use crate::{
    auth::origin,
    config::{PreparedServer, is_loopback_host},
    error::AppServerError,
};

#[cfg(test)]
#[path = "listener/tests/url_resolution.rs"]
mod url_resolution;

/// Explicit websocket frame and message ceiling.
pub const WS_FRAME_BYTES_MAX: usize = 100 * 1024 * 1024;

struct ListenerState {
    auth: crate::auth::AuthPolicy,
    clock: Arc<dyn Clock + Send + Sync>,
}

/// Owned running listener with resolved URLs and graceful shutdown.
pub struct ListenerHandle {
    address: SocketAddr,
    base_url: String,
    websocket_url: String,
    openai_url: Option<String>,
    shutdown_sender: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), AppServerError>>,
}

impl ListenerHandle {
    /// Returns the actual bound address, including an OS-selected port.
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    /// Returns the resolved HTTP authority represented with the baseline `ws` scheme.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the resolved configured websocket endpoint.
    #[must_use]
    pub fn websocket_url(&self) -> &str {
        &self.websocket_url
    }

    /// Returns the optional OpenAI-compatible HTTP base.
    #[must_use]
    pub fn openai_url(&self) -> Option<&str> {
        self.openai_url.as_deref()
    }

    /// Requests graceful listener shutdown.
    pub fn shutdown(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
    }

    /// Waits for the owned serve task.
    ///
    /// # Errors
    /// Returns a stable task or listener failure if graceful serving fails.
    pub async fn wait(mut self) -> Result<(), AppServerError> {
        self.shutdown();
        self.task.await.map_err(|_| AppServerError::Task)?
    }
}

/// Binds and starts an Axum WebSocket listener after all policy preparation.
///
/// # Errors
/// Returns a stable listener error when bind or local address discovery fails.
pub async fn start_listener(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
) -> Result<ListenerHandle, AppServerError> {
    if !is_loopback_host(&prepared.host) && prepared.auth.is_none() {
        return Err(AppServerError::Config(
            "non-loopback listeners require websocket authentication",
        ));
    }
    let bind = format_bind_address(&prepared.host, prepared.port);
    let listener = TcpListener::bind(&bind)
        .await
        .map_err(|_| AppServerError::Listener)?;
    let address = listener
        .local_addr()
        .map_err(|_| AppServerError::Listener)?;
    let (base_url, websocket_url, openai_url) = resolved_urls(&prepared, address);
    tracing::info!(
        base_url,
        websocket_url,
        openai_url = openai_url.as_deref().unwrap_or("disabled"),
        auth_mode = prepared.auth.mode_name(),
        "app server listener started"
    );
    let state = Arc::new(ListenerState {
        auth: prepared.auth,
        clock,
    });
    let router = build_router(&prepared.websocket_path, state);
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_receiver.await;
            })
            .await
            .map_err(|_| AppServerError::Listener)
    });
    Ok(ListenerHandle {
        address,
        base_url,
        websocket_url,
        openai_url,
        shutdown_sender: Some(shutdown_sender),
        task,
    })
}

fn build_router(path: &str, state: Arc<ListenerState>) -> Router {
    let router = Router::new()
        .route("/", get(upgrade))
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/readyz", get(|| async { "ok\n" }));
    let router = if path == "/" {
        router
    } else {
        router.route(path, get(upgrade))
    };
    router.with_state(state).fallback(not_found)
}

async fn upgrade(
    State(state): State<Arc<ListenerState>>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    if let Err(error) = state.auth.authorize(&headers, state.clock.as_ref()) {
        tracing::warn!(code = error.code(), "websocket authentication denied");
        return error.into_response();
    }
    if let Err(error) = origin::enforce(&headers, &state.auth) {
        tracing::warn!(code = error.code(), "websocket origin denied");
        return error.into_response();
    }
    websocket
        .max_frame_size(WS_FRAME_BYTES_MAX)
        .max_message_size(WS_FRAME_BYTES_MAX)
        .on_upgrade(drain_socket)
        .into_response()
}

async fn drain_socket(mut socket: WebSocket) {
    while socket.recv().await.is_some() {}
}

async fn not_found(_: Request<Body>) -> Response {
    axum::http::StatusCode::NOT_FOUND.into_response()
}

fn format_bind_address(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn resolved_urls(
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
