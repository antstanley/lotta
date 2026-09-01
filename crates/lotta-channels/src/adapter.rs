//! Production composition boundary between channel adapters and Runtime WebSockets.

use lotta_domain::ChannelRoute;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tokio::sync::{Mutex, mpsc, oneshot};

/// Maximum queued inbound, outbound, or runtime-event deliveries.
pub const CHANNEL_ADAPTER_DELIVERIES_MAX: usize = 256;
/// Maximum UTF-8 bytes in one normalized adapter message or event.
pub const CHANNEL_ADAPTER_MESSAGE_BYTES_MAX: usize = 16 * 1024 * 1024;
/// Maximum pending Runtime input correlations.
pub const CHANNEL_ADAPTER_INPUT_CORRELATIONS_MAX: usize = 256;

/// A normalized inbound adapter message submitted to the Runtime WebSocket owner.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InboundChannelMessage {
    /// Exact canonical route selected by the adapter.
    pub route: ChannelRoute,
    /// Stable adapter-generated message identifier.
    pub client_message_id: String,
    /// Canonical bounded Runtime input payload.
    pub payload: serde_json::Value,
}

/// Typed Runtime events delivered back to the originating adapter.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdapterEvent {
    /// Runtime accepted one correlated input.
    InputAccepted {
        /// Runtime request correlation.
        request_id: String,
        /// Complete canonical frame retained for adapter UX.
        #[serde(flatten)]
        fields: BTreeMap<String, serde_json::Value>,
    },
    /// Ordered model stream delta.
    StreamDelta {
        /// Runtime request correlation.
        request_id: String,
        /// Complete canonical frame retained for adapter UX.
        #[serde(flatten)]
        fields: BTreeMap<String, serde_json::Value>,
    },
    /// Ordered runtime state update.
    UpdateLoopStatus {
        /// Runtime request correlation when supplied by the protocol.
        request_id: Option<String>,
        /// Complete canonical frame retained for adapter UX.
        #[serde(flatten)]
        fields: BTreeMap<String, serde_json::Value>,
    },
    /// Exactly one terminal turn event.
    TurnFinished {
        /// Runtime request correlation.
        request_id: String,
        /// Complete canonical frame retained for adapter UX.
        #[serde(flatten)]
        fields: BTreeMap<String, serde_json::Value>,
    },
}

impl AdapterEvent {
    fn request_id(&self) -> Option<&str> {
        match self {
            Self::InputAccepted { request_id, .. }
            | Self::StreamDelta { request_id, .. }
            | Self::TurnFinished { request_id, .. } => Some(request_id),
            Self::UpdateLoopStatus { request_id, .. } => request_id.as_deref(),
        }
    }
}

/// One outbound `MessageChannel` delivery sent to an adapter.
#[derive(Debug)]
pub struct MessageChannelDelivery {
    /// Exact canonical route/runtime scope.
    pub route: ChannelRoute,
    /// Runtime external-tool request identifier.
    pub request_id: String,
    /// Runtime tool-call identifier.
    pub tool_call_id: String,
    /// Visible message text.
    pub message: String,
    completion: oneshot::Sender<MessageChannelResult>,
}

impl MessageChannelDelivery {
    /// Completes this exact outbound delivery once.
    pub fn complete(self, result: MessageChannelResult) {
        let _ = self.completion.send(result);
    }
}

/// Result returned by an adapter for a visible outbound delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageChannelResult {
    /// The adapter accepted the visible message.
    Delivered,
    /// No adapter is attached for the configured channel.
    Unavailable,
    /// The adapter rejected delivery with a bounded non-secret reason.
    Failed(String),
}

/// Stable bounded adapter boundary error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ChannelAdapterError {
    /// No live channel host owns the adapter boundary.
    #[error("channel runtime unavailable")]
    Unavailable,
    /// The bounded adapter queue is full.
    #[error("channel runtime busy")]
    Busy,
    /// The normalized message exceeds its byte ceiling or violates ordering.
    #[error("channel message exceeds bounds")]
    Bound,
}

/// Cloneable production port handed to one adapter factory.
#[derive(Clone)]
pub struct ChannelRuntimeClient {
    inbound: mpsc::Sender<InboundChannelMessage>,
    outbound: Arc<Mutex<mpsc::Receiver<MessageChannelDelivery>>>,
    events: Arc<Mutex<mpsc::Receiver<AdapterEvent>>>,
}

impl ChannelRuntimeClient {
    /// Submits one inbound message without unbounded waiting.
    ///
    /// # Errors
    /// Returns unavailable, busy, or bound without admitting the message.
    pub fn submit_inbound(
        &self,
        message: InboundChannelMessage,
    ) -> Result<(), ChannelAdapterError> {
        validate_inbound(&message)?;
        self.inbound
            .try_send(message)
            .map_err(|error| map_try_send(&error))
    }

