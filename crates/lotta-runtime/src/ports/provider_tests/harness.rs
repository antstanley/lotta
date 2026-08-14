use super::super::*;
use crate::boundary::{
    ProviderEventText, ProviderImageBytes, ProviderName, ProviderText, ToolArgumentChunk,
};
use crate::bounds::{
    PROVIDER_REQUEST_BYTES_MAX, PROVIDER_STREAM_EVENTS_MAX, TOOL_ARGUMENT_BYTES_MAX,
};
use crate::ports::{ProviderEvent, ProviderUsage};
use lotta_domain::{BoundedJsonValue, ModelDescriptor, NonEmptyString};
use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) fn name(value: &str) -> ProviderName {
    ProviderName::new(value.into()).unwrap()
}

pub(super) fn text(value: &str) -> ProviderText {
    ProviderText::new(value.into()).unwrap()
}

pub(super) fn event_text(value: &str) -> ProviderEventText {
    ProviderEventText::new(value.into()).unwrap()
}

pub(super) fn call(value: &str) -> ToolCallId {
    ToolCallId::from_name(name(value))
}

pub(super) fn usage(input: u64, output: u64) -> ProviderUsage {
    ProviderUsage {
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: 0,
        reasoning_tokens: 0,
    }
}

fn descriptor() -> ModelDescriptor {
    ModelDescriptor {
        handle: NonEmptyString::new("model").unwrap(),
        provider_id: NonEmptyString::new("provider").unwrap(),
        available: true,
        context_window: Some(4_096),
        model_settings: None,
    }
}

pub(super) fn request(policy: ImagePolicy) -> ProviderRequest {
    ProviderRequest {
        model: descriptor(),
        system_prompt: Some(text("")),
        messages: ProviderMessages::new(Vec::new()).unwrap(),
        tools: ProviderTools::new(Vec::new()).unwrap(),
        tool_choice: ProviderToolChoice::Auto,
        image_policy: policy,
        context_tokens_max: TokenLimit::new(4_096).unwrap(),
        output_tokens_max: TokenLimit::new(1_024).unwrap(),
        reasoning: ReasoningControls {
            enabled: true,
            effort: Some(name("high")),
            tier: Some(name("priority")),
        },
        cancellation: CancellationToken::new(),
        deadline: ProviderDeadline::new(Duration::from_secs(30)).unwrap(),
    }
}

pub(crate) fn request_for_structural_test() -> ProviderRequest {
    request(ImagePolicy::Strict)
}

#[derive(Default)]
pub(super) struct TraceState {
    terminal: bool,
    stopped: bool,
    pub(super) usage: Option<ProviderUsage>,
    calls: ToolArgumentBuffer,
    metadata: Option<ProviderMetadata>,
}

pub(super) fn validate_trace(events: &[ProviderEvent]) -> Result<TraceState, crate::RuntimeError> {
    let mut state = TraceState::default();
    for event in events {
        if state.terminal {
            state.calls.terminal_error();
            return Err(invalid_test("provider event after terminal"));
        }
        apply_event(&mut state, event)?;
    }
    if !state.terminal {
        state.calls.terminal_error();
        return Err(invalid_test("provider terminal missing"));
    }
    Ok(state)
}

