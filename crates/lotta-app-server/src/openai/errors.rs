//! Stable OpenAI-compatible error envelopes.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

const INVALID_REQUEST_ERROR: &str = "invalid_request_error";
const MODEL_NOT_FOUND: &str = "model_not_found";

/// Stable `OpenAI` error envelope shared by model-taking handlers.
#[derive(Clone, Debug, Serialize)]
pub struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Clone, Debug, Serialize)]
struct ErrorBody {
    message: String,
    #[serde(rename = "type")]
    error_type: &'static str,
    param: Option<String>,
    code: Option<&'static str>,
}

/// Builds the pinned pure missing-model contract without altering the model value.
#[must_use]
pub fn model_not_found(model: &str) -> ErrorEnvelope {
    let message =
        format!("The model '{model}' does not exist. Use GET /v1/models to list available agents.");
    ErrorEnvelope {
        error: ErrorBody {
            message,
            error_type: INVALID_REQUEST_ERROR,
            param: None,
            code: Some(MODEL_NOT_FOUND),
        },
    }
}

/// Builds the pinned invalid-request envelope.
#[must_use]
pub fn invalid_request(message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope {
        error: ErrorBody {
            message: message.into(),
            error_type: INVALID_REQUEST_ERROR,
            param: None,
            code: None,
        },
    }
}

/// Builds a scrubbed server-error envelope.
#[must_use]
pub fn server_error(message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope {
        error: ErrorBody {
            message: message.into(),
            error_type: "server_error",
            param: None,
            code: None,
        },
    }
}

/// Converts an `OpenAI` envelope to exact JSON content type and status.
#[must_use]
pub fn response(status: StatusCode, error: ErrorEnvelope) -> Response {
    (status, Json(error)).into_response()
}

#[cfg(test)]
mod tests {
    fn encoded(model: &str) -> String {
        serde_json::to_string(&super::model_not_found(model))
            .unwrap_or_else(|error| panic!("error JSON: {error}"))
    }

    #[test]
    fn missing_model_has_exact_openai_shape() {
        assert_eq!(
            encoded("absent"),
            concat!(
                r#"{"error":{"message":"The model 'absent' does not exist. "#,
                r#"Use GET /v1/models to list available agents.","type":"invalid_request_error","#,
                r#""param":null,"code":"model_not_found"}}"#
            )
        );
    }

    #[test]
    fn missing_model_preserves_unicode_exactly() {
        assert_eq!(
            encoded("模型/🦄/é"),
            concat!(
                r#"{"error":{"message":"The model '模型/🦄/é' does not exist. "#,
                r#"Use GET /v1/models to list available agents.","type":"invalid_request_error","#,
                r#""param":null,"code":"model_not_found"}}"#
            )
        );
    }

    #[test]
    fn missing_model_preserves_long_value_exactly() {
        let model = "模型🦄".repeat(2_049);
        let value = serde_json::to_value(super::model_not_found(&model))
            .unwrap_or_else(|error| panic!("error value: {error}"));
        assert_eq!(
            value["error"]["message"],
            format!(
                "The model '{model}' does not exist. Use GET /v1/models to list available agents."
            )
        );
    }
}
