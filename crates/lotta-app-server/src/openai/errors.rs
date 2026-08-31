//! Stable OpenAI-compatible error envelopes.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

const INVALID_REQUEST_ERROR: &str = "invalid_request_error";
const MODEL_NOT_FOUND: &str = "model_not_found";
const RESPONSE_NOT_FOUND: &str = "response_not_found";
const UNSUPPORTED_BACKEND: &str = "unsupported_backend";

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

/// Builds the pinned authentication-error envelope.
#[must_use]
pub fn authentication_error() -> ErrorEnvelope {
    ErrorEnvelope {
        error: ErrorBody {
            message: "authentication failed".to_owned(),
            error_type: "authentication_error",
            param: None,
            code: None,
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

/// Builds the pinned missing stored-Response envelope.
#[must_use]
pub fn response_not_found(response_id: &str, model: Option<&str>) -> ErrorEnvelope {
    let message = model.map_or_else(
        || format!("The response '{response_id}' does not exist or is no longer stored."),
        |model| format!("The response '{response_id}' does not exist for model '{model}'."),
    );
    ErrorEnvelope {
        error: ErrorBody {
            message,
            error_type: INVALID_REQUEST_ERROR,
            param: None,
            code: Some(RESPONSE_NOT_FOUND),
        },
    }
}

/// Builds the pinned unavailable-fork backend envelope.
#[must_use]
pub fn unsupported_backend() -> ErrorEnvelope {
    ErrorEnvelope {
        error: ErrorBody {
            message: concat!(
                "previous_response_id is unavailable because this backend ",
                "cannot fork conversations"
            )
            .to_owned(),
            error_type: "server_error",
            param: None,
            code: Some(UNSUPPORTED_BACKEND),
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
