use std::{collections::VecDeque, convert::Infallible, sync::Arc};

use axum::{
    Json,
    body::{Body, Bytes},
    http::{StatusCode, header},
    response::{IntoResponse as _, Response},
};
use futures_util::stream;
use serde_json::{Value, json};

use super::super::{cursor, idempotency::Usage};
use super::{
    output::{
        OutputBuilder, ResponseMeta, ResponseOutcome, response_value, response_value_for_output,
    },
    state::{ResponseCell, ResponseEvent},
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
            let error = json!({
                "error": {
                    "message": "internal server error",
                    "type": "server_error",
                    "param": null,
                    "code": null
                }
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        })
}

struct SseCursor {
    cell: Arc<ResponseCell>,
    meta: Option<ResponseMeta>,
    pending: VecDeque<Bytes>,
    output: OutputBuilder,
    event_index: usize,
    sequence: u64,
    finished: bool,
}

impl SseCursor {
    fn new(cell: Arc<ResponseCell>, meta: ResponseMeta) -> Self {
        Self {
            cell,
            meta: Some(meta),
            pending: VecDeque::new(),
            output: OutputBuilder::default(),
            event_index: 0,
            sequence: 0,
            finished: false,
        }
    }

    async fn next(&mut self) -> Option<Bytes> {
        loop {
            if let Some(bytes) = self.pending.pop_front() {
                return Some(bytes);
            }
            if self.finished {
                return None;
            }
            let Some(event) = self.cell.event(self.event_index).await else {
                self.finished = true;
                return None;
            };
            self.event_index = self.event_index.saturating_add(1);
            self.apply(event);
        }
    }

    fn apply(&mut self, event: ResponseEvent) {
        match event {
            ResponseEvent::Started(conversation) => {
                let Some(meta) = self.meta.take() else {
                    return;
                };
                let progress_outcome = ResponseOutcome {
                    signals: Vec::new(),
                    usage: Usage::default(),
                    error: None,
                    conversation_id: conversation,
                };
                let mut progress = response_value_for_output(&meta, &progress_outcome, &[]);
                progress["status"] = json!("in_progress");
                progress["output"] = json!([]);
                progress["error"] = Value::Null;
                progress["usage"] = Value::Null;
                self.push(json!({"type":"response.created","response":progress}));
                self.push(json!({"type":"response.in_progress","response":progress}));
                self.meta = Some(meta);
            }
            ResponseEvent::Signal(signal) => {
                let mut events = Vec::new();
                self.output.apply(&signal, Some(&mut events));
                for event in events {
                    self.push(event);
                }
            }
            ResponseEvent::Settled(outcome) => {
                let mut events = Vec::new();
                self.output.finish(Some(&mut events));
                for event in events {
                    self.push(event);
                }
                let Some(meta) = self.meta.take() else {
                    self.finished = true;
                    return;
                };
                let meta = settled_meta(meta, &outcome);
                let terminal = response_value_for_output(&meta, &outcome, &self.output.output);
                let kind = if outcome.error.is_some() {
                    "response.failed"
                } else {
                    "response.completed"
                };
                self.push(json!({"type":kind,"response":terminal}));
                self.finished = true;
            }
        }
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
