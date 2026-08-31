//! Stable OpenAI-compatible error envelopes.

use serde::Serialize;

const INVALID_REQUEST_ERROR: &str = "invalid_request_error";
const MODEL_NOT_FOUND: &str = "model_not_found";

/// Stable `OpenAI` error envelope shared by model-taking handlers.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
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
            code: MODEL_NOT_FOUND,
        },
    }
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