    /// Receives the next outbound `MessageChannel` delivery.
    pub async fn receive_outbound(&self) -> Option<MessageChannelDelivery> {
        self.outbound.lock().await.recv().await
    }

    /// Receives the next typed, order-checked Runtime event.
    pub async fn receive_event(&self) -> Option<AdapterEvent> {
        self.events.lock().await.recv().await
    }
}

fn map_try_send<T>(error: &mpsc::error::TrySendError<T>) -> ChannelAdapterError {
    match error {
        mpsc::error::TrySendError::Full(_) => ChannelAdapterError::Busy,
        mpsc::error::TrySendError::Closed(_) => ChannelAdapterError::Unavailable,
    }
}

/// Immutable route context supplied at the real adapter factory boundary.
#[derive(Clone, Debug)]
pub struct AdapterFactoryContext {
    /// Exact canonical route owned by this adapter attachment.
    pub route: ChannelRoute,
}

/// Factory boundary for future concrete adapters.
pub trait ChannelAdapterFactory: Send + Sync {
    /// Starts an adapter using the same bounded production port as every other adapter.
    ///
    /// # Errors
    /// Returns a bounded adapter failure without installing a partial attachment.
    fn start(
        &self,
        context: AdapterFactoryContext,
        runtime: ChannelRuntimeClient,
    ) -> Result<(), ChannelAdapterError>;
}

/// Child composition context containing adapter factories, not test-global seams.
#[derive(Clone, Default)]
pub struct HostContext {
    factories: BTreeMap<String, Arc<dyn ChannelAdapterFactory>>,
}

impl HostContext {
    /// Creates the production context. Tasks 79–83 register concrete factories here.
    #[must_use]
    pub fn production() -> Self {
        Self::default()
    }

    /// Registers one channel plugin factory before the host starts.
    ///
    /// # Errors
    /// Rejects empty and duplicate channel identifiers.
    pub fn register_factory(
        &mut self,
        channel: impl Into<String>,
        factory: Arc<dyn ChannelAdapterFactory>,
    ) -> Result<(), ChannelAdapterError> {
        let channel = channel.into();
        if channel.is_empty() || self.factories.insert(channel, factory).is_some() {
            return Err(ChannelAdapterError::Bound);
        }
        Ok(())
    }
}

struct AdapterPort {
    inbound: mpsc::Receiver<InboundChannelMessage>,
    outbound: mpsc::Sender<MessageChannelDelivery>,
    events: mpsc::Sender<AdapterEvent>,
}