fn apply_event(state: &mut TraceState, event: &ProviderEvent) -> Result<(), crate::RuntimeError> {
    match event {
        ProviderEvent::ToolCallStart { call_id, .. } => state.calls.start(call_id.clone()),
        ProviderEvent::ToolCallArgumentsDelta { call_id, chunk } => {
            state.calls.append(call_id, chunk.as_slice())
        }
        ProviderEvent::ToolCallEnd { call_id } => state.calls.end(call_id).map(|_| ()),
        ProviderEvent::Usage { usage } => {
            if state
                .usage
                .is_some_and(|previous| !usage.is_monotonic_after(previous))
            {
                return Err(invalid_test("provider usage decreased"));
            }
            usage.checked_total()?;
            state.usage = Some(*usage);
            Ok(())
        }
        ProviderEvent::ProviderMetadata { metadata } => {
            state.metadata = Some(metadata.clone());
            Ok(())
        }
        ProviderEvent::Stop { .. } => {
            state.calls.terminal_stop()?;
            state.stopped = true;
            state.terminal = true;
            Ok(())
        }
        ProviderEvent::Error { .. } => {
            state.calls.terminal_error();
            state.terminal = true;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn invalid_test(context: &'static str) -> crate::RuntimeError {
    crate::RuntimeError::InvalidData {
        context: context.into(),
    }
}

pub(super) fn stop() -> ProviderEvent {
    ProviderEvent::Stop {
        reason: StopReason::EndTurn,
    }
}

#[derive(Clone)]
pub(super) enum SourceEvent {
    Text(String),
    Reasoning(String),
    Redacted(String),
    ToolStart(String, String),
    ToolArguments(String, Vec<u8>),
    ToolEnd(String),
    Usage(ProviderUsage),
    Metadata(ProviderMetadata),
    Stop,
    Error,
}

#[derive(Clone, Copy)]
pub(super) enum FaultMode {
    None,
    Reverse,
    DropReasoning,
    CollapseReasoning,
    RewriteStartId,
    RewriteDeltaId,
    RewriteEndId,
    RewriteToolName,
    DuplicateStart,
    UnknownEnd,
    ReplayEndedDelta,
    ReplayEndedEnd,
    MergeArguments,
    SplitArguments,
    CorruptArguments,
    OverflowArguments,
    DecreaseInput,
    DecreaseOutput,
    DecreaseCached,
    DecreaseReasoning,
    ImpossibleUsage,
    OverflowUsage,
    DropFinalUsage,
    OmitStop,
    DuplicateStop,
    EmitAfterStop,
    EmitError,
    EmitAfterError,
    DropImageParts,
    ReorderImageParts,
    MisclassifySecret,
    PersistNestedSecret,
}

pub(super) struct SourceProvider {
    pub(super) source: Vec<SourceEvent>,
    pub(super) fault: FaultMode,
}

impl SourceProvider {
    fn normalized(&self) -> Vec<ProviderEvent> {
        let mut output: Vec<_> = self.source.iter().map(normalize_source).collect();
        match self.fault {
            FaultMode::None
            | FaultMode::DropImageParts
            | FaultMode::ReorderImageParts
            | FaultMode::MisclassifySecret
            | FaultMode::PersistNestedSecret => {}
            FaultMode::Reverse => output.reverse(),
            FaultMode::DropReasoning => {
                output.retain(|event| !matches!(event, ProviderEvent::ReasoningDelta { .. }));
            }
            FaultMode::CollapseReasoning => {
                output.retain(|event| !matches!(event, ProviderEvent::RedactedReasoning { .. }));
            }
            FaultMode::RewriteStartId => rewrite_id(&mut output, EventKind::Start),
            FaultMode::RewriteDeltaId => rewrite_id(&mut output, EventKind::Delta),
            FaultMode::RewriteEndId => rewrite_id(&mut output, EventKind::End),
            FaultMode::RewriteToolName => rewrite_name(&mut output),
            FaultMode::DuplicateStart => duplicate_matching(&mut output, EventKind::Start),
            FaultMode::UnknownEnd => output.insert(
                0,
                ProviderEvent::ToolCallEnd {
                    call_id: call("unknown"),
                },
            ),
            FaultMode::ReplayEndedDelta => output.push(ProviderEvent::ToolCallArgumentsDelta {
                call_id: call("stable"),
                chunk: ToolArgumentChunk::new(b"{}".to_vec()).unwrap(),
            }),
            FaultMode::ReplayEndedEnd => output.push(ProviderEvent::ToolCallEnd {
                call_id: call("stable"),
            }),
            FaultMode::MergeArguments => merge_arguments(&mut output),
            FaultMode::SplitArguments => split_arguments(&mut output),
            FaultMode::CorruptArguments => corrupt_arguments(&mut output),
            FaultMode::OverflowArguments => output.insert(
                1,
                ProviderEvent::ToolCallArgumentsDelta {
                    call_id: call("stable"),
                    chunk: ToolArgumentChunk::new(vec![b'x'; TOOL_ARGUMENT_BYTES_MAX.value])
                        .unwrap(),
                },
            ),
            FaultMode::DecreaseInput => decrease_usage(&mut output, UsageField::Input),
            FaultMode::DecreaseOutput => decrease_usage(&mut output, UsageField::Output),
            FaultMode::DecreaseCached => decrease_usage(&mut output, UsageField::Cached),
            FaultMode::DecreaseReasoning => decrease_usage(&mut output, UsageField::Reasoning),
            FaultMode::ImpossibleUsage => output.insert(
                1,
                ProviderEvent::Usage {
                    usage: ProviderUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                        cached_input_tokens: 2,
                        reasoning_tokens: 0,
                    },
                },
            ),
            FaultMode::OverflowUsage => output.insert(
                1,
                ProviderEvent::Usage {
                    usage: ProviderUsage {
                        input_tokens: u64::MAX,
                        output_tokens: 1,
                        cached_input_tokens: 0,
                        reasoning_tokens: 0,
                    },
                },
            ),
            FaultMode::DropFinalUsage => {
                if let Some(index) = output
                    .iter()
                    .rposition(|event| matches!(event, ProviderEvent::Usage { .. }))
                {
                    output.remove(index);
                }
            }
            FaultMode::OmitStop => {
                output.retain(|event| !matches!(event, ProviderEvent::Stop { .. }));
            }
            FaultMode::DuplicateStop => output.push(stop()),
            FaultMode::EmitAfterStop => output.push(ProviderEvent::TextDelta {
                text: event_text("late"),
            }),
            FaultMode::EmitError => output = vec![normalize_source(&SourceEvent::Error)],
            FaultMode::EmitAfterError => {
                output = vec![normalize_source(&SourceEvent::Error), stop()];
            }
        }
        output
    }
}

#[derive(Clone, Copy)]
enum EventKind {
    Start,
    Delta,
    End,
}
#[derive(Clone, Copy)]
enum UsageField {
    Input,
    Output,
    Cached,
    Reasoning,
}

#[allow(clippy::collapsible_match)]
fn rewrite_id(output: &mut [ProviderEvent], kind: EventKind) {
    if let Some(event) = output.iter_mut().find(|event| {
        matches!(
            (kind, &**event),
            (EventKind::Start, ProviderEvent::ToolCallStart { .. })
                | (
                    EventKind::Delta,
                    ProviderEvent::ToolCallArgumentsDelta { .. }
                )
                | (EventKind::End, ProviderEvent::ToolCallEnd { .. })
        )
    }) {
        match event {
            ProviderEvent::ToolCallStart { call_id, .. }
            | ProviderEvent::ToolCallArgumentsDelta { call_id, .. }
            | ProviderEvent::ToolCallEnd { call_id } => *call_id = call("rewritten"),
            _ => {}
        }
    }
}
fn rewrite_name(output: &mut [ProviderEvent]) {
    if let Some(ProviderEvent::ToolCallStart { name, .. }) = output
        .iter_mut()
        .find(|event| matches!(event, ProviderEvent::ToolCallStart { .. }))
    {
        *name = event_text("rewritten");
    }
}
fn duplicate_matching(output: &mut Vec<ProviderEvent>, kind: EventKind) {
    if let Some(index) = output.iter().position(|event| {
        matches!(
            (kind, event),
            (EventKind::Start, ProviderEvent::ToolCallStart { .. })
        )
    }) {
        output.insert(index + 1, output[index].clone());
    }
}
fn merge_arguments(output: &mut Vec<ProviderEvent>) {
    let mut bytes = Vec::new();
    output.retain(|event| {
        if let ProviderEvent::ToolCallArgumentsDelta { chunk, .. } = event {
            bytes.extend_from_slice(chunk.as_slice());
            false
        } else {
            true
        }
    });
    output.insert(
        1,
        ProviderEvent::ToolCallArgumentsDelta {
            call_id: call("stable"),
            chunk: ToolArgumentChunk::new(bytes).unwrap(),
        },
    );
}
#[allow(clippy::collapsible_if)]
fn split_arguments(output: &mut Vec<ProviderEvent>) {
    if let Some(index) = output
        .iter()
        .position(|event| matches!(event, ProviderEvent::ToolCallArgumentsDelta { .. }))
    {
        if let ProviderEvent::ToolCallArgumentsDelta { call_id, chunk } = output.remove(index) {
            let bytes = chunk.into_vec();
            let at = bytes.len() / 2;
            output.insert(
                index,
                ProviderEvent::ToolCallArgumentsDelta {
                    call_id: call_id.clone(),
                    chunk: ToolArgumentChunk::new(bytes[..at].to_vec()).unwrap(),
                },
            );
            output.insert(
                index + 1,
                ProviderEvent::ToolCallArgumentsDelta {
                    call_id,
                    chunk: ToolArgumentChunk::new(bytes[at..].to_vec()).unwrap(),
                },
            );
        }
    }
}
fn corrupt_arguments(output: &mut [ProviderEvent]) {
    if let Some(ProviderEvent::ToolCallArgumentsDelta { chunk, .. }) = output
        .iter_mut()
        .find(|event| matches!(event, ProviderEvent::ToolCallArgumentsDelta { .. }))
    {
        *chunk = ToolArgumentChunk::new(b"{".to_vec()).unwrap();
    }
}
fn decrease_usage(output: &mut [ProviderEvent], field: UsageField) {
    if let Some(ProviderEvent::Usage { usage }) = output
        .iter_mut()
        .rfind(|event| matches!(event, ProviderEvent::Usage { .. }))
    {
        match field {
            UsageField::Input => usage.input_tokens = 0,
            UsageField::Output => usage.output_tokens = 0,
            UsageField::Cached => usage.cached_input_tokens = 0,
            UsageField::Reasoning => usage.reasoning_tokens = 0,
        }
    }
}

pub(super) fn normalize_source(source: &SourceEvent) -> ProviderEvent {
    match source {
        SourceEvent::Text(value) => ProviderEvent::TextDelta {
            text: event_text(value),
        },
        SourceEvent::Reasoning(value) => ProviderEvent::ReasoningDelta {
            text: event_text(value),
        },
        SourceEvent::Redacted(value) => ProviderEvent::RedactedReasoning {
            marker: event_text(value),
        },
        SourceEvent::ToolStart(id, tool) => ProviderEvent::ToolCallStart {
            call_id: call(id),
            name: event_text(tool),
        },
        SourceEvent::ToolArguments(id, bytes) => ProviderEvent::ToolCallArgumentsDelta {
            call_id: call(id),
            chunk: ToolArgumentChunk::new(bytes.clone()).unwrap(),
        },
        SourceEvent::ToolEnd(id) => ProviderEvent::ToolCallEnd { call_id: call(id) },
        SourceEvent::Usage(usage) => ProviderEvent::Usage { usage: *usage },
        SourceEvent::Metadata(metadata) => ProviderEvent::ProviderMetadata {
            metadata: metadata.clone(),
        },
        SourceEvent::Stop => stop(),
        SourceEvent::Error => ProviderEvent::Error {
            error: ProviderError::Unknown(ProviderErrorContext {
                code: name("source_error"),
                context: event_text("safe"),
            }),
        },
    }
}

impl ProviderPort for SourceProvider {
    fn stream(&self, request: ProviderRequest, events: ProviderEventSink) -> PortFuture<'_, ()> {
        let output = self.normalized();
        Box::pin(async move {
            let _ = request;
            for event in output {
                events.send(event).await?;
            }
            Ok(())
        })
    }
}

pub(super) fn poll_ready<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    for _ in 0..100_000 {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
    panic!("test future did not complete within bounded polls")
}

pub(super) fn run_port<P: ProviderPort>(
    provider: &P,
    request: ProviderRequest,
) -> Result<Vec<ProviderEvent>, crate::RuntimeError> {
    run_port_with_capacity(provider, request, PROVIDER_STREAM_EVENTS_MAX.value)
}

pub(super) fn run_port_with_capacity<P: ProviderPort>(
    provider: &P,
    request: ProviderRequest,
    capacity: usize,
) -> Result<Vec<ProviderEvent>, crate::RuntimeError> {
    let (sink, mut receiver) = provider_event_channel(capacity, &request.cancellation)?;
    let future = async move {
        let mut stream = std::pin::pin!(provider.stream(request, sink));
        let mut stream_done = false;
        let mut events = Vec::new();
        loop {
            tokio::select! {
                biased;
                result = &mut stream, if !stream_done => { result?; stream_done = true; }
                event = receiver.receive() => match event? {
                    Some(event) => events.push(event),
                    None if stream_done => return Ok(events),
                    None => stream_done = true,
                }
            }
        }
    };
    poll_ready(future)
}

pub(super) fn source_provider(source: Vec<SourceEvent>) -> SourceProvider {
    faulty_source_provider(source, FaultMode::None)
}

pub(super) fn faulty_source_provider(source: Vec<SourceEvent>, fault: FaultMode) -> SourceProvider {
    SourceProvider { source, fault }
}

pub(super) fn tool_source() -> Vec<SourceEvent> {
    vec![
        SourceEvent::ToolStart("stable".into(), "tool".into()),
        SourceEvent::ToolArguments("stable".into(), b"{\"value\":\"\xc3".to_vec()),
        SourceEvent::ToolArguments("stable".into(), b"\xa9\"}".to_vec()),
        SourceEvent::ToolEnd("stable".into()),
        SourceEvent::Stop,
    ]
}

pub(super) fn usage_source() -> Vec<SourceEvent> {
    vec![
        SourceEvent::Usage(ProviderUsage {
            input_tokens: 4,
            output_tokens: 4,
            cached_input_tokens: 2,
            reasoning_tokens: 2,
        }),
        SourceEvent::Usage(ProviderUsage {
            input_tokens: 5,
            output_tokens: 5,
            cached_input_tokens: 3,
            reasoning_tokens: 3,
        }),
        SourceEvent::Stop,
    ]
}

pub(super) fn run_validated<P: ProviderPort>(
    provider: &P,
) -> Result<TraceState, crate::RuntimeError> {
    let events = run_port(provider, request(ImagePolicy::Strict))?;
    validate_trace(&events)
}

pub(crate) fn provider_event_variants() {
    let call_id = call("call");
    let metadata = ProviderMetadata::classified(Vec::<ProviderMetadataInput>::new()).unwrap();
    let context = ProviderErrorContext {
        code: name("code"),
        context: event_text("context"),
    };
    let events = [
        ProviderEvent::TextDelta {
            text: event_text(""),
        },
        ProviderEvent::ReasoningDelta {
            text: event_text("r"),
        },
        ProviderEvent::RedactedReasoning {
            marker: event_text("x"),
        },
        ProviderEvent::ToolCallStart {
            call_id: call_id.clone(),
            name: event_text("tool"),
        },
        ProviderEvent::ToolCallArgumentsDelta {
            call_id: call_id.clone(),
            chunk: ToolArgumentChunk::new(b"{}".to_vec()).unwrap(),
        },
        ProviderEvent::ToolCallEnd { call_id },
        ProviderEvent::Usage { usage: usage(1, 2) },
        ProviderEvent::ProviderMetadata { metadata },
        stop(),
        ProviderEvent::Error {
            error: ProviderError::Unknown(context),
        },
    ];
    assert_eq!(events.len(), 10);
}

pub(crate) fn provider_error_variants() {
    fn context() -> ProviderErrorContext {
        ProviderErrorContext {
            code: name("stable"),
            context: event_text("scrubbed"),
        }
    }
    let errors = [
        ProviderError::Authentication(context()),
        ProviderError::Authorization(context()),
        ProviderError::InvalidRequest(context()),
        ProviderError::RateLimit(context()),
        ProviderError::Quota(context()),
        ProviderError::Timeout(context()),
        ProviderError::ContextOverflow(context()),
        ProviderError::Overloaded(context()),
        ProviderError::Unavailable(context()),
        ProviderError::Protocol(context()),
        ProviderError::Cancelled(context()),
        ProviderError::Unknown(context()),
    ];
    assert_eq!(errors.len(), 12);
}

pub(crate) fn model_descriptor_is_domain_type() {
    fn takes_domain(_: lotta_domain::ModelDescriptor) {}
    let ProviderRequest { model, .. } = request(ImagePolicy::Strict);
    takes_domain(model);
}

pub(crate) fn provider_request_fields_have_exact_types() {
    let ProviderRequest {
        model,
        system_prompt,
        messages,
        tools,
        tool_choice,
        image_policy,
        context_tokens_max,
        output_tokens_max,
        reasoning,
        cancellation,
        deadline,
    } = request(ImagePolicy::Strict);
    let _: ModelDescriptor = model;
    let _: Option<ProviderText> = system_prompt;
    let _: ProviderMessages = messages;
    let _: ProviderTools = tools;
    let _: ProviderToolChoice = tool_choice;
    let _: ImagePolicy = image_policy;
    let _: TokenLimit = context_tokens_max;
    let _: TokenLimit = output_tokens_max;
    let _: ReasoningControls = reasoning;
    let _: CancellationToken = cancellation;
    let _: ProviderDeadline = deadline;
}

pub(crate) fn async_channel_backpressure_and_cancellation() {
    let many: Vec<_> = (0..8)
        .map(|_| ProviderEvent::TextDelta {
            text: event_text("x"),
        })
        .chain(std::iter::once(stop()))
        .collect();
    let output = run_port_with_capacity(
        &source_provider(
            (0..8)
                .map(|_| SourceEvent::Text("x".into()))
                .chain(std::iter::once(SourceEvent::Stop))
                .collect(),
        ),
        request(ImagePolicy::Strict),
        1,
    )
    .unwrap();
    assert_eq!(output, many);

    let cancellation = CancellationToken::new();
    let (sink, mut receiver) = provider_event_channel(1, &cancellation).unwrap();
    poll_ready(sink.send(stop())).unwrap();
    let mut blocked = std::pin::pin!(sink.send(stop()));
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(blocked.as_mut().poll(&mut context), Poll::Pending));
    assert_eq!(poll_ready(receiver.receive()).unwrap(), Some(stop()));
    assert!(matches!(
        blocked.as_mut().poll(&mut context),
        Poll::Ready(Ok(()))
    ));

    {
        let mut pending_receive = Box::pin(receiver.receive());
        assert!(matches!(
            pending_receive.as_mut().poll(&mut context),
            Poll::Ready(Ok(Some(_)))
        ));
    }
    let mut waiting = Box::pin(receiver.receive());
    assert!(matches!(waiting.as_mut().poll(&mut context), Poll::Pending));
    poll_ready(sink.send(stop())).unwrap();
    assert!(matches!(
        waiting.as_mut().poll(&mut context),
        Poll::Ready(Ok(Some(_)))
    ));
    drop(waiting);

    poll_ready(sink.send(stop())).unwrap();
    let mut blocked = std::pin::pin!(sink.send(stop()));
    assert!(matches!(blocked.as_mut().poll(&mut context), Poll::Pending));
    cancellation.cancel();
    assert!(matches!(
        blocked.as_mut().poll(&mut context),
        Poll::Ready(Err(_))
    ));
    assert!(poll_ready(receiver.receive()).is_err());

    let closed_token = CancellationToken::new();
    let (closed_sink, closed_receiver) = provider_event_channel(1, &closed_token).unwrap();
    drop(closed_receiver);
    assert!(matches!(
        poll_ready(closed_sink.send(stop())),
        Err(crate::RuntimeError::AdapterFailure { .. })
    ));
}

