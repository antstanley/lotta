//! Responses output items, terminal JSON, and native event projection.

use lotta_domain::{AgentId, ConversationId};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::super::{chat::fresh_uuid, idempotency::Usage};

/// Maximum retained runtime projection signals per response.
pub const OPENAI_RESPONSES_OUTPUT_SIGNALS_MAX: usize = 4_096;

/// One canonical runtime projection consumed by JSON and SSE renderers.
#[derive(Clone, Debug)]
pub enum OutputSignal {
    /// Assistant output text delta.
    Text(String),
    /// Assistant reasoning summary delta.
    Reasoning(String),
    /// Redacted reasoning retained as a reasoning summary.
    RedactedReasoning(String),
    /// Server-side tool invocation started.
    ToolStart {
        /// Provider call ID.
        call_id: String,
        /// Model-facing tool name.
        name: String,
        /// Complete available arguments.
        arguments: String,
    },
    /// Server-side tool invocation finished.
    ToolEnd {
        /// Provider call ID.
        call_id: String,
        /// Whether execution succeeded.
        success: bool,
    },
}

/// Settled owner result before wire identity selection.
#[derive(Clone, Debug)]
pub struct ResponseOutcome {
    /// Ordered runtime projections.
    pub signals: Vec<OutputSignal>,
    /// Runtime token accounting.
    pub usage: Usage,
    /// Scrubbed terminal error.
    pub error: Option<String>,
    /// Allocated conversation available only after successful lifecycle handling.
    pub conversation_id: Option<ConversationId>,
}

impl ResponseOutcome {
    /// Creates the canonical scrubbed failure.
    #[must_use]
    pub fn failed() -> Self {
        Self {
            signals: Vec::new(),
            usage: Usage::default(),
            error: Some("failed to run agent turn".to_owned()),
            conversation_id: None,
        }
    }
}

/// Immutable response identity and echoed request subset.
#[derive(Clone)]
pub struct ResponseMeta {
    /// Canonical cursor owner, omitted from the wire object.
    pub agent_id: AgentId,
    /// Chosen wire response ID.
    pub id: String,
    /// Creation epoch seconds.
    pub created_at: i64,
    /// Advertised model.
    pub model: String,
    /// Echoed explicit instructions.
    pub instructions: Option<String>,
    /// Echoed previous response ID.
    pub previous_response_id: Option<String>,
    /// Whether this successful response is stored.
    pub store: bool,
}

#[derive(Serialize)]
struct ResponseError {
    code: &'static str,
    message: &'static str,
}

#[derive(Serialize)]
struct ResponseUsage {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    output_tokens_details: ReasoningUsage,
}

#[derive(Serialize)]
struct ReasoningUsage {
    reasoning_tokens: u64,
}

#[derive(Serialize)]
struct ResponseWire<'a> {
    id: &'a str,
    object: &'static str,
    created_at: i64,
    status: &'static str,
    output: &'a [Value],
    error: Option<ResponseError>,
    incomplete_details: Option<Value>,
    instructions: Option<&'a str>,
    model: &'a str,
    parallel_tool_calls: bool,
    tools: [Value; 0],
    tool_choice: &'static str,
    truncation: &'static str,
    usage: Option<ResponseUsage>,
    metadata: Map<String, Value>,
    store: bool,
    temperature: f64,
    top_p: f64,
    background: bool,
    max_output_text: Option<u64>,
    previous_response_id: Option<&'a str>,
}

/// Builds a pinned response object from settled output.
#[must_use]
pub fn response_value(meta: &ResponseMeta, outcome: &ResponseOutcome) -> Value {
    let mut builder = OutputBuilder::default();
    for signal in &outcome.signals {
        builder.apply(signal, None);
    }
    builder.finish(None);
    response_value_for_output(meta, outcome, &builder.output)
}

