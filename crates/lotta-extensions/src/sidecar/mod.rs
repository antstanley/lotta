//! Versioned, bounded sidecar transport, validation, and supervision.

/// The canonical length-prefixed JSON codec.
pub mod framing;
/// Mandatory first-frame handshake and validated inbound sessions.
pub mod handshake;
/// Restart-bounded transactional sidecar supervision.
pub mod supervisor;

#[cfg(test)]
mod framing_certificate;
#[cfg(test)]
mod handshake_certificate;
#[cfg(test)]
mod profile_certificate;
#[cfg(test)]
mod supervisor_certificate;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod validation_certificate;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Canonical mod-host frame ceiling from specification 05.
pub const MOD_HOST_MESSAGE_BYTES_MAX: usize = 8 * 1024 * 1024;
/// Canonical provider response-event ceiling from specification 06.
pub const PROVIDER_RESPONSE_EVENT_BYTES_MAX: usize = MOD_HOST_MESSAGE_BYTES_MAX;
/// Current authoritative Task 42 adapter wire protocol version.
pub const SIDECAR_PROTOCOL_VERSION: SidecarProtocolVersion = SidecarProtocolVersion(1);
/// Maximum accepted child-declared operation timeout.
pub const SIDECAR_TIMEOUT_MS_MAX: u64 = 300_000;

/// A supported sidecar wire protocol version.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SidecarProtocolVersion(pub u16);

/// Exact runtime ownership identity bound to a sidecar session.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct SidecarOwnerIdentity {
    agent_id: String,
    runtime_id: String,
    conversation_id: String,
}

impl SidecarOwnerIdentity {
    /// Builds an exact identity from bounded non-empty identifiers.
    pub fn new(
        agent_id: &str,
        runtime_id: &str,
        conversation_id: &str,
    ) -> Result<Self, IdentityError> {
        const ID_BYTES_MAX: usize = 256;
        for value in [agent_id, runtime_id, conversation_id] {
            if value.is_empty() || value.len() > ID_BYTES_MAX {
                return Err(IdentityError);
            }
        }
        Ok(Self {
            agent_id: agent_id.into(),
            runtime_id: runtime_id.into(),
            conversation_id: conversation_id.into(),
        })
    }
}
impl fmt::Debug for SidecarOwnerIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SidecarOwnerIdentity([redacted])")
    }
}
/// Stable identity validation error without identifier contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid sidecar owner identity")]
pub struct IdentityError;

/// Capability granted to one sidecar session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarCapability {
    /// JavaScript/TypeScript mod compatibility host.
    Mod,
    /// Provider compatibility host.
    Provider,
    /// Pinned subagent host.
    Subagent,
}
/// Envelope role on the wire.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarEnvelopeKind {
    /// Mandatory first-frame declaration.
    Hello,
    /// Host or child request.
    Request,
    /// Correlated response.
    Response,
    /// Unsolicited bounded event.
    Event,
}
/// One complete versioned sidecar message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SidecarEnvelope {
    /// Protocol version repeated on every frame.
    pub version: SidecarProtocolVersion,
    /// Exact host-owned session identity.
    pub owner: SidecarOwnerIdentity,
    /// Capability exercised by this frame.
    pub capability: SidecarCapability,
    /// Child-declared timeout, constrained by host policy.
    pub timeout_ms: u64,
    /// Stable request identifier.
    pub request_id: String,
    /// Optional request correlation identifier.
    pub correlation_id: Option<String>,
    /// Envelope role.
    pub kind: SidecarEnvelopeKind,
    /// Capability-specific JSON payload.
    pub payload: Value,
}

/// Selects a frame ceiling that can only tighten a canonical profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SidecarFrameLimit(usize);
impl SidecarFrameLimit {
    /// Constructs the mod-host profile.
    #[must_use]
    pub const fn mod_host() -> Self {
        Self(MOD_HOST_MESSAGE_BYTES_MAX)
    }
    /// Constructs the provider-host profile.
    #[must_use]
    pub const fn provider_host() -> Self {
        Self(PROVIDER_RESPONSE_EVENT_BYTES_MAX)
    }
    /// Constructs a caller-bounded generic profile for future sidecar adapters.
    ///
    /// Future consumers such as Task 46 must supply their own canonical bound.
    #[must_use]
    pub const fn bounded(bytes_max: usize) -> Self {
        Self(bytes_max)
    }
    /// Tightens this profile without permitting a higher caller-selected ceiling.
    #[must_use]
    pub const fn tightened(self, requested: usize) -> Self {
        Self(if requested < self.0 {
            requested
        } else {
            self.0
        })
    }
    /// Returns the effective byte ceiling.
    #[must_use]
    pub const fn bytes_max(self) -> usize {
        self.0
    }
}
