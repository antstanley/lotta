//! Pull-driven JSON and SSE Responses rendering.

use std::{collections::VecDeque, convert::Infallible, sync::Arc};

use axum::{
    Json,
    body::{Body, Bytes},
    http::{StatusCode, header},
    response::{IntoResponse as _, Response},
};
use futures_util::stream;
use serde_json::{Value, json};

use super::super::{chat::fresh_uuid, cursor};
use super::{
    output::{
        ResponseMeta, ResponseOutcome, response_value, response_value_for_output, signal_events,
    },
    state::ResponseCell,
};

/// Renders one request cell as exact JSON or a pull-driven SSE body.
pub async fn render(cell: Arc<ResponseCell>, meta: ResponseMeta, streaming: bool) -> Response {
    if streaming {
        return sse_response(cell, meta);
    }
    let outcome = cell.wait().await;
    let meta = settled_meta(meta, &outcome);
    let value = response_value(&meta, &outcome);
    let status = if outcome.error.is_some() {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::OK
    };
    (status, Json(value)).into_response()
}

fn settled_meta(mut meta: ResponseMeta, outcome: &ResponseOutcome) -> ResponseMeta {
    if outcome.error.is_none()
        && meta.store
        && let Some(conversation) = &outcome.conversation_id
        && let Ok(id) = cursor::encode(&meta.agent_id, conversation)
    {
        meta.id = id;
        return meta;
    }
    meta.id = format!("resp_{}", fresh_uuid());
    meta.store = false;
    meta
}

fn sse_response(cell: Arc<ResponseCell>, meta: ResponseMeta) -> Response {
    let body = stream::unfold(SseCursor::new(cell, meta), |mut cursor| async move {
        cursor
            .next()
            .await
            .map(|bytes| (Ok::<Bytes, Infallible>(bytes), cursor))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(body))
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"message":"internal server error",
                    "type":"server_error","param":null,"code":null}})),
            )
                .into_response()
        })
}

struct SseCursor {
    cell: Arc<ResponseCell>,
    meta: Option<ResponseMeta>,
    pending: VecDeque<Bytes>,
    sequence: u64,
    finished: bool,
}

impl SseCursor {
    fn new(cell: Arc<ResponseCell>, meta: ResponseMeta) -> Self {
        Self {
            cell,
            meta: Some(meta),
            pending: VecDeque::new(),
            sequence: 0,
            finished: false,
        }
    }

    async fn next(&mut self) -> Option<Bytes> {
        if let Some(bytes) = self.pending.pop_front() {
            return Some(bytes);
        }
        if self.finished {
            return None;
        }
        let outcome = self.cell.wait().await;
        let Some(meta) = self.meta.take() else {
            self.finished = true;
            return None;
        };
        self.enqueue(meta, &outcome);
        self.finished = true;
        self.pending.pop_front()
    }

    fn enqueue(&mut self, meta: ResponseMeta, outcome: &ResponseOutcome) {
        let meta = settled_meta(meta, outcome);
        let (events, output) = signal_events(&outcome.signals);
        let terminal = response_value_for_output(&meta, outcome, &output);
        let mut progress = terminal.clone();
        progress["status"] = json!("in_progress");
        progress["output"] = json!([]);
        progress["error"] = Value::Null;
        progress["usage"] = Value::Null;
        self.push(json!({"type":"response.created","response":progress}));
        self.push(json!({"type":"response.in_progress","response":progress}));
        for event in events {
            self.push(event);
        }
        let kind = if outcome.error.is_some() {
            "response.failed"
        } else {
            "response.completed"
        };
        self.push(json!({"type":kind,"response":terminal}));
    }

    fn push(&mut self, event: Value) {
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("response.failed")
            .to_owned();
        let mut payload = event;
        payload["sequence_number"] = json!(self.sequence);
        self.sequence = self.sequence.saturating_add(1);
        self.pending
            .push_back(Bytes::from(format!("event: {kind}\ndata: {payload}\n\n")));
    }
}
