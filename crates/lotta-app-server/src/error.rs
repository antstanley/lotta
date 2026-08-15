use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::json;
use thiserror::Error;

/// App-server boundary failures with scrubbed stable messages.
#[derive(Debug, Error)]
pub enum AppServerError {
    /// Command-line or startup policy is invalid.
    #[error("invalid server configuration: {0}")]
    Config(&'static str),
    /// A secret file could not be safely loaded.
    #[error("unable to read secret file: {path}")]
    SecretFile {
        /// Configured absolute path; file contents are never included.
        path: String,
    },
    /// Listener bind or serve failed.
    #[error("listener operation failed")]
    Listener,
    /// Authentication was absent or invalid.
    #[error("authentication failed")]
    Unauthorized,
    /// An Origin-bearing request had no configured policy.
    #[error("origin-bearing websocket requests require authentication")]
    OriginAuthenticationRequired,
    /// A spawned listener task could not be joined.
    #[error("listener task failed")]
    Task,
}

impl AppServerError {
    /// Returns the stable diagnostic code, never secret input.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Config(_) => "config_invalid",
            Self::SecretFile { .. } => "secret_file_unavailable",
            Self::Listener => "listener_failed",
            Self::Unauthorized => "auth_unauthorized",
            Self::OriginAuthenticationRequired => "auth_origin_required",
            Self::Task => "listener_task_failed",
        }
    }
}

impl IntoResponse for AppServerError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            Self::OriginAuthenticationRequired | Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Config(_) | Self::SecretFile { .. } => StatusCode::BAD_REQUEST,
            Self::Listener | Self::Task => StatusCode::SERVICE_UNAVAILABLE,
        };
        (
            status,
            Json(json!({ "error": self.to_string(), "code": self.code() })),
        )
            .into_response()
    }
}
