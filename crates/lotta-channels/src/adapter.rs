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
pub enum AdapterRuntimeEvent {
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
    /// Exactly one typed terminal Runtime failure for a correlated input.
    #[serde(alias = "error")]
    RuntimeError {
        /// Runtime request correlation.
        request_id: String,
        /// Stable bounded non-secret failure code or detail.
        error: serde_json::Value,
        /// Complete canonical frame retained for adapter UX.
        #[serde(flatten)]
        fields: BTreeMap<String, serde_json::Value>,
    },
}

/// Backwards-compatible name for the typed adapter Runtime event boundary.
pub type AdapterEvent = AdapterRuntimeEvent;

impl AdapterRuntimeEvent {
    fn request_id(&self) -> Option<&str> {
        match self {
            Self::InputAccepted { request_id, .. }
            | Self::StreamDelta { request_id, .. }
            | Self::TurnFinished { request_id, .. }
            | Self::RuntimeError { request_id, .. } => Some(request_id),
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
    route_key: CanonicalRouteKey,
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
        if CanonicalRouteKey::from_route(&message.route) != self.route_key {
            return Err(ChannelAdapterError::Bound);
        }
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

fn channel_adapter_port(route_key: CanonicalRouteKey) -> (ChannelRuntimeClient, AdapterPort) {
    let (inbound_sender, inbound) = mpsc::channel(CHANNEL_ADAPTER_DELIVERIES_MAX);
    let (outbound, outbound_receiver) = mpsc::channel(CHANNEL_ADAPTER_DELIVERIES_MAX);
    let (events, event_receiver) = mpsc::channel(CHANNEL_ADAPTER_DELIVERIES_MAX);
    (
        ChannelRuntimeClient {
            route_key,
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

/// Full canonical adapter route identity, including its Runtime scope.
///
/// Mutable route flags and timestamps are deliberately excluded: they are route
/// metadata, not delivery identity. Explicitly absent and explicitly-null thread
/// identifiers remain distinct, matching the canonical domain representation.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CanonicalRouteKey {
    /// Channel plugin identifier.
    pub channel_id: String,
    /// Channel account identifier.
    pub account_id: String,
    /// Canonical chat/target identifier.
    pub chat_id: String,
    /// Canonical optional thread identity with explicit-null preservation.
    pub thread_id: Option<Option<String>>,
    /// Runtime agent scope.
    pub agent_id: String,
    /// Runtime conversation scope.
    pub conversation_id: String,
}

impl CanonicalRouteKey {
    /// Constructs the full immutable delivery identity from a canonical route.
    #[must_use]
    pub fn from_route(route: &ChannelRoute) -> Self {
        Self {
            channel_id: route.channel_id.as_str().to_owned(),
            account_id: route.account_id.as_str().to_owned(),
            chat_id: route.chat_id.as_str().to_owned(),
            thread_id: route.thread_id.clone(),
            agent_id: route.agent_id.as_str().to_owned(),
            conversation_id: route.conversation_id.as_str().to_owned(),
        }
    }

    fn runtime_key(&self) -> RuntimeKey {
        (self.agent_id.clone(), self.conversation_id.clone())
    }
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
    route_key: CanonicalRouteKey,
    phase: InputPhase,
}

type RuntimeKey = (String, String);

/// Real bounded adapter hub owned by the child Runtime WebSocket loop.
pub struct AdapterHub {
    inbound: mpsc::Receiver<InboundChannelMessage>,
    outbound: BTreeMap<CanonicalRouteKey, mpsc::Sender<MessageChannelDelivery>>,
    events: BTreeMap<CanonicalRouteKey, mpsc::Sender<AdapterRuntimeEvent>>,
    pending: BTreeMap<String, PendingInput>,
    active_routes: BTreeMap<RuntimeKey, CanonicalRouteKey>,
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
            active_routes: BTreeMap::new(),
            forwarders: Vec::new(),
        };
        let mut attached = BTreeSet::new();
        for route in routes {
            let key = CanonicalRouteKey::from_route(route);
            if !attached.insert(key.clone()) {
                continue;
            }
            let Some(factory) = context.factories.get(route.channel_id.as_str()) else {
                continue;
            };
            let (client, port) = channel_adapter_port(key.clone());
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
        let route_key = CanonicalRouteKey::from_route(route);
        let runtime = route_key.runtime_key();
        if self.pending.len() >= CHANNEL_ADAPTER_INPUT_CORRELATIONS_MAX
            || self.pending.contains_key(&request_id)
            || self.active_routes.contains_key(&runtime)
            || !self.events.contains_key(&route_key)
        {
            return Err(ChannelAdapterError::Busy);
        }
        self.active_routes.insert(runtime, route_key.clone());
        self.pending.insert(
            request_id,
            PendingInput {
                route_key,
                phase: InputPhase::Submitted,
            },
        );
        Ok(())
    }

    fn correlate_missing_request_id(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, ChannelAdapterError> {
        if value.get("request_id").is_some() || !runtime_event_kind(value) {
            return Ok(None);
        }
        let runtime = value
            .get("runtime")
            .and_then(serde_json::Value::as_object)
            .and_then(|runtime| {
                Some((
                    runtime.get("agent_id")?.as_str()?.to_owned(),
                    runtime.get("conversation_id")?.as_str()?.to_owned(),
                ))
            })
            .ok_or(ChannelAdapterError::Bound)?;
        let route = self
            .active_routes
            .get(&runtime)
            .ok_or(ChannelAdapterError::Bound)?;
        let request_id = self
            .pending
            .iter()
            .find_map(|(request_id, pending)| (&pending.route_key == route).then_some(request_id))
            .ok_or(ChannelAdapterError::Bound)?;
        let mut object = value
            .as_object()
            .cloned()
            .ok_or(ChannelAdapterError::Bound)?;
        object.insert("request_id".to_owned(), serde_json::json!(request_id));
        Ok(Some(serde_json::Value::Object(object)))
    }

    /// Parses, bounds, correlates, and orders one Runtime event before adapter delivery.
    ///
    /// # Errors
    /// Rejects oversized, unknown-correlation, duplicate, or out-of-order events.
    pub fn accept_runtime_event(
        &mut self,
        value: &serde_json::Value,
    ) -> Result<bool, ChannelAdapterError> {
        let correlated = self.correlate_missing_request_id(value)?;
        let value = correlated.as_ref().unwrap_or(value);
        let event = match parse_runtime_event(value) {
            Ok(Some(event)) => event,
            Ok(None) => return Ok(false),
            Err(error) => return Err(error),
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
        let route_key = pending.route_key.clone();
        let sender = self
            .events
            .get(&route_key)
            .ok_or(ChannelAdapterError::Unavailable)?;
        sender
            .try_send(event)
            .map_err(|error| map_try_send(&error))?;
        if terminal {
            self.pending.remove(&request_id);
            self.active_routes.remove(&route_key.runtime_key());
        }
        Ok(true)
    }

    /// Clears the exact active input on cancellation or a terminal transport error.
    ///
    /// # Errors
    /// Rejects unknown and already-terminal correlations.
    pub fn cancel_input(&mut self, request_id: &str) -> Result<(), ChannelAdapterError> {
        let pending = self
            .pending
            .remove(request_id)
            .ok_or(ChannelAdapterError::Bound)?;
        self.active_routes.remove(&pending.route_key.runtime_key());
        Ok(())
    }

    /// Returns whether this route's serialized Runtime currently has an active input.
    #[must_use]
    pub fn runtime_is_active(&self, route: &ChannelRoute) -> bool {
        self.active_routes
            .contains_key(&CanonicalRouteKey::from_route(route).runtime_key())
    }

    /// Returns the full route identity currently owning a serialized Runtime turn.
    #[must_use]
    pub fn active_route_key(
        &self,
        agent_id: &str,
        conversation_id: &str,
    ) -> Option<&CanonicalRouteKey> {
        self.active_routes
            .get(&(agent_id.to_owned(), conversation_id.to_owned()))
    }

    /// Delivers a correlated `MessageChannel` call through the exact route's adapter.
    pub async fn deliver(
        &self,
        route: &ChannelRoute,
        request_id: &str,
        tool_call_id: &str,
        message: &str,
    ) -> MessageChannelResult {
        let route_key = CanonicalRouteKey::from_route(route);
        let Some(adapter) = self.outbound.get(&route_key) else {
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
        (InputPhase::State, AdapterRuntimeEvent::TurnFinished { .. })
        | (
            InputPhase::Submitted
            | InputPhase::Accepted
            | InputPhase::Streaming
            | InputPhase::State,
            AdapterRuntimeEvent::RuntimeError { .. },
        ) => Ok(InputPhase::Terminal),
        _ => Err(ChannelAdapterError::Bound),
    }
}

fn runtime_event_kind(value: &serde_json::Value) -> bool {
    matches!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some("stream_delta" | "update_loop_status" | "turn_finished")
    )
}

fn parse_runtime_event(
    value: &serde_json::Value,
) -> Result<Option<AdapterRuntimeEvent>, ChannelAdapterError> {
    let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    if kind == "input_accepted" && value.get("accepted") == Some(&serde_json::Value::Bool(false)) {
        let request_id = value
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(ChannelAdapterError::Bound)?
            .to_owned();
        let error = value
            .get("error")
            .filter(|error| error.is_string() || error.is_object())
            .cloned()
            .ok_or(ChannelAdapterError::Bound)?;
        let mut fields = value
            .as_object()
            .cloned()
            .ok_or(ChannelAdapterError::Bound)?;
        for key in ["type", "request_id", "error"] {
            fields.remove(key);
        }
        return Ok(Some(AdapterRuntimeEvent::RuntimeError {
            request_id,
            error,
            fields: fields.into_iter().collect(),
        }));
    }
    match serde_json::from_value(value.clone()) {
        Ok(event) => Ok(Some(event)),
        Err(_) => Ok(None),
    }
}

fn validate_event_bound(event: &AdapterRuntimeEvent) -> Result<(), ChannelAdapterError> {
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

fn payload_client_message_id(payload: &serde_json::Value) -> Option<&str> {
    payload
        .get("client_message_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            payload
                .get("messages")?
                .as_array()?
                .first()?
                .get("client_message_id")?
                .as_str()
        })
}

fn validate_inbound(message: &InboundChannelMessage) -> Result<(), ChannelAdapterError> {
    if message.client_message_id.is_empty()
        || message.client_message_id.len() > crate::control_plane::CONTROL_ID_BYTES_MAX
        || !message.route.enabled
        || payload_client_message_id(&message.payload) != Some(message.client_message_id.as_str())
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

    #[derive(Default)]
    struct MultiFactory(StdMutex<Vec<(CanonicalRouteKey, ChannelRuntimeClient)>>);
    impl ChannelAdapterFactory for MultiFactory {
        fn start(
            &self,
            context: AdapterFactoryContext,
            runtime: ChannelRuntimeClient,
        ) -> Result<(), ChannelAdapterError> {
            self.0
                .lock()
                .map_err(|_| ChannelAdapterError::Unavailable)?
                .push((CanonicalRouteKey::from_route(&context.route), runtime));
            Ok(())
        }
    }

    fn second_route() -> ChannelRoute {
        serde_json::from_value(serde_json::json!({
            "channel_id":"telegram", "account_id":"secondary", "chat_id":"other-chat",
            "thread_id":"thread-2", "agent_id":"agent-local-a", "conversation_id":"default",
            "enabled":true, "outbound_enabled":true,
            "created_at":"2026-01-01T00:00:00Z", "updated_at":"2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn same_runtime_routes_keep_exact_inbound_event_and_outbound_identity() {
        let factory = Arc::new(MultiFactory::default());
        let mut context = HostContext::production();
        context
            .register_factory("telegram", factory.clone())
            .unwrap();
        let routes = [route(), second_route()];
        let mut hub = AdapterHub::compose(&routes, &context).unwrap();
        let clients = factory.0.lock().unwrap().clone();
        assert_eq!(clients.len(), 2, "one attachment per full route key");
        assert_eq!(
            clients[0].1.submit_inbound(InboundChannelMessage {
                route: routes[1].clone(),
                client_message_id: "spoof".into(),
                payload: serde_json::json!({"client_message_id":"spoof"}),
            }),
            Err(ChannelAdapterError::Bound),
            "an attachment cannot spoof another full route"
        );

        for (index, route) in routes.iter().enumerate() {
            let key = CanonicalRouteKey::from_route(route);
            let client = clients
                .iter()
                .find(|(candidate, _)| candidate == &key)
                .unwrap()
                .1
                .clone();
            let client_id = format!("client-{index}");
            client
                .submit_inbound(InboundChannelMessage {
                    route: route.clone(),
                    client_message_id: client_id.clone(),
                    payload: serde_json::json!({"client_message_id":client_id,"content":"hello"}),
                })
                .unwrap();
            let inbound = hub.receive_inbound().await.unwrap();
            assert_eq!(CanonicalRouteKey::from_route(&inbound.route), key);
            let request_id = format!("input-{index}");
            hub.record_input(request_id.clone(), route).unwrap();

            let delivery_task = tokio::spawn({
                let client = client.clone();
                let key = key.clone();
                async move {
                    let delivery = client.receive_outbound().await.unwrap();
                    assert_eq!(CanonicalRouteKey::from_route(&delivery.route), key);
                    delivery.complete(MessageChannelResult::Delivered);
                }
            });
            assert_eq!(
                hub.deliver(route, "call", "tool", "exact").await,
                MessageChannelResult::Delivered
            );
            delivery_task.await.unwrap();

            for event in [
                serde_json::json!({
                    "type":"input_accepted", "request_id":request_id, "accepted":true
                }),
                serde_json::json!({
                    "type":"update_loop_status", "request_id":request_id, "status":"idle"
                }),
                serde_json::json!({"type":"turn_finished","request_id":request_id}),
            ] {
                assert!(hub.accept_runtime_event(&event).unwrap());
            }
            assert!(matches!(
                client.receive_event().await,
                Some(AdapterRuntimeEvent::InputAccepted { .. })
            ));
            assert!(hub.active_route_key("agent-local-a", "default").is_none());
        }
    }

    #[tokio::test]
    async fn correlated_runtime_error_is_terminal_and_rejects_malformed_unmatched_duplicate() {
        let factory = Arc::new(FakeFactory::default());
        let mut context = HostContext::production();
        context
            .register_factory("telegram", factory.clone())
            .unwrap();
        let route = route();
        let mut hub = AdapterHub::compose(std::slice::from_ref(&route), &context).unwrap();
        let client = factory.0.lock().unwrap().clone().unwrap();
        hub.record_input("failed".into(), &route).unwrap();
        assert!(
            hub.accept_runtime_event(&serde_json::json!({
                "type":"input_accepted", "request_id":"failed", "accepted":false,
                "error":"runtime service unavailable"
            }))
            .unwrap()
        );
        assert!(matches!(
            client.receive_event().await,
            Some(AdapterRuntimeEvent::RuntimeError { request_id, .. }) if request_id == "failed"
        ));
        assert!(hub.active_route_key("agent-local-a", "default").is_none());
        for invalid in [
            serde_json::json!({"type":"input_accepted","request_id":"failed","accepted":false}),
            serde_json::json!({"type":"runtime_error","request_id":"unknown","error":"x"}),
            serde_json::json!({"type":"runtime_error","request_id":"failed","error":"x"}),
        ] {
            assert!(hub.accept_runtime_event(&invalid).is_err());
        }
    }
}
