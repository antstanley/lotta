use std::sync::{Arc, Mutex};

use lotta_domain::{ConversationId, RuntimeScope};
use tokio::sync::Notify;

use crate::ws::{RuntimeEvent, RuntimeEventSink, ToolExecutionResult, event::StreamDelta};

use super::super::idempotency::Usage;
use super::output::{OPENAI_RESPONSES_OUTPUT_SIGNALS_MAX, OutputSignal, ResponseOutcome};

/// Maximum request-owned replay records, including start and terminal records.
pub const OPENAI_RESPONSES_EVENT_LOG_MAX: usize = OPENAI_RESPONSES_OUTPUT_SIGNALS_MAX + 2;

/// One ordered internal record consumed by live and late-joining renderers.
#[derive(Clone)]
pub enum ResponseEvent {
    /// Allocation completed and admission may now begin.
    Started(Option<ConversationId>),
    /// One canonical runtime projection.
    Signal(OutputSignal),
    /// Cleanup-complete terminal outcome.
    Settled(Arc<ResponseOutcome>),
}

#[derive(Default)]
struct CellState {
    events: Vec<ResponseEvent>,
    outcome: Option<Arc<ResponseOutcome>>,
    started: bool,
}

/// One request-owned bounded ordered event log with gap-free replay and live notification.
pub struct ResponseCell {
    state: Mutex<CellState>,
    changed: Notify,
}

impl ResponseCell {
    /// Creates an active independent request cell.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(CellState::default()),
            changed: Notify::new(),
        }
    }

    /// Publishes allocation completion exactly once, before admission.
    pub fn start(&self, conversation: Option<ConversationId>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.started || state.outcome.is_some() {
            return;
        }
        state.started = true;
        state.events.push(ResponseEvent::Started(conversation));
        drop(state);
        self.changed.notify_waiters();
    }

    /// Appends one ordered bounded runtime signal for live delivery and replay.
    pub fn publish(&self, signal: OutputSignal) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if !state.started
            || state.outcome.is_some()
            || state.events.len() >= OPENAI_RESPONSES_EVENT_LOG_MAX - 1
        {
            return;
        }
        state.events.push(ResponseEvent::Signal(signal));
        drop(state);
        self.changed.notify_waiters();
    }

    /// Reads the next record without gaps; waits if the owner is still active.
    pub async fn event(&self, index: usize) -> Option<ResponseEvent> {
        loop {
            let notified = self.changed.notified();
            {
                let Ok(state) = self.state.lock() else {
                    return None;
                };
                if let Some(event) = state.events.get(index) {
                    return Some(event.clone());
                }
                if state.outcome.is_some() {
                    return None;
                }
            }
            notified.await;
        }
    }

    /// Waits for cleanup-before-settlement owner completion.
    pub async fn wait(&self) -> Arc<ResponseOutcome> {
        loop {
            let notified = self.changed.notified();
            if let Ok(state) = self.state.lock()
                && let Some(value) = &state.outcome
            {
                return Arc::clone(value);
            }
            notified.await;
        }
    }

    /// Settles exactly once and publishes exactly one terminal replay record.
    pub fn settle(&self, outcome: ResponseOutcome) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.outcome.is_some() {
            return;
        }
        if !state.started {
            state.started = true;
            state.events.push(ResponseEvent::Started(None));
        }
        let outcome = Arc::new(outcome);
        state.outcome = Some(Arc::clone(&outcome));
        if state.events.len() == OPENAI_RESPONSES_EVENT_LOG_MAX {
            state.events.pop();
        }
        state.events.push(ResponseEvent::Settled(outcome));
        drop(state);
        self.changed.notify_waiters();
    }
}

impl Default for ResponseCell {
    fn default() -> Self {
        Self::new()
    }
}

/// Runtime sink retaining canonical text, reasoning, redaction, tools, and usage.
pub struct ResponseTurnSink {
    cell: Arc<ResponseCell>,
    signals: Mutex<Vec<OutputSignal>>,
    usage: Mutex<Usage>,
    error: Mutex<Option<String>>,
}

impl ResponseTurnSink {
    /// Creates an empty bounded projection sink publishing into `cell`.
    #[must_use]
    pub fn new(cell: Arc<ResponseCell>) -> Self {
        Self {
            cell,
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
            signals.push(signal.clone());
            drop(signals);
            self.cell.publish(signal);
        } else if let Ok(mut error) = self.error.lock() {
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
            RuntimeEvent::TurnFinished {
                stop_reason, error, ..
            } => self.terminal(stop_reason.as_str(), error.is_some()),
            // A production tool result is delivered by `emit_tool_result`; accepting a bare
            // public lifecycle end here would fabricate output, so unmatched events are omitted.
            _ => {}
        }
        Ok(())
    }

    fn emit_tool_result(
        &self,
        _: &RuntimeScope,
        result: ToolExecutionResult,
    ) -> Result<(), crate::error::AppServerError> {
        if result.output.len() > lotta_runtime::bounds::TOOL_RESULT_BYTES_MAX.value
            || result.output.chars().count()
                > lotta_runtime::bounds::TOOL_RESULT_MODEL_CHARS_MAX.value
        {
            if let Ok(mut error) = self.error.lock() {
                *error = Some("failed to run agent turn".to_owned());
            }
            return Ok(());
        }
        self.push(OutputSignal::ToolEnd {
            call_id: result.tool_call_id,
            success: result.success,
            output: result.output,
        });
        Ok(())
    }
}

fn number(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}