/// Builds a response reusing the exact output item identities emitted over SSE.
#[must_use]
pub fn response_value_for_output(
    meta: &ResponseMeta,
    outcome: &ResponseOutcome,
    output: &[Value],
) -> Value {
    let failed = outcome.error.is_some();
    serde_json::to_value(ResponseWire {
        id: &meta.id,
        object: "response",
        created_at: meta.created_at,
        status: if failed { "failed" } else { "completed" },
        output,
        error: failed.then_some(ResponseError {
            code: "server_error",
            message: "failed to run agent turn",
        }),
        incomplete_details: None,
        instructions: meta.instructions.as_deref(),
        model: &meta.model,
        parallel_tool_calls: true,
        tools: [],
        tool_choice: "auto",
        truncation: "disabled",
        usage: Some(usage(&outcome.usage)),
        metadata: Map::new(),
        store: meta.store && !failed,
        temperature: 1.0,
        top_p: 1.0,
        background: false,
        max_output_text: None,
        previous_response_id: meta.previous_response_id.as_deref(),
    })
    .unwrap_or_else(|_| json!({"status":"failed"}))
}

/// Projects ordered runtime signals into exact Responses SSE event payloads.
#[must_use]
pub fn signal_events(signals: &[OutputSignal]) -> (Vec<Value>, Vec<Value>) {
    let mut builder = OutputBuilder::default();
    let mut events = Vec::new();
    for signal in signals {
        builder.apply(signal, Some(&mut events));
    }
    builder.finish(Some(&mut events));
    (events, builder.output)
}

fn usage(value: &Usage) -> ResponseUsage {
    ResponseUsage {
        input_tokens: value.prompt_tokens,
        output_tokens: value.completion_tokens,
        total_tokens: value.total_tokens,
        output_tokens_details: ReasoningUsage {
            reasoning_tokens: value.reasoning_tokens.unwrap_or(0),
        },
    }
}

#[derive(Default)]
struct OutputBuilder {
    output: Vec<Value>,
    active: Option<ActiveItem>,
}

struct ActiveItem {
    kind: ActiveKind,
    index: usize,
    id: String,
    text: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ActiveKind {
    Text,
    Reasoning,
}

impl OutputBuilder {
    fn apply(&mut self, signal: &OutputSignal, events: Option<&mut Vec<Value>>) {
        match signal {
            OutputSignal::Text(text) => self.add_text(ActiveKind::Text, text, events),
            OutputSignal::Reasoning(text) | OutputSignal::RedactedReasoning(text) => {
                self.add_text(ActiveKind::Reasoning, text, events);
            }
            OutputSignal::ToolStart {
                call_id,
                name,
                arguments,
            } => self.add_tool(call_id, name, arguments, events),
            OutputSignal::ToolEnd { call_id, success } => {
                self.finish_tool(call_id, *success, events);
            }
        }
    }

