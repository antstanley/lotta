//! WebSocket introspection command group.
//!
//! Serves the pinned §WebSocket command groups Introspection row — the single
//! `app_server_info` command — with the exact response shape of the pinned
//! `letta-code/src/types/app-server-info.ts`: protocol version 1, this
//! server's backend and version, and the capability flag set. Capability
//! discovery is synchronous post-authentication: requests arriving on a
//! connection that has not completed WebSocket authentication are rejected
//! (logged and dropped) without a response or state mutation.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use serde::Serialize;

use crate::{
    error::AppServerError, errors::ProtocolErrorEnvelope, framing::DecodedFrame,
    ws::connection::ConnectionId,
};

/// Wire protocol version this server reports, pinned to the baseline's
/// `APP_SERVER_PROTOCOL_VERSION`.
pub const APP_SERVER_PROTOCOL_VERSION: u32 = 1;

/// Pinned `app_server_info` payload.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct AppServerInfoCommand {
    /// Response correlation identifier.
    pub request_id: String,
}

/// Typed capability flag; serializes exactly like a JSON boolean so the wire
/// shape stays pinned to the baseline's capability record. The newtype also
/// deliberately keeps raw `bool` fields out of [`ServerCapabilities`], which
/// would otherwise trip `clippy::struct_excessive_bools` (seven flags).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CapabilityFlag(pub bool);

/// Capability flags reported by `app_server_info_response`.
///
/// Each flag reflects actual implemented behavior audited against this
/// server's command groups and production runtime start path:
/// agent/conversation/memory management and runtime start are served;
/// runtime start ignores client workspace-sandbox extensions (the server
/// always runs its own prepared sandbox policy); external-tool updates are
/// applied per runtime through the Task 41 bridge; split channels are absent.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ServerCapabilities {
    /// Agent management command group is served.
    pub agent_management: CapabilityFlag,
    /// Conversation management command group is served.
    pub conversation_management: CapabilityFlag,
    /// Memory command group is served.
    pub memory_management: CapabilityFlag,
    /// Runtime start lifecycle is served.
    pub runtime_start: CapabilityFlag,
    /// Runtime start accepts workspace sandboxes (not implemented).
    pub runtime_workspace_sandbox: CapabilityFlag,
    /// Runtime external-tool updates are accepted.
    pub runtime_external_tools_update: CapabilityFlag,
    /// Split-channel surface is not served by this server.
    pub split_channels: CapabilityFlag,
}

/// Pinned `app_server_info_response` message.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename = "app_server_info_response")]
pub struct AppServerInfoResponseMessage {
    /// Command correlation identifier.
    pub request_id: String,
    /// Post-auth discovery has no domain failure variant.
    pub success: bool,
    /// Local backend selection; Lotta always runs the local backend.
    pub backend: String,
    /// This server's package version.
    pub letta_code_version: String,
    /// Wire protocol version compared against by clients.
    pub protocol_version: u32,
    /// Advertised capability flags.
    pub capabilities: ServerCapabilities,
}

/// Push callback delivering one outbound message to one connection.
pub type IntrospectionForwarder = Arc<
    dyn Fn(ConnectionId, AppServerInfoResponseMessage) -> Result<(), AppServerError> + Send + Sync,
>;

fn lock<T>(state: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
pub(crate) fn inert_forwarder() -> IntrospectionForwarder {
    Arc::new(|_, _| Ok(()))
}

/// Serves authenticated `app_server_info` capability discovery.
///
/// The listener registers every connection that completed WebSocket upgrade
/// authentication and unregisters it on close; only registered connections
/// receive answers.
pub struct IntrospectionBridge {
    backend: &'static str,
    version: &'static str,
    authenticated: Mutex<HashSet<ConnectionId>>,
    forward: IntrospectionForwarder,
}

impl IntrospectionBridge {
    /// Creates the bridge pushing responses through `forward`.
    #[must_use]
    pub fn new(forward: IntrospectionForwarder) -> Self {
        Self {
            backend: "local",
            version: env!("CARGO_PKG_VERSION"),
            authenticated: Mutex::new(HashSet::new()),
            forward,
        }
    }

    /// Marks one authenticated connection as eligible for answers.
    ///
    /// Called by the listener after its HTTP-upgrade authentication succeeds.
    pub fn register_authenticated(&self, connection: ConnectionId) {
        lock(&self.authenticated).insert(connection);
    }

    /// Drops one closed connection from the eligibility set.
    pub fn unregister(&self, connection: ConnectionId) {
        lock(&self.authenticated).remove(&connection);
    }

    /// Answers one `app_server_info` command when the connection is
    /// authenticated; otherwise rejects it silently without any response.
    pub fn handle(&self, connection: ConnectionId, command: &AppServerInfoCommand) -> bool {
        if !lock(&self.authenticated).contains(&connection) {
            tracing::warn!(
                request_id = %command.request_id,
                "unauthenticated app_server_info rejected"
            );
            return false;
        }
        let _ = (self.forward)(connection, self.response(&command.request_id));
        true
    }

    fn response(&self, request_id: &str) -> AppServerInfoResponseMessage {
        AppServerInfoResponseMessage {
            request_id: request_id.to_owned(),
            success: true,
            backend: self.backend.to_owned(),
            letta_code_version: self.version.to_owned(),
            protocol_version: APP_SERVER_PROTOCOL_VERSION,
            capabilities: ServerCapabilities {
                agent_management: CapabilityFlag(true),
                conversation_management: CapabilityFlag(true),
                memory_management: CapabilityFlag(true),
                runtime_start: CapabilityFlag(true),
                runtime_workspace_sandbox: CapabilityFlag(false),
                runtime_external_tools_update: CapabilityFlag(true),
                split_channels: CapabilityFlag(false),
            },
        }
    }
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for a malformed known
/// `app_server_info` command.
pub fn decode(frame: &DecodedFrame) -> Result<Option<AppServerInfoCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(Tag::AppServerInfo) = &frame.effects.outcome else {
        return Ok(None);
    };
    serde_json::from_value::<AppServerInfoCommand>(frame.value.clone())
        .map(Some)
        .map_err(|_| {
            ProtocolErrorEnvelope::new(
                "app_server_info_invalid",
                "invalid app_server_info command",
                frame.request_id.clone(),
            )
        })
}

#[cfg(test)]
#[path = "introspection_info_tests.rs"]
mod info;