fn channel_adapter_port() -> (ChannelRuntimeClient, AdapterPort) {
    let (inbound_sender, inbound) = mpsc::channel(CHANNEL_ADAPTER_DELIVERIES_MAX);
    let (outbound, outbound_receiver) = mpsc::channel(CHANNEL_ADAPTER_DELIVERIES_MAX);
    let (events, event_receiver) = mpsc::channel(CHANNEL_ADAPTER_DELIVERIES_MAX);
    (
        ChannelRuntimeClient {
            inbound: inbound_sender,
            outbound: Arc::new(Mutex::new(outbound_receiver)),
            events: Arc::new(Mutex::new(event_receiver)),
        },
        AdapterPort {
            inbound,
            outbound,
            events,
        },
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputPhase {
    Submitted,
    Accepted,
    Streaming,
    State,
    Terminal,
}

struct PendingInput {
    route_key: RuntimeKey,
    phase: InputPhase,
}

type RuntimeKey = (String, String);

/// Real bounded adapter hub owned by the child Runtime WebSocket loop.
pub struct AdapterHub {
    inbound: mpsc::Receiver<InboundChannelMessage>,
    outbound: BTreeMap<RuntimeKey, mpsc::Sender<MessageChannelDelivery>>,
    events: BTreeMap<RuntimeKey, mpsc::Sender<AdapterEvent>>,
    pending: BTreeMap<String, PendingInput>,
    forwarders: Vec<tokio::task::JoinHandle<()>>,
}

impl AdapterHub {
    /// Attaches every configured route through the registered production factory boundary.
    ///
    /// # Errors
    /// Returns when any registered factory rejects its bounded attachment.
    pub fn compose(
        routes: &[ChannelRoute],
        context: &HostContext,
    ) -> Result<Self, ChannelAdapterError> {
        let (inbound_sender, inbound) = mpsc::channel(CHANNEL_ADAPTER_DELIVERIES_MAX);
        let mut hub = Self {
            inbound,
            outbound: BTreeMap::new(),
            events: BTreeMap::new(),
            pending: BTreeMap::new(),
            forwarders: Vec::new(),
        };
        let mut attached = BTreeSet::new();
        for route in routes {
            let key = runtime_key(route);
            if !attached.insert(key.clone()) {
                continue;
            }
            let Some(factory) = context.factories.get(route.channel_id.as_str()) else {
                continue;
            };
            let (client, port) = channel_adapter_port();
            factory.start(
                AdapterFactoryContext {
                    route: route.clone(),
                },
                client,
            )?;
            hub.outbound.insert(key.clone(), port.outbound);
            hub.events.insert(key, port.events);
            hub.forwarders
                .push(forward_inbound(port.inbound, inbound_sender.clone()));
        }
        Ok(hub)
    }

    /// Receives the next adapter input admitted by any attached factory.
    pub async fn receive_inbound(&mut self) -> Option<InboundChannelMessage> {
        self.inbound.recv().await
    }

    /// Records the exact request generated for an admitted adapter input.
    ///
    /// # Errors
    /// Rejects duplicate identifiers and a full correlation table.
    pub fn record_input(
        &mut self,
        request_id: String,
        route: &ChannelRoute,
    ) -> Result<(), ChannelAdapterError> {
        if self.pending.len() >= CHANNEL_ADAPTER_INPUT_CORRELATIONS_MAX
            || self.pending.contains_key(&request_id)
        {
            return Err(ChannelAdapterError::Busy);
        }
        self.pending.insert(
            request_id,
            PendingInput {
                route_key: runtime_key(route),
                phase: InputPhase::Submitted,
            },
        );
        Ok(())
    }

    /// Parses, bounds, correlates, and orders one Runtime event before adapter delivery.
    ///
    /// # Errors
    /// Rejects oversized, unknown-correlation, duplicate, or out-of-order events.
    pub fn accept_runtime_event(
        &mut self,
        value: &serde_json::Value,
    ) -> Result<bool, ChannelAdapterError> {
        let event: AdapterEvent = match serde_json::from_value(value.clone()) {
            Ok(event) => event,
            Err(_) => return Ok(false),
        };
        validate_event_bound(&event)?;
        let request_id = event
            .request_id()
            .ok_or(ChannelAdapterError::Bound)?
            .to_owned();
        let pending = self
            .pending
            .get_mut(&request_id)
            .ok_or(ChannelAdapterError::Bound)?;
        pending.phase = next_phase(pending.phase, &event)?;
        let terminal = pending.phase == InputPhase::Terminal;
        let sender = self
            .events
            .get(&pending.route_key)
            .ok_or(ChannelAdapterError::Unavailable)?;
        sender
            .try_send(event)
            .map_err(|error| map_try_send(&error))?;
        if terminal {
            self.pending.remove(&request_id);
        }
        Ok(true)
    }

    /// Delivers a correlated `MessageChannel` call through the route's attached adapter.
    pub async fn deliver(
        &self,
        route: &ChannelRoute,
        request_id: &str,
        tool_call_id: &str,
        message: &str,
    ) -> MessageChannelResult {
        let Some(adapter) = self.outbound.get(&runtime_key(route)) else {
            return MessageChannelResult::Unavailable;
        };
        let (delivery, completion) = delivery(
            route.clone(),
            request_id.into(),
            tool_call_id.into(),
            message.into(),
        );
        if adapter.try_send(delivery).is_err() {
            return MessageChannelResult::Unavailable;
        }
        tokio::time::timeout(
            std::time::Duration::from_millis(crate::host::CHANNEL_ADAPTER_DELIVERY_TIMEOUT_MS),
            completion,
        )
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(MessageChannelResult::Unavailable)
    }
}

fn forward_inbound(
    mut receiver: mpsc::Receiver<InboundChannelMessage>,
    sender: mpsc::Sender<InboundChannelMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            if sender.send(message).await.is_err() {
                break;
            }
        }
    })
}

fn next_phase(
    current: InputPhase,
    event: &AdapterEvent,
) -> Result<InputPhase, ChannelAdapterError> {
    match (current, event) {
        (InputPhase::Submitted, AdapterEvent::InputAccepted { .. }) => Ok(InputPhase::Accepted),
        (InputPhase::Accepted | InputPhase::Streaming, AdapterEvent::StreamDelta { .. }) => {
            Ok(InputPhase::Streaming)
        }
        (InputPhase::Accepted | InputPhase::Streaming, AdapterEvent::UpdateLoopStatus { .. }) => {
            Ok(InputPhase::State)
        }
        (InputPhase::State, AdapterEvent::TurnFinished { .. }) => Ok(InputPhase::Terminal),
        _ => Err(ChannelAdapterError::Bound),
    }
}

