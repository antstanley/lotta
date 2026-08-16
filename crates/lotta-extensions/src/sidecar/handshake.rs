//! Mandatory Task 42 v1 handshake and production host-session dispatch.

use std::{collections::HashSet, future::Future};
use tokio::io::AsyncRead;

use super::framing::{FramingError, read_frame};
use super::{
    SidecarCapability, SidecarEnvelope, SidecarEnvelopeKind, SidecarFrameLimit,
    SidecarOwnerIdentity, SidecarProtocolVersion,
};

/// Typed terminal inbound-session failure.
#[derive(Debug, thiserror::Error)]
pub enum SidecarSessionError {
    /// Frame length was invalid.
    #[error("sidecar frame length rejected")]
    Length,
    /// Protocol version did not match.
    #[error("sidecar protocol version rejected")]
    Version,
    /// Owner identity did not match.
    #[error("sidecar owner rejected")]
    Owner,
    /// Capability was not granted.
    #[error("sidecar capability rejected")]
    Capability,
    /// Declared timeout violated host policy.
    #[error("sidecar timeout rejected")]
    Timeout,
    /// Handshake sequence was invalid.
    #[error("sidecar handshake rejected")]
    Handshake,
    /// JSON decoding failed.
    #[error("sidecar json rejected")]
    Json,
    /// Pipe I/O failed.
    #[error("sidecar io failed")]
    Io,
    /// Payload dispatch failed.
    #[error("sidecar payload dispatch failed")]
    Dispatch,
}

/// Host-owned validation policy fixed before reading child data.
#[derive(Clone, Debug)]
pub struct SidecarSessionPolicy {
    version: SidecarProtocolVersion,
    owner: SidecarOwnerIdentity,
    capabilities: HashSet<SidecarCapability>,
    timeout_ms_max: u64,
    deadline_ms: u64,
    frame_limit: SidecarFrameLimit,
}
impl SidecarSessionPolicy {
    /// Constructs a fixed policy with an already canonical frame profile.
    #[must_use]
    pub fn new(
        version: SidecarProtocolVersion,
        owner: SidecarOwnerIdentity,
        capabilities: impl IntoIterator<Item = SidecarCapability>,
        timeout_ms_max: u64,
        frame_limit: SidecarFrameLimit,
    ) -> Self {
        Self {
            version,
            owner,
            capabilities: capabilities.into_iter().collect(),
            timeout_ms_max,
            deadline_ms: timeout_ms_max,
            frame_limit,
        }
    }
    /// Tightens the remaining host deadline without ever widening static or prior bounds.
    #[must_use]
    pub const fn with_deadline_ms(mut self, deadline_ms: u64) -> Self {
        self.deadline_ms = if deadline_ms < self.deadline_ms {
            deadline_ms
        } else {
            self.deadline_ms
        };
        self
    }
}

/// Reader that permanently terminates after any framing or validation failure.
pub struct ValidatedSidecarReader<R> {
    reader: R,
    policy: SidecarSessionPolicy,
    hello: bool,
    terminal: bool,
}
impl<R: AsyncRead + Unpin> ValidatedSidecarReader<R> {
    /// Binds an async local pipe to an immutable host policy.
    #[must_use]
    pub const fn new(reader: R, policy: SidecarSessionPolicy) -> Self {
        Self {
            reader,
            policy,
            hello: false,
            terminal: false,
        }
    }
    /// Reads the mandatory hello and fixes the session identity and capabilities.
    pub async fn accept_handshake(&mut self) -> Result<(), SidecarSessionError> {
        let envelope = self.read_validated().await?;
        if envelope.kind != SidecarEnvelopeKind::Hello {
            return self.fail(SidecarSessionError::Handshake);
        }
        self.hello = true;
        Ok(())
    }
    /// Reads one post-handshake envelope after all ordered boundary checks.
    pub async fn read_payload(&mut self) -> Result<SidecarEnvelope, SidecarSessionError> {
        if !self.hello {
            return self.fail(SidecarSessionError::Handshake);
        }
        let envelope = self.read_validated().await?;
        if envelope.kind == SidecarEnvelopeKind::Hello {
            return self.fail(SidecarSessionError::Handshake);
        }
        Ok(envelope)
    }
    async fn read_validated(&mut self) -> Result<SidecarEnvelope, SidecarSessionError> {
        if self.terminal {
            return Err(SidecarSessionError::Handshake);
        }
        let envelope: SidecarEnvelope =
            match read_frame(&mut self.reader, self.policy.frame_limit).await {
                Ok(value) => value,
                Err(error) => return self.fail(map_framing(error)),
            };
        let error = if envelope.version != self.policy.version {
            Some(SidecarSessionError::Version)
        } else if envelope.owner != self.policy.owner {
            Some(SidecarSessionError::Owner)
        } else if !self.policy.capabilities.contains(&envelope.capability) {
            Some(SidecarSessionError::Capability)
        } else if envelope.timeout_ms == 0
            || envelope.timeout_ms > self.policy.timeout_ms_max
            || envelope.timeout_ms > self.policy.deadline_ms
        {
            Some(SidecarSessionError::Timeout)
        } else {
            None
        };
        if let Some(error) = error {
            self.fail(error)
        } else {
            Ok(envelope)
        }
    }
    fn fail<T>(&mut self, error: SidecarSessionError) -> Result<T, SidecarSessionError> {
        self.terminal = true;
        Err(error)
    }
}

/// Capability-specific production dispatch port for accepted payload envelopes.
pub trait SidecarDispatcher {
    /// Dispatcher future.
    type Dispatch<'a>: Future<Output = Result<(), SidecarSessionError>> + 'a
    where
        Self: 'a;

    /// Dispatches one fully validated post-handshake envelope.
    fn dispatch(&mut self, envelope: SidecarEnvelope) -> Self::Dispatch<'_>;
}

/// Production host session owning validation, dispatch, and the exact child lifecycle handle.
pub struct SidecarHostSession<R, D, C> {
    reader: ValidatedSidecarReader<R>,
    dispatcher: D,
    child: C,
}
impl<R, D, C> SidecarHostSession<R, D, C>
where
    R: AsyncRead + Unpin,
    D: SidecarDispatcher,
{
    /// Binds a validated reader, dispatcher port, and exact child lifecycle handle.
    #[must_use]
    pub const fn new(reader: ValidatedSidecarReader<R>, dispatcher: D, child: C) -> Self {
        Self {
            reader,
            dispatcher,
            child,
        }
    }

    /// Performs the mandatory handshake, then reads and dispatches exactly one payload.
    pub async fn run(&mut self) -> Result<(), SidecarSessionError> {
        self.reader.accept_handshake().await?;
        let payload = self.reader.read_payload().await?;
        self.dispatcher
            .dispatch(payload)
            .await
            .map_err(|_| SidecarSessionError::Dispatch)
    }

    /// Returns ownership of the child lifecycle handle.
    #[must_use]
    pub fn into_child(self) -> C {
        self.child
    }
}

fn map_framing(error: FramingError) -> SidecarSessionError {
    match error {
        FramingError::Length | FramingError::Allocation => SidecarSessionError::Length,
        FramingError::JsonDecode | FramingError::JsonEncode => SidecarSessionError::Json,
        FramingError::Truncated | FramingError::Io => SidecarSessionError::Io,
    }
}