    fn add_text(&mut self, kind: ActiveKind, delta: &str, mut events: Option<&mut Vec<Value>>) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.kind != kind)
        {
            self.finish(events.as_deref_mut());
        }
        if self.active.is_none() {
            self.start_text(kind, events.as_deref_mut());
        }
        let Some(active) = self.active.as_mut() else {
            return;
        };
        active.text.push_str(delta);
        push_event(
            events,
            match kind {
                ActiveKind::Text => json!({"type":"response.output_text.delta",
                    "output_index":active.index,"content_index":0,
                    "item_id":active.id,"delta":delta}),
                ActiveKind::Reasoning => json!({
                    "type":"response.reasoning_summary_text.delta",
                    "output_index":active.index,"summary_index":0,
                    "item_id":active.id,"delta":delta}),
            },
        );
    }

    fn start_text(&mut self, kind: ActiveKind, mut events: Option<&mut Vec<Value>>) {
        let id = match kind {
            ActiveKind::Text => format!("msg_{}", fresh_uuid()),
            ActiveKind::Reasoning => format!("rs_{}", fresh_uuid()),
        };
        let index = self.output.len();
        let item = match kind {
            ActiveKind::Text => json!({"type":"message","id":id,"status":"in_progress",
                "role":"assistant","content":[]}),
            ActiveKind::Reasoning => json!({"type":"reasoning","id":id,
                "status":"in_progress","summary":[]}),
        };
        push_event(
            events.as_deref_mut(),
            json!({"type":"response.output_item.added","output_index":index,"item":item}),
        );
        let part = match kind {
            ActiveKind::Text => json!({"type":"output_text","text":"","annotations":[]}),
            ActiveKind::Reasoning => json!({"type":"summary_text","text":""}),
        };
        let event = match kind {
            ActiveKind::Text => json!({"type":"response.content_part.added","output_index":index,
                "content_index":0,"item_id":id,"part":part}),
            ActiveKind::Reasoning => json!({"type":"response.reasoning_summary_part.added",
                "output_index":index,"summary_index":0,"item_id":id,"part":part}),
        };
        push_event(events, event);
        self.active = Some(ActiveItem {
            kind,
            index,
            id,
            text: String::new(),
        });
    }

    fn finish(&mut self, mut events: Option<&mut Vec<Value>>) {
        let Some(active) = self.active.take() else {
            return;
        };
        let (item, done, part_done) = finished_text_values(&active);
        push_event(events.as_deref_mut(), done);
        push_event(events.as_deref_mut(), part_done);
        push_event(
            events,
            json!({"type":"response.output_item.done","output_index":active.index,"item":item}),
        );
        self.output.push(item);
    }

    fn add_tool(
        &mut self,
        call_id: &str,
        name: &str,
        arguments: &str,
        mut events: Option<&mut Vec<Value>>,
    ) {
        self.finish(events.as_deref_mut());
        let item = json!({"type":"function_call","id":format!("fc_{}",fresh_uuid()),
            "call_id":call_id,"name":name,"arguments":arguments,"status":"in_progress"});
        let index = self.output.len();
        push_event(
            events.as_deref_mut(),
            json!({"type":"response.output_item.added",
            "output_index":index,"item":item}),
        );
        if !arguments.is_empty() {
            push_event(
                events,
                json!({"type":"response.function_call_arguments.delta",
                "output_index":index,"item_id":item["id"],"delta":arguments}),
            );
        }
        self.output.push(item);
    }

    fn finish_tool(&mut self, call_id: &str, success: bool, mut events: Option<&mut Vec<Value>>) {
        let Some(index) = self
            .output
            .iter()
            .position(|item| item["call_id"] == call_id)
        else {
            return;
        };
        self.output[index]["status"] = json!(if success { "completed" } else { "incomplete" });
        let item = self.output[index].clone();
        push_event(
            events.as_deref_mut(),
            json!({"type":"response.function_call_arguments.done",
            "output_index":index,"item_id":item["id"],"name":item["name"],
            "arguments":item["arguments"]}),
        );
        push_event(
            events.as_deref_mut(),
            json!({"type":"response.output_item.done",
            "output_index":index,"item":item}),
        );
        let result = json!({"type":"function_call_output","id":format!("fco_{}",fresh_uuid()),
            "call_id":call_id,"output":[{"type":"input_text","text":""}],
            "status":if success {"completed"} else {"incomplete"}});
        let result_index = self.output.len();
        self.output.push(result.clone());
        push_event(
            events.as_deref_mut(),
            json!({"type":"response.output_item.added",
            "output_index":result_index,"item":result}),
        );
        push_event(
            events,
            json!({"type":"response.output_item.done",
            "output_index":result_index,"item":result}),
        );
    }
}

fn finished_text_values(active: &ActiveItem) -> (Value, Value, Value) {
    match active.kind {
        ActiveKind::Text => {
            let part = json!({"type":"output_text","text":active.text,"annotations":[]});
            let item = json!({"type":"message","id":active.id,"status":"completed",
                "role":"assistant","content":[part]});
            let done = json!({"type":"response.output_text.done","output_index":active.index,
                "content_index":0,"item_id":active.id,"text":active.text});
            let part_done = json!({"type":"response.content_part.done","output_index":active.index,
                "content_index":0,"item_id":active.id,"part":part});
            (item, done, part_done)
        }
        ActiveKind::Reasoning => {
            let part = json!({"type":"summary_text","text":active.text});
            let item = json!({"type":"reasoning","id":active.id,"status":"completed",
                "summary":[part]});
            let done = json!({"type":"response.reasoning_summary_text.done",
                "output_index":active.index,"summary_index":0,
                "item_id":active.id,"text":active.text});
            let part_done = json!({"type":"response.reasoning_summary_part.done",
                "output_index":active.index,"summary_index":0,"item_id":active.id,"part":part});
            (item, done, part_done)
        }
    }
}

fn push_event(events: Option<&mut Vec<Value>>, event: Value) {
    if let Some(events) = events {
        events.push(event);
    }
}