fn runtime_key(route: &ChannelRoute) -> RuntimeKey {
    (
        route.agent_id.as_str().to_owned(),
        route.conversation_id.as_str().to_owned(),
    )
}

fn validate_event_bound(event: &AdapterEvent) -> Result<(), ChannelAdapterError> {
    if serde_json::to_vec(event)
        .map_err(|_| ChannelAdapterError::Bound)?
        .len()
        > CHANNEL_ADAPTER_MESSAGE_BYTES_MAX
    {
        return Err(ChannelAdapterError::Bound);
    }
    Ok(())
}

fn delivery(
    route: ChannelRoute,
    request_id: String,
    tool_call_id: String,
    message: String,
) -> (
    MessageChannelDelivery,
    oneshot::Receiver<MessageChannelResult>,
) {
    let (completion, receiver) = oneshot::channel();
    (
        MessageChannelDelivery {
            route,
            request_id,
            tool_call_id,
            message,
            completion,
        },
        receiver,
    )
}

fn validate_inbound(message: &InboundChannelMessage) -> Result<(), ChannelAdapterError> {
    if message.client_message_id.is_empty()
        || message.client_message_id.len() > crate::control_plane::CONTROL_ID_BYTES_MAX
        || !message.route.enabled
        || message.payload["client_message_id"].as_str() != Some(message.client_message_id.as_str())
        || serde_json::to_vec(&message.payload)
            .map_err(|_| ChannelAdapterError::Bound)?
            .len()
            > CHANNEL_ADAPTER_MESSAGE_BYTES_MAX
    {
        return Err(ChannelAdapterError::Bound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct FakeFactory(StdMutex<Option<ChannelRuntimeClient>>);
    impl ChannelAdapterFactory for FakeFactory {
        fn start(
            &self,
            _context: AdapterFactoryContext,
            runtime: ChannelRuntimeClient,
        ) -> Result<(), ChannelAdapterError> {
            *self
                .0
                .lock()
                .map_err(|_| ChannelAdapterError::Unavailable)? = Some(runtime);
            Ok(())
        }
    }

    fn route() -> ChannelRoute {
        serde_json::from_value(serde_json::json!({
            "channel_id":"telegram", "account_id":"main", "chat_id":"chat",
            "agent_id":"agent-local-a", "conversation_id":"default", "enabled":true,
            "outbound_enabled":true, "created_at":"2026-01-01T00:00:00Z",
            "updated_at":"2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn factory_composition_drives_ordered_runtime_and_message_channel_ports() {
        let factory = Arc::new(FakeFactory::default());
        let mut context = HostContext::production();
        context
            .register_factory("telegram", factory.clone())
            .unwrap();
        let route = route();
        let mut hub = AdapterHub::compose(std::slice::from_ref(&route), &context).unwrap();
        let client = factory.0.lock().unwrap().clone().unwrap();
        client
            .submit_inbound(InboundChannelMessage {
                route: route.clone(),
                client_message_id: "client-1".into(),
                payload: serde_json::json!({"client_message_id":"client-1","content":"hello"}),
            })
            .unwrap();
        let inbound = hub.receive_inbound().await.unwrap();
        assert_eq!(inbound.client_message_id, "client-1");
        hub.record_input("input-1".into(), &route).unwrap();
        for event in [
            serde_json::json!({"type":"input_accepted","request_id":"input-1"}),
            serde_json::json!({"type":"stream_delta","request_id":"input-1","delta":"hi"}),
            serde_json::json!({"type":"update_loop_status","request_id":"input-1","status":"idle"}),
            serde_json::json!({"type":"turn_finished","request_id":"input-1"}),
        ] {
            assert!(hub.accept_runtime_event(&event).unwrap());
        }
        assert!(matches!(
            client.receive_event().await,
            Some(AdapterEvent::InputAccepted { .. })
        ));
        assert!(
            hub.accept_runtime_event(&serde_json::json!({
                "type":"turn_finished", "request_id":"input-1"
            }))
            .is_err()
        );
        let delivery = tokio::spawn({
            let client = client.clone();
            async move {
                let delivery = client.receive_outbound().await.unwrap();
                assert_eq!(delivery.tool_call_id, "tool-1");
                delivery.complete(MessageChannelResult::Delivered);
            }
        });
        assert_eq!(
            hub.deliver(&route, "request-1", "tool-1", "visible").await,
            MessageChannelResult::Delivered
        );
        delivery.await.unwrap();
    }
}
