//! Authenticated bounded OpenAI Responses subset.
//!
//! This route shares Task 75 authentication, model resolution, persistent chat
//! allocation, canonical runtime ownership, and quiescent teardown. It keeps an
//! independent request cell for every call and never reads either idempotency header.

use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    extract::FromRequest,
    http::{HeaderMap, Request, StatusCode},
    response::Response,
};
use serde_json::{Value, json};

use crate::bounds::HTTP_BODY_BYTES_MAX;

use super::{chat::fresh_uuid, errors, resolve};

mod execution;
mod input;
mod output;
mod render;
mod state;

pub use execution::ResponsesState;

/// Route-local body extractor preserving the canonical 20 MiB cap.
pub struct ResponsesJson(Value);

impl<S> FromRequest<S> for ResponsesJson
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(request: Request<Body>, _: &S) -> Result<Self, Self::Rejection> {
        let bytes = to_bytes(request.into_body(), HTTP_BODY_BYTES_MAX)
            .await
            .map_err(|_| {
                errors::response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    errors::invalid_request("request body too large"),
                )
            })?;
        let value = if bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&bytes).map_err(|_| {
                errors::response(
                    StatusCode::BAD_REQUEST,
                    errors::invalid_request("invalid JSON body"),
                )
            })?
        };
        Ok(Self(value))
    }
}

/// Executes one independently owned Responses request.
pub async fn respond(
    state: Arc<ResponsesState>,
    headers: HeaderMap,
    ResponsesJson(value): ResponsesJson,
) -> Response {
    let request = match input::prepare(value, &headers) {
        Ok(request) => request,
        Err(error) => return invalid(error.0),
    };
    let Ok(agents) = resolve::visible_agents(&state.chat.agents).await else {
        return server_failure();
    };
    let agent = match resolve::resolve(&agents, &request.model) {
        Ok(agent) => agent.clone(),
        Err(error) => return errors::response(StatusCode::NOT_FOUND, error),
    };
    let previous = match execution::resolve_previous(&state, &agent, &request).await {
        Ok(previous) => previous,
        Err(error) => return previous_error(&request, error),
    };
    let meta = output::ResponseMeta {
        agent_id: agent.id.clone(),
        id: format!("resp_{}", fresh_uuid()),
        created_at: state.chat.clock.now().as_utc().timestamp(),
        model: request.model.clone(),
        instructions: request.instructions.clone(),
        previous_response_id: request.previous_response_id.clone(),
        store: request.store,
    };
    let streaming = request.streaming;
    let cell = Arc::new(state::ResponseCell::new());
    if execution::spawn_owner(state, agent, request, previous, Arc::clone(&cell))
        .await
        .is_err()
    {
        return errors::response(
            StatusCode::SERVICE_UNAVAILABLE,
            errors::server_error("Responses execution capacity unavailable"),
        );
    }
    render::render(cell, meta, streaming).await
}

fn previous_error(request: &input::PreparedRequest, error: execution::PreviousError) -> Response {
    let response_id = request.previous_response_id.as_deref().unwrap_or_default();
    match error {
        execution::PreviousError::NotFound => errors::response(
            StatusCode::NOT_FOUND,
            errors::response_not_found(response_id, None),
        ),
        execution::PreviousError::WrongModel => errors::response(
            StatusCode::NOT_FOUND,
            errors::response_not_found(response_id, Some(&request.model)),
        ),
        execution::PreviousError::Unsupported => {
            errors::response(StatusCode::NOT_IMPLEMENTED, errors::unsupported_backend())
        }
        execution::PreviousError::Failed => server_failure(),
    }
}

fn invalid(message: impl Into<String>) -> Response {
    errors::response(StatusCode::BAD_REQUEST, errors::invalid_request(message))
}

fn server_failure() -> Response {
    errors::response(
        StatusCode::INTERNAL_SERVER_ERROR,
        errors::server_error("internal server error"),
    )
}

#[cfg(test)]
include!("responses_tests.rs");

#[cfg(test)]
include!("tests/responses_transport.rs");
