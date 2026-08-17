/// Observable provider behaviors that an adapter factory must arrange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderContractScenario {
    /// Every non-error event variant followed by exactly one successful stop.
    Success,
    /// A legal prefix followed by exactly one terminal provider error.
    TerminalError,
    /// Streaming is cancelled while blocked on the bounded receiver.
    Cancelled,
    /// The event receiver is closed before streaming.
    ReceiverClosed,
}

/// Event variants a provider dialect can honestly emit in a successful stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderContractShape {
    /// Successful streams can emit visible reasoning.
    pub reasoning: bool,
    /// Successful streams can emit opaque redacted reasoning.
    pub redacted_reasoning: bool,
    /// Successful streams can emit provider metadata.
    pub metadata: bool,
    /// Expected successful terminal reason.
    pub stop_reason: StopReason,
}

impl ProviderContractShape {
    /// Full fake/native contract retained for existing callers.
    pub const FULL: Self = Self {
        reasoning: true,
        redacted_reasoning: true,
        metadata: true,
        stop_reason: StopReason::EndTurn,
    };
    /// Honest Ollama NDJSON contract shape.
    pub const OLLAMA: Self = Self {
        reasoning: true,
        redacted_reasoning: false,
        metadata: false,
        stop_reason: StopReason::ToolUse,
    };
}

use crate::contract::common::assert_pending_once;
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{
    ProviderEvent, ProviderPort, ProviderRequest, StopReason, provider_event_channel,
};
use std::future::Future;
use tokio_util::sync::CancellationToken;

/// Runs provider streaming, cancellation, and terminal-event assertions.
///
/// # Panics
/// Panics when an adapter violates the asserted contract.
pub async fn provider_contract<F, Fut, Adapter>(factory: F, request: ProviderRequest)
where
    F: Fn(ProviderContractScenario) -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: ProviderPort,
{
    provider_contract_with_shape(factory, request, ProviderContractShape::FULL).await;
}

/// Runs the provider contract while requiring only events the dialect can honestly emit.
///
/// Lifecycle, ordering, terminal, cancellation, and receiver-closed invariants are unchanged.
///
/// # Panics
/// Panics when an adapter violates the asserted contract.
pub async fn provider_contract_with_shape<F, Fut, Adapter>(
    factory: F,
    request: ProviderRequest,
    shape: ProviderContractShape,
) where
    F: Fn(ProviderContractScenario) -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: ProviderPort,
{
    let port = factory(ProviderContractScenario::Success).await;
    let events = collect_provider_events(&port, request.clone()).await;
    assert_provider_events(&events, shape);
    let port = factory(ProviderContractScenario::TerminalError).await;
    let error_events = collect_provider_events(&port, request.clone()).await;
    assert!(matches!(
        &error_events[..],
        [ProviderEvent::TextDelta { .. }, ProviderEvent::Error { .. }]
    ));
    assert_eq!(
        error_events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Error { .. }))
            .count(),
        1
    );
    assert!(
        !error_events
            .iter()
            .any(|event| matches!(event, ProviderEvent::Stop { .. }))
    );
    let port = factory(ProviderContractScenario::ReceiverClosed).await;
    let cancellation = CancellationToken::new();
    let (sink, receiver) = provider_event_channel(1, &cancellation).expect("closed channel");
    drop(receiver);
    assert!(matches!(
        port.stream(request.clone(), sink).await,
        Err(RuntimeError::AdapterFailure { .. })
    ));
    let port = factory(ProviderContractScenario::Cancelled).await;
    let cancellation = request.cancellation.clone();
    let (sink, mut receiver) = provider_event_channel(1, &cancellation).expect("cancel channel");
    let stream = port.stream(request, sink);
    tokio::pin!(stream);
    assert_pending_once(stream.as_mut()).await;
    cancellation.cancel();
    assert!(matches!(stream.await, Err(RuntimeError::Cancelled { .. })));
    assert!(matches!(
        receiver.receive().await,
        Err(RuntimeError::Cancelled { .. })
    ));
}

async fn collect_provider_events(
    port: &impl ProviderPort,
    request: ProviderRequest,
) -> Vec<ProviderEvent> {
    let (sink, mut receiver) = provider_event_channel(1, &request.cancellation).expect("channel");
    let stream = port.stream(request, sink);
    tokio::pin!(stream);
    let mut events = Vec::new();
    loop {
        tokio::select! {
            result = &mut stream => {
                result.expect("provider stream");
                while let Some(value) = receiver.receive().await.expect("event") {
                    events.push(value);
                }
                break;
            }
            value = receiver.receive() => {
                if let Some(value) = value.expect("event") {
                    events.push(value);
                }
            }
        }
    }
    events
}

fn assert_provider_events(events: &[ProviderEvent], shape: ProviderContractShape) {
    let mut index = 0;
    assert!(
        matches!(events[index], ProviderEvent::TextDelta { .. }),
        "first event: {:?}",
        events.get(index)
    );
    index += 1;
    if shape.reasoning {
        assert!(matches!(
            events[index],
            ProviderEvent::ReasoningDelta { .. }
        ));
        index += 1;
    }
    if shape.redacted_reasoning {
        assert!(matches!(
            events[index],
            ProviderEvent::RedactedReasoning { .. }
        ));
        index += 1;
    }
    let call_id = match &events[index] {
        ProviderEvent::ToolCallStart { call_id, .. } => call_id.as_str(),
        _ => "",
    };
    index += 1;
    assert!(matches!(
        &events[index],
        ProviderEvent::ToolCallArgumentsDelta { call_id: id, chunk }
            if id.as_str() == call_id && chunk.as_slice() == b"{}"
    ));
    index += 1;
    assert!(matches!(
        &events[index],
        ProviderEvent::ToolCallEnd { call_id: id } if id.as_str() == call_id
    ));
    index += 1;
    let ProviderEvent::Usage { usage: first } = events[index] else {
        panic!("first usage");
    };
    index += 1;
    if shape.metadata {
        assert!(matches!(
            &events[index],
            ProviderEvent::ProviderMetadata { metadata }
                if !format!("{metadata:?}").to_ascii_lowercase().contains("secret")
        ));
        index += 1;
    }
    let ProviderEvent::Usage { usage: final_usage } = events[index] else {
        panic!("final usage");
    };
    assert!(final_usage.is_monotonic_after(first));
    index += 1;
    assert!(matches!(
        events[index],
        ProviderEvent::Stop { reason } if reason == shape.stop_reason
    ));
    index += 1;
    assert_eq!(events.len(), index);
    assert_success_terminal(events);
}

fn assert_success_terminal(events: &[ProviderEvent]) {
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Stop { .. }))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProviderEvent::Error { .. }))
    );
}
