//! Stable OpenAI-compatible HTTP error envelopes.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

const INVALID_REQUEST_ERROR: &str = "invalid_request_error";
const MODEL_NOT_FOUND: &str = "model_not_found";
const ERROR_MODEL_CHARS_MAX: usize = 1_024;

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    message: String,
    #[serde(rename = "type")]
    error_type: &'static str,
    param: Option<String>,
    code: &'static str,
}

/// Builds the pinned missing-model status and `OpenAI` error envelope.
#[must_use]
pub fn model_not_found(model: &str) -> Response {
    let displayed = model
        .chars()
        .take(ERROR_MODEL_CHARS_MAX)
        .collect::<String>();
    let message = format!(
        "The model '{displayed}' does not exist. Use GET /v1/models to list available agents."
    );
    let body = ErrorEnvelope {
        error: ErrorBody {
            message,
            error_type: INVALID_REQUEST_ERROR,
            param: None,
            code: MODEL_NOT_FOUND,
        },
    };
    (StatusCode::NOT_FOUND, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::Value;

    #[tokio::test]
    async fn missing_model_has_standard_openai_shape() {
        let response = super::model_not_found("absent");
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
        let bytes = to_bytes(response.into_body(), 4_096)
            .await
            .unwrap_or_else(|error| panic!("error body: {error}"));
        let body: Value =
            serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("error JSON: {error}"));
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "model_not_found");
        assert_eq!(body["error"]["param"], Value::Null);
        assert_eq!(
            body["error"]["message"],
            "The model 'absent' does not exist. Use GET /v1/models to list available agents."
        );
    }
}
