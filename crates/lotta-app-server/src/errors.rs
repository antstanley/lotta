//! Stable client envelopes and protected diagnostic records.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use lotta_domain::Secret;
use lotta_runtime::ports::IdGenerator;
use serde::Serialize;
use std::fmt;
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
    /// An authenticated principal lacks permission.
    #[error("forbidden")]
    Forbidden,
    /// An Origin-bearing request had no configured policy.
    #[error("origin-bearing websocket requests require authentication")]
    OriginAuthenticationRequired,
    /// Input syntax or upgrade headers are malformed.
    #[error("malformed request")]
    Malformed,
    /// Input exceeded a transport limit.
    #[error("payload too large")]
    PayloadTooLarge,
    /// Service is temporarily unavailable.
    #[error("service unavailable")]
    Unavailable,
    /// Internal processing failed without exposing detail.
    #[error("internal server error")]
    Internal,
    /// A primary operation failed and its mandatory cleanup also failed.
    #[error("{primary}; cleanup also failed: {cleanup}")]
    CleanupAttached {
        /// Primary operation failure preserved for classification.
        primary: Box<Self>,
        /// Cleanup failure attached without replacing the primary failure.
        cleanup: Box<Self>,
    },
    /// No route exists for the requested path.
    #[error("not found")]
    NotFound,
    /// The route does not support the requested method.
    #[error("method not allowed")]
    MethodNotAllowed,
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
            Self::Forbidden => "auth_forbidden",
            Self::OriginAuthenticationRequired => "auth_origin_required",
            Self::Malformed => "request_malformed",
            Self::PayloadTooLarge => "payload_too_large",
            Self::Unavailable => "service_unavailable",
            Self::Internal => "internal_error",
            Self::CleanupAttached { primary, .. } => primary.code(),
            Self::NotFound => "not_found",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::Task => "listener_task_failed",
        }
    }

    /// Returns the stable external HTTP status.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::OriginAuthenticationRequired | Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Config(_) | Self::SecretFile { .. } | Self::Malformed => StatusCode::BAD_REQUEST,
            Self::Listener | Self::Task | Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::CleanupAttached { primary, .. } => primary.status(),
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
        }
    }

    fn external_message(&self) -> &'static str {
        match self {
            Self::Config(_) | Self::SecretFile { .. } | Self::Malformed => "malformed request",
            Self::OriginAuthenticationRequired | Self::Unauthorized => "authentication failed",
            Self::Forbidden => "forbidden",
            Self::PayloadTooLarge => "payload too large",
            Self::Listener | Self::Task | Self::Unavailable => "service unavailable",
            Self::Internal => "internal server error",
            Self::CleanupAttached { primary, .. } => primary.external_message(),
            Self::NotFound => "not found",
            Self::MethodNotAllowed => "method not allowed",
        }
    }
}

/// Stable HTTP JSON error body.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct HttpErrorEnvelope {
    /// Stable machine-readable code.
    pub code: &'static str,
    /// Scrubbed client-readable message.
    pub error: &'static str,
}

impl IntoResponse for AppServerError {
    fn into_response(self) -> Response {
        let body = HttpErrorEnvelope {
            code: self.code(),
            error: self.external_message(),
        };
        (self.status(), Json(body)).into_response()
    }
}

/// Stable WebSocket protocol error body.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct ProtocolErrorEnvelope {
    /// Stable envelope discriminator.
    #[serde(rename = "type")]
    pub discriminator: &'static str,
    /// Stable machine-readable code.
    pub code: &'static str,
    /// Scrubbed client-readable message.
    pub message: &'static str,
    /// Valid request correlation, omitted when unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl ProtocolErrorEnvelope {
    /// Constructs a stable protocol error.
    #[must_use]
    pub fn new(code: &'static str, message: &'static str, request_id: Option<String>) -> Self {
        Self {
            discriminator: "protocol_error",
            code,
            message,
            request_id,
        }
    }
}

/// Protected internal-failure diagnostic retained outside client envelopes.
pub struct DiagnosticRecord {
    incident_id: String,
    code: &'static str,
    detail: Secret<String>,
}

impl DiagnosticRecord {
    /// Returns the generated identifier safe for diagnostic correlation.
    #[must_use]
    pub fn incident_id(&self) -> &str {
        &self.incident_id
    }

    /// Returns the stable diagnostic classification.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Gives a controlled sink operation temporary access to exact protected detail.
    pub fn with_detail<R>(&self, sink: impl for<'a> FnOnce(&'a str) -> R) -> R {
        self.detail.expose_secret(|detail| sink(detail))
    }
}

