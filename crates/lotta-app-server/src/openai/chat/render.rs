use super::{fresh_uuid, server_failure};
use crate::openai::idempotency::{OutcomeCell, TurnOutcome};
use axum::{
    body::{Body, Bytes},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::stream;
use serde_json::{Value, json};
use std::{collections::VecDeque, convert::Infallible, sync::Arc};

pub(super) async fn render(
    cell: Arc<OutcomeCell>,
    owner: bool,
    model: &str,
    streaming: bool,
    created: i64,
) -> Response {
    let completion_id = format!("chatcmpl-{}", fresh_uuid());
    if streaming {
        return sse_response(cell, owner, completion_id, created, model.to_owned());
    }
    let outcome = cell.wait().await;
    if let Some(error) = &outcome.error {
        return server_failure(error);
    }
    json_completion(&completion_id, created, model, &outcome)
}

fn json_completion(id: &str, created: i64, model: &str, outcome: &TurnOutcome) -> Response {
    let usage = json!({
        "prompt_tokens":outcome.usage.prompt_tokens,
        "completion_tokens":outcome.usage.completion_tokens,
        "total_tokens":outcome.usage.total_tokens,
        "completion_tokens_details":{
            "reasoning_tokens":outcome.usage.reasoning_tokens.unwrap_or(0),
        },
    });
    axum::Json(json!({
        "id":id, "object":"chat.completion", "created":created, "model":model,
        "choices":[{"index":0, "message":{"role":"assistant", "content":outcome.text},
            "finish_reason":"stop"}],
        "usage":usage,
    }))
    .into_response()
}

fn sse_response(
    cell: Arc<OutcomeCell>,
    _owner: bool,
    id: String,
    created: i64,
    model: String,
) -> Response {
    let body_stream = stream::unfold(
        SseCursor::new(cell, id, created, model),
        |mut cursor| async move {
            cursor
                .next()
                .await
                .map(|bytes| (Ok::<Bytes, Infallible>(bytes), cursor))
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| server_failure("failed to create stream"))
}

struct SseCursor {
    cell: Arc<OutcomeCell>,
    deltas: tokio::sync::broadcast::Receiver<(u64, String)>,
    pending: VecDeque<Bytes>,
    id: String,
    created: i64,
    model: String,
    last_sequence: u64,
    sent: String,
    finished: bool,
}

impl SseCursor {
    fn new(cell: Arc<OutcomeCell>, id: String, created: i64, model: String) -> Self {
        let deltas = cell.subscribe();
        let replay = cell.replay();
        let mut cursor = Self {
            cell,
            deltas,
            pending: VecDeque::from([Bytes::from(chunk(
                &id,
                created,
                &model,
                &json!({"role":"assistant", "content":""}),
                &Value::Null,
            ))]),
            id,
            created,
            model,
            last_sequence: 0,
            sent: String::new(),
            finished: false,
        };
        cursor.enqueue_replay(replay);
        cursor
    }

    async fn next(&mut self) -> Option<Bytes> {
        loop {
            if let Some(bytes) = self.pending.pop_front() {
                return Some(bytes);
            }
            if self.finished {
                return None;
            }
            if !self.cell.is_active() {
                let outcome = self.cell.wait().await;
                self.enqueue_finish(&outcome);
                continue;
            }
            tokio::select! {
                outcome = self.cell.wait() => self.enqueue_finish(&outcome),
                delta = self.deltas.recv() => self.receive(delta).await,
            }
        }
    }

    async fn receive(
        &mut self,
        delta: Result<(u64, String), tokio::sync::broadcast::error::RecvError>,
    ) {
        match delta {
            Ok((sequence, piece)) => self.enqueue_delta(sequence, &piece),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                self.enqueue_replay(self.cell.replay());
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                let outcome = self.cell.wait().await;
                self.enqueue_finish(&outcome);
            }
        }
    }

    fn enqueue_delta(&mut self, sequence: u64, piece: &str) {
        if sequence <= self.last_sequence {
            return;
        }
        self.last_sequence = sequence;
        self.sent.push_str(piece);
        self.pending.push_back(Bytes::from(chunk(
            &self.id,
            self.created,
            &self.model,
            &json!({"content":piece}),
            &Value::Null,
        )));
    }

    fn enqueue_replay(&mut self, replay: Vec<(u64, String)>) {
        for (sequence, piece) in replay {
            self.enqueue_delta(sequence, &piece);
        }
    }

    fn enqueue_finish(&mut self, outcome: &TurnOutcome) {
        if self.finished {
            return;
        }
        self.enqueue_replay(self.cell.replay());
        if outcome.error.is_none() {
            if let Some(remaining) = outcome.text.strip_prefix(&self.sent)
                && !remaining.is_empty()
            {
                self.enqueue_delta(self.last_sequence.saturating_add(1), remaining);
            }
            self.pending.push_back(Bytes::from(terminal_chunk(
                &self.id,
                self.created,
                &self.model,
            )));
        } else if let Some(error) = &outcome.error {
            self.pending.push_back(Bytes::from(format!(
                "data: {}\n\n",
                json!({"error":{"message":error, "type":"server_error"}})
            )));
        }
        self.pending
            .push_back(Bytes::from_static(b"data: [DONE]\n\n"));
        self.finished = true;
    }
}

fn terminal_chunk(id: &str, created: i64, model: &str) -> String {
    format!(
        "data: {}\n\n",
        json!({
            "id":id, "object":"chat.completion.chunk", "created":created, "model":model,
            "choices":[{"index":0, "delta":{}, "finish_reason":"stop"}]
        })
    )
}

fn chunk(id: &str, created: i64, model: &str, delta: &Value, finish_reason: &Value) -> String {
    format!(
        "data: {}\n\n",
        json!({
            "id":id, "object":"chat.completion.chunk", "created":created, "model":model,
            "choices":[{"index":0, "delta":delta, "finish_reason":finish_reason}]
        })
    )
}
