//! Bounded in-memory boundary between future channel adapters and the runtime client.

use lotta_domain::ChannelRoute;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc, oneshot};

/// Maximum queued inbound or outbound adapter deliveries.
pub const CHANNEL_ADAPTER_DELIVERIES_MAX: usize = 256;
/// Maximum UTF-8 bytes in one normalized adapter message.
pub const CHANNEL_ADAPTER_MESSAGE_BYTES_MAX: usize = 16 * 1024 * 1024;

/// A normalized inbound adapter message submitted to the Runtime WebSocket owner.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InboundChannelMessage {
    /// Exact canonical route selected by the future adapter.
    pub route: ChannelRoute,
    /// Stable adapter-generated message identifier.
    pub client_message_id: String,
    /// Canonical bounded Runtime input payload.
    pub payload: serde_json::Value,
}

/// One outbound `MessageChannel` delivery sent to a future adapter.
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
    /// The normalized message exceeds its byte ceiling.
    #[error("channel message exceeds bounds")]
    Bound,
}

/// Cloneable client handed to future adapters.
#[derive(Clone)]
pub struct ChannelRuntimeClient {
    inbound: mpsc::Sender<InboundChannelMessage>,
    outbound: std::sync::Arc<Mutex<mpsc::Receiver<MessageChannelDelivery>>>,
}

impl ChannelRuntimeClient {
    /// Submits one inbound message without filesystem persistence or unbounded waiting.
    ///
    /// # Errors
    /// Returns unavailable, busy, or bound without admitting the message.
    pub fn submit_inbound(
        &self,
        message: InboundChannelMessage,
    ) -> Result<(), ChannelAdapterError> {
        validate_inbound(&message)?;
        self.inbound.try_send(message).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => ChannelAdapterError::Busy,
            mpsc::error::TrySendError::Closed(_) => ChannelAdapterError::Unavailable,
        })
    }

    /// Receives the next outbound `MessageChannel` delivery.
    pub async fn receive_outbound(&self) -> Option<MessageChannelDelivery> {
        self.outbound.lock().await.recv().await
    }
}

/// Child-owned end of the bounded adapter boundary.
pub struct ChannelAdapterPort {
    pub(crate) inbound: mpsc::Receiver<InboundChannelMessage>,
    pub(crate) outbound: mpsc::Sender<MessageChannelDelivery>,
}

/// Creates one bounded in-memory future-adapter boundary.
#[must_use]
pub fn channel_adapter_port() -> (ChannelRuntimeClient, ChannelAdapterPort) {
    let (inbound_sender, inbound) = mpsc::channel(CHANNEL_ADAPTER_DELIVERIES_MAX);
    let (outbound, outbound_receiver) = mpsc::channel(CHANNEL_ADAPTER_DELIVERIES_MAX);
    (
        ChannelRuntimeClient {
            inbound: inbound_sender,
            outbound: std::sync::Arc::new(Mutex::new(outbound_receiver)),
        },
        ChannelAdapterPort { inbound, outbound },
    )
}

pub(crate) fn delivery(
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