impl fmt::Debug for DiagnosticRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticRecord")
            .field("incident_id", &self.incident_id)
            .field("code", &self.code)
            .field("detail", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for DiagnosticRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} incident {}", self.code, self.incident_id)
    }
}

/// Constructs a scrubbed failure and its protected diagnostic record.
pub async fn internal_failure(
    ids: &dyn IdGenerator,
    detail: String,
) -> (AppServerError, Option<DiagnosticRecord>) {
    let Ok(incident_id) = ids.incident_id().await else {
        tracing::error!(code = "internal_error", "internal request failure");
        return (AppServerError::Internal, None);
    };
    let incident_id = incident_id.to_string();
    tracing::error!(
        incident_id,
        code = "internal_error",
        "internal request failure"
    );
    let record = DiagnosticRecord {
        incident_id,
        code: "internal_error",
        detail: Secret::new(detail),
    };
    (AppServerError::Internal, Some(record))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn response(error: AppServerError) -> (StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 4096)
            .await
            .expect("bounded body");
        (
            status,
            String::from_utf8(bytes.to_vec()).expect("JSON UTF-8"),
        )
    }

    macro_rules! response_test {
        ($name:ident, $error:expr, $status:ident, $code:literal, $message:literal) => {
            #[tokio::test]
            async fn $name() {
                let actual = response($error).await;
                assert_eq!(actual.0, StatusCode::$status);
                assert_eq!(
                    actual.1,
                    concat!("{\"code\":\"", $code, "\",\"error\":\"", $message, "\"}")
                );
            }
        };
    }

    response_test!(
        preupgrade_400_shape,
        AppServerError::Malformed,
        BAD_REQUEST,
        "request_malformed",
        "malformed request"
    );
    response_test!(
        preupgrade_401_shape,
        AppServerError::Unauthorized,
        UNAUTHORIZED,
        "auth_unauthorized",
        "authentication failed"
    );
    response_test!(
        preupgrade_403_shape,
        AppServerError::Forbidden,
        FORBIDDEN,
        "auth_forbidden",
        "forbidden"
    );
    response_test!(
        preupgrade_503_shape,
        AppServerError::Unavailable,
        SERVICE_UNAVAILABLE,
        "service_unavailable",
        "service unavailable"
    );
    response_test!(
        body_413_shape,
        AppServerError::PayloadTooLarge,
        PAYLOAD_TOO_LARGE,
        "payload_too_large",
        "payload too large"
    );
    response_test!(
        internal_500_shape,
        AppServerError::Internal,
        INTERNAL_SERVER_ERROR,
        "internal_error",
        "internal server error"
    );

    #[test]
    fn protocol_error_correlates_valid_id() {
        let value = ProtocolErrorEnvelope::new("invalid", "invalid command", Some("r1".into()));
        let expected = concat!(
            "{\"type\":\"protocol_error\",\"code\":\"invalid\",",
            "\"message\":\"invalid command\",\"request_id\":\"r1\"}",
        );
        assert_eq!(serde_json::to_string(&value).expect("serialize"), expected,);
    }

    #[test]
    fn protocol_error_omits_invalid_correlation() {
        let value = ProtocolErrorEnvelope::new("invalid", "invalid command", None);
        assert_eq!(
            serde_json::to_string(&value).expect("serialize"),
            r#"{"type":"protocol_error","code":"invalid","message":"invalid command"}"#
        );
    }

    #[tokio::test]
    async fn internal_incident_is_diagnostic_only() {
        let secret = "secret /private/file.rs stack frame";
        let ids = lotta_testkit::ids::DeterministicIdGenerator::new();
        let (error, record) = internal_failure(&ids, secret.into()).await;
        let (_, client) = response(error).await;
        let record = record.expect("generated incident");
        assert!(!client.contains(record.incident_id()));
        assert!(!client.contains("secret"));
        assert_eq!(record.code(), "internal_error");
        assert_eq!(record.with_detail(str::to_owned), secret);
    }

    #[tokio::test]
    async fn diagnostic_formatting_redacts_detail_and_ids_are_unique() {
        let ids = lotta_testkit::ids::DeterministicIdGenerator::new();
        let (_, first) = internal_failure(&ids, "protected".into()).await;
        let (_, second) = internal_failure(&ids, "protected".into()).await;
        let first = first.expect("first incident");
        let second = second.expect("second incident");
        assert_ne!(first.incident_id(), second.incident_id());
        assert!(!format!("{first:?}").contains("protected"));
        assert!(!format!("{first}").contains("protected"));
        assert_eq!(first.with_detail(str::to_owned), "protected");
    }
}