pub(crate) fn provider_request_aggregate_bytes_are_bounded() {
    let mut request = request(ImagePolicy::Strict);
    request.model.model_settings =
        Some(serde_json::from_value(serde_json::json!({"nested":{"escaped":"a\n\"b"}})).unwrap());
    request.system_prompt = Some(text("escaped\n\"prompt"));
    request.messages = ProviderMessages::new(vec![ProviderMessage {
        role: ProviderMessageRole::Tool,
        content: ProviderContent::new(vec![
            ProviderContentPart::Text(text("message")),
            ProviderContentPart::Image {
                media_type: name("image/png"),
                bytes: ProviderImageBytes::new(vec![0, 1, 255]).unwrap(),
            },
        ])
        .unwrap(),
        tool_call_id: Some(call("call")),
    }])
    .unwrap();
    request.tools = ProviderTools::new(vec![ProviderToolDefinition {
        name: name("tool"),
        description: text("description"),
        input_schema: BoundedJsonValue::new(
            serde_json::json!({"type":"object","properties":{"quoted":{"const":"x\"y"}}}),
        )
        .unwrap(),
    }])
    .unwrap();
    request.tool_choice = ProviderToolChoice::Named(name("tool"));
    let measured = request.normalized_wire_bytes().unwrap();
    assert!(measured > serde_json::to_vec("escaped\n\"prompt").unwrap().len());
    assert_eq!(
        super::super::measure_test_value_with_limit(
            &serde_json::json!({"escaped":"a\n\"b"}),
            usize::MAX
        )
        .unwrap(),
        serde_json::to_vec(&serde_json::json!({"escaped":"a\n\"b"}))
            .unwrap()
            .len()
    );
    let bytes = vec![b'x'; 64];
    let exact = serde_json::to_vec(&bytes).unwrap().len();
    assert_eq!(
        super::super::measure_test_value_with_limit(&bytes, exact).unwrap(),
        exact
    );
    assert!(super::super::measure_test_value_with_limit(&bytes, exact - 1).is_err());
    assert!(super::super::checked_count_for_test(usize::MAX, 1, usize::MAX).is_err());
    request.system_prompt = Some(text(&"x".repeat(PROVIDER_REQUEST_BYTES_MAX.value)));
    assert!(request.validate_bytes().is_err());
}
