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
    let port = factory(ProviderContractScenario::Success).await;
    let events = collect_provider_events(&port, request.clone()).await;
    assert_provider_events(&events);
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

fn assert_provider_events(events: &[ProviderEvent]) {
    assert_eq!(events.len(), 10);
    assert!(matches!(events[0], ProviderEvent::TextDelta { .. }));
    assert!(matches!(events[1], ProviderEvent::ReasoningDelta { .. }));
    assert!(matches!(events[2], ProviderEvent::RedactedReasoning { .. }));
    let call_id = match &events[3] {
        ProviderEvent::ToolCallStart { call_id, .. } => call_id.as_str(),
        _ => "",
    };
    assert!(matches!(
        &events[4],
        ProviderEvent::ToolCallArgumentsDelta { call_id: id, chunk }
            if id.as_str() == call_id && chunk.as_slice() == b"{}"
    ));
    assert!(
        matches!(&events[5], ProviderEvent::ToolCallEnd { call_id: id } if id.as_str() == call_id)
    );
    let ProviderEvent::Usage { usage: first } = events[6] else {
        panic!("first usage");
    };
    assert!(matches!(
        &events[7],
        ProviderEvent::ProviderMetadata { metadata }
            if !format!("{metadata:?}").to_ascii_lowercase().contains("secret")
    ));
    let ProviderEvent::Usage { usage: final_usage } = events[8] else {
        panic!("final usage");
    };
    assert!(final_usage.is_monotonic_after(first));
    assert!(matches!(
        events[9],
        ProviderEvent::Stop {
            reason: StopReason::EndTurn
        }
    ));
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
