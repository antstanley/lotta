//! Supervised Responses outcome state and runtime event capture.

use std::sync::{Arc, Mutex};

use lotta_domain::RuntimeScope;
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::ws::{RuntimeEvent, RuntimeEventSink, event::StreamDelta};

use super::super::idempotency::Usage;
use super::output::{OPENAI_RESPONSES_OUTPUT_SIGNALS_MAX, OutputSignal, ResponseOutcome};

/// One request-owned outcome. Responses never indexes these cells by idempotency headers.
pub struct ResponseCell {
    value: AsyncMutex<Option<Arc<ResponseOutcome>>>,
    settled: Notify,
}

impl ResponseCell {
    /// Creates an active independent request cell.
    #[must_use]
    pub fn new() -> Self {
        Self {
            value: AsyncMutex::new(None),
            settled: Notify::new(),
        }
    }

    /// Waits for cleanup-before-settlement owner completion.
    pub async fn wait(&self) -> Arc<ResponseOutcome> {
        loop {
            let notified = self.settled.notified();
            if let Some(value) = self.value.lock().await.clone() {
                return value;
            }
            notified.await;
        }
    }

    /// Settles exactly once.
    pub async fn settle(&self, outcome: ResponseOutcome) {
        let mut value = self.value.lock().await;
        if value.is_none() {
            *value = Some(Arc::new(outcome));
            drop(value);
            self.settled.notify_waiters();
        }
    }
}

impl Default for ResponseCell {
    fn default() -> Self {
        Self::new()
    }
}

/// Runtime sink retaining canonical text, reasoning, redaction, tools, and usage.
pub struct ResponseTurnSink {
    signals: Mutex<Vec<OutputSignal>>,
    usage: Mutex<Usage>,
    error: Mutex<Option<String>>,
}

impl ResponseTurnSink {
    /// Creates an empty bounded projection sink.
    #[must_use]
    pub fn new() -> Self {
        Self {
            signals: Mutex::new(Vec::new()),
            usage: Mutex::new(Usage::default()),
            error: Mutex::new(None),
        }
    }

    /// Returns the scrubbed terminal result.
    #[must_use]
    pub fn outcome(&self, controller_failed: bool) -> ResponseOutcome {
        let signals = self
            .signals
            .lock()
            .map(|signals| signals.clone())
            .unwrap_or_default();
        let usage = self
            .usage
            .lock()
            .map(|usage| usage.clone())
            .unwrap_or_default();
        let mut error = self.error.lock().ok().and_then(|error| error.clone());
        if controller_failed && error.is_none() {
            error = Some("failed to run agent turn".to_owned());
        }
        ResponseOutcome {
            signals,
            usage,
            error,
            conversation_id: None,
        }
    }

    fn push(&self, signal: OutputSignal) {
        let Ok(mut signals) = self.signals.lock() else {
            return;
        };
        if signals.len() < OPENAI_RESPONSES_OUTPUT_SIGNALS_MAX {
            signals.push(signal);
        } else if !merge_last(&mut signals, signal)
            && let Ok(mut error) = self.error.lock()
        {
            *error = Some("failed to run agent turn".to_owned());
        }
    }

    fn other(&self, value: &serde_json::Value) {
        let kind = value.get("kind").and_then(serde_json::Value::as_str);
        if let Some(text) = value.get("text").and_then(serde_json::Value::as_str) {
            match kind {
                Some("text" | "assistant") => self.push(OutputSignal::Text(text.to_owned())),
                Some("reasoning") => self.push(OutputSignal::Reasoning(text.to_owned())),
                Some("redactedreasoning" | "redacted_reasoning") => {
                    self.push(OutputSignal::RedactedReasoning(text.to_owned()));
                }
                _ => {}
            }
        }
        if value
            .get("message_type")
            .and_then(serde_json::Value::as_str)
            == Some("usage_statistics")
            && let Ok(mut usage) = self.usage.lock()
        {
            usage.prompt_tokens = number(value, "prompt_tokens");
            usage.completion_tokens = number(value, "completion_tokens");
            usage.total_tokens = number(value, "total_tokens");
            usage.reasoning_tokens = value
                .get("reasoning_tokens")
                .and_then(serde_json::Value::as_u64);
        }
    }

    fn terminal(&self, stop_reason: &str, explicit_error: bool) {
        if (explicit_error
            || matches!(
                stop_reason,
                "cancelled"
                    | "user_cancellation"
                    | "empty_response"
                    | "context_overflow"
                    | "transport_failure"
                    | "provider_quota_error"
            ))
            && let Ok(mut error) = self.error.lock()
        {
            *error = Some("failed to run agent turn".to_owned());
        }
    }
}

impl Default for ResponseTurnSink {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeEventSink for ResponseTurnSink {
    fn emit(
        &self,
        _: &RuntimeScope,
        event: RuntimeEvent,
    ) -> Result<(), crate::error::AppServerError> {
        match event {
            RuntimeEvent::StreamDelta {
                delta: StreamDelta::Other(value),
                subagent_id: None,
            } => self.other(value.as_value()),
            RuntimeEvent::StreamDelta {
                delta: StreamDelta::ClientToolStart(value),
                subagent_id: None,
            } => self.push(OutputSignal::ToolStart {
                call_id: value.tool_call_id.as_str().to_owned(),
                name: value
                    .tool_name
                    .map_or_else(|| "tool".to_owned(), |name| name.as_str().to_owned()),
                arguments: value.tool_args.unwrap_or_default(),
            }),
            RuntimeEvent::StreamDelta {
                delta: StreamDelta::ClientToolEnd(value),
                subagent_id: None,
            } => self.push(OutputSignal::ToolEnd {
                call_id: value.tool_call_id.as_str().to_owned(),
                success: matches!(value.status, crate::ws::event::ClientToolStatus::Success),
            }),
            RuntimeEvent::TurnFinished {
                stop_reason, error, ..
            } => self.terminal(stop_reason.as_str(), error.is_some()),
            _ => {}
        }
        Ok(())
    }
}

fn merge_last(signals: &mut [OutputSignal], incoming: OutputSignal) -> bool {
    let Some(last) = signals.last_mut() else {
        return false;
    };
    match (last, incoming) {
        (OutputSignal::Text(current), OutputSignal::Text(next))
        | (OutputSignal::Reasoning(current), OutputSignal::Reasoning(next))
        | (OutputSignal::RedactedReasoning(current), OutputSignal::RedactedReasoning(next)) => {
            current.push_str(&next);
            true
        }
        _ => false,
    }
}

fn number(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}
