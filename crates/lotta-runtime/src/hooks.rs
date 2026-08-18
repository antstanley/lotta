//! Dependency-neutral hook wire types and firing capability.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::{future::Future, pin::Pin};

/// Future returned by a lifecycle operation.
pub type LifecycleFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;
/// Typed asynchronous lifecycle operation.
pub trait LifecycleOperation<T, E>: Send {
    /// Runs the production operation exactly once.
    fn run(self) -> LifecycleFuture<'static, T, E>;
}
impl<T, E, F, Fut> LifecycleOperation<T, E> for F
where
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
{
    fn run(self) -> LifecycleFuture<'static, T, E> {
        Box::pin(self())
    }
}
use tokio_util::sync::CancellationToken;

/// Maximum serialized hook payload admitted at the runtime boundary.
pub const HOOK_PAYLOAD_BYTES_MAX: usize = 1_048_576;
/// Maximum hook block reason size.
pub const HOOK_REASON_BYTES_MAX: usize = 16_384;
/// Maximum hook owner or identifier size.
pub const HOOK_ID_BYTES_MAX: usize = 256;

/// The exact eleven baseline hook events.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum HookEvent {
    /// Before a tool call.
    #[serde(rename = "PreToolUse")]
    PreToolUse,
    /// After a successful or admitted tool result.
    #[serde(rename = "PostToolUse")]
    PostToolUse,
    /// After executor infrastructure failure.
    #[serde(rename = "PostToolUseFailure")]
    PostToolUseFailure,
    /// At the permission decision boundary.
    #[serde(rename = "PermissionRequest")]
    PermissionRequest,
    /// When a user submits a prompt.
    #[serde(rename = "UserPromptSubmit")]
    UserPromptSubmit,
    /// When a notification is published.
    #[serde(rename = "Notification")]
    Notification,
    /// When an agent turn stops.
    #[serde(rename = "Stop")]
    Stop,
    /// When a subagent stops.
    #[serde(rename = "SubagentStop")]
    SubagentStop,
    /// Immediately before compaction.
    #[serde(rename = "PreCompact")]
    PreCompact,
    /// When a session begins or resumes.
    #[serde(rename = "SessionStart")]
    SessionStart,
    /// When a session ends.
    #[serde(rename = "SessionEnd")]
    SessionEnd,
}

impl HookEvent {
    /// All events in canonical load and dispatch order.
    pub const ALL: [Self; 11] = [
        Self::PreToolUse,
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::PermissionRequest,
        Self::UserPromptSubmit,
        Self::Notification,
        Self::Stop,
        Self::SubagentStop,
        Self::PreCompact,
        Self::SessionStart,
        Self::SessionEnd,
    ];
    /// Events supporting model-backed prompt hooks.
    pub const PROMPT_SUPPORTED: [Self; 7] = [
        Self::PreToolUse,
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::PermissionRequest,
        Self::UserPromptSubmit,
        Self::Stop,
        Self::SubagentStop,
    ];
    /// Events restricted to command hooks.
    pub const COMMAND_ONLY: [Self; 4] = [
        Self::Notification,
        Self::PreCompact,
        Self::SessionStart,
        Self::SessionEnd,
    ];
    /// Whether this event supports prompt hooks.
    #[must_use]
    pub fn supports_prompt(self) -> bool {
        Self::PROMPT_SUPPORTED.contains(&self)
    }
    /// Whether modifications are legal for this event.
    #[must_use]
    pub const fn permits_modification(self) -> bool {
        matches!(self, Self::PreToolUse | Self::PostToolUse)
    }
}

/// Validated bounded hook owner identity.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HookOwner(String);
/// Validated bounded hook identifier.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HookId(String);

macro_rules! hook_id_type {
    ($name:ident, $context:literal) => {
        impl $name {
            /// Validates a non-empty, NUL-free bounded identity.
            ///
            /// # Errors
            /// Returns [`HookContractError::Identity`] when invalid or overbound.
            pub fn new(value: String) -> Result<Self, HookContractError> {
                if value.is_empty() || value.len() > HOOK_ID_BYTES_MAX || value.contains('\0') {
                    return Err(HookContractError::Identity);
                }
                Ok(Self(value))
            }
            /// Borrows the validated identity.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.debug_tuple($context).field(&self.0).finish()
            }
        }
    };
}
hook_id_type!(HookOwner, "HookOwner");
hook_id_type!(HookId, "HookId");

/// Validated event payload retaining the pinned event discriminator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookPayload {
    event: HookEvent,
    value: Value,
}

impl HookPayload {
    /// Validates size, object shape, and exact `event_type` wire value.
    ///
    /// # Errors
    /// Returns [`HookContractError::Payload`] for malformed or overbound JSON.
    pub fn new(event: HookEvent, value: Value) -> Result<Self, HookContractError> {
        let bytes = serde_json::to_vec(&value).map_err(|_| HookContractError::Payload)?;
        if bytes.len() > HOOK_PAYLOAD_BYTES_MAX || !value.is_object() {
            return Err(HookContractError::Payload);
        }
        let wire = serde_json::to_value(event).map_err(|_| HookContractError::Payload)?;
        if value.get("event_type") != Some(&wire) {
            return Err(HookContractError::Payload);
        }
        Ok(Self { event, value })
    }
    /// Returns the event associated with the payload.
    #[must_use]
    pub const fn event(&self) -> HookEvent {
        self.event
    }
    /// Borrows the admitted JSON object.
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.value
    }
    /// Consumes the payload into JSON.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.value
    }
}

/// Typed hook pipeline decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HookOutcome {
    /// Continue unchanged.
    Allow,
    /// Stop dispatch and action with a bounded reason.
    Block(HookReason),
    /// Continue with an event-specific replacement payload.
    Modify(HookPayload),
}

/// Bounded block reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookReason(String);
impl HookReason {
    /// Validates a non-empty, NUL-free bounded reason.
    ///
    /// # Errors
    /// Returns [`HookContractError::Reason`] when invalid or overbound.
    pub fn new(value: String) -> Result<Self, HookContractError> {
        if value.is_empty() || value.len() > HOOK_REASON_BYTES_MAX || value.contains('\0') {
            return Err(HookContractError::Reason);
        }
        Ok(Self(value))
    }
    /// Borrows the reason.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable owner and hook-attributed failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookFailure {
    /// Failing owner.
    pub owner: HookOwner,
    /// Failing hook.
    pub hook_id: HookId,
    /// Stable scrubbed machine code.
    pub code: &'static str,
}

/// Result of firing an event through all matching hooks.
pub type HookFireResult = Result<HookOutcome, HookFailure>;
/// Object-safe future returned by hook runtimes without erasing attribution.
pub type HookFuture<'a> = Pin<Box<dyn Future<Output = HookFireResult> + Send + 'a>>;

/// Dependency-neutral asynchronous hook capability.
pub trait HookRuntime: Send + Sync {
    /// Fires one typed event in registry order.
    fn fire(&self, payload: HookPayload, cancellation: CancellationToken) -> HookFuture<'_>;
    /// Fires one typed event against a caller-captured immutable registry snapshot.
    ///
    /// Runtimes without snapshot support retain their normal dispatch behavior.
    fn fire_snapshot(
        &self,
        _snapshot_id: u64,
        payload: HookPayload,
        cancellation: CancellationToken,
    ) -> HookFuture<'_> {
        self.fire(payload, cancellation)
    }
}

macro_rules! lifecycle_method {
    ($name:ident, $event:ident) => {
        /// Fires this event exactly once at its orchestration boundary.
        ///
        /// # Errors
        /// Returns a stable owner and hook-attributed runtime failure.
        pub async fn $name(
            &self,
            payload: HookPayload,
            cancellation: CancellationToken,
        ) -> HookFireResult {
            self.fire(HookEvent::$event, payload, cancellation).await
        }
    };
}

/// Production hook composition boundary for tool pipeline events.
pub struct HookLifecycle<'a> {
    runtime: &'a dyn HookRuntime,
}
impl<'a> HookLifecycle<'a> {
    /// Attaches the sole hook capability used by an orchestration layer.
    #[must_use]
    pub const fn new(runtime: &'a dyn HookRuntime) -> Self {
        Self { runtime }
    }

    async fn fire(
        &self,
        event: HookEvent,
        payload: HookPayload,
        cancellation: CancellationToken,
    ) -> HookFireResult {
        if payload.event() != event {
            return Err(boundary_failure("event_mismatch"));
        }
        self.runtime.fire(payload, cancellation).await
    }

    lifecycle_method!(pre_tool_use, PreToolUse);
    lifecycle_method!(post_tool_use, PostToolUse);
    lifecycle_method!(post_tool_use_failure, PostToolUseFailure);
    lifecycle_method!(permission_request, PermissionRequest);
    lifecycle_method!(user_prompt_submit, UserPromptSubmit);
    lifecycle_method!(notification, Notification);
    lifecycle_method!(stop, Stop);
    lifecycle_method!(subagent_stop, SubagentStop);
    lifecycle_method!(pre_compact, PreCompact);
    lifecycle_method!(session_start, SessionStart);
    lifecycle_method!(session_end, SessionEnd);
}

/// Sole production operation boundary for non-tool lifecycle events.
pub struct HookLifecycleHost<'a> {
    runtime: &'a dyn HookRuntime,
    snapshot_id: Option<u64>,
}
impl<'a> HookLifecycleHost<'a> {
    /// Creates an operation boundary over the configured hook runtime.
    #[must_use]
    pub const fn new(runtime: &'a dyn HookRuntime) -> Self {
        Self {
            runtime,
            snapshot_id: None,
        }
    }
    /// Creates an operation boundary pinned to one caller-captured snapshot identity.
    #[must_use]
    pub const fn with_snapshot(runtime: &'a dyn HookRuntime, snapshot_id: u64) -> Self {
        Self {
            runtime,
            snapshot_id: Some(snapshot_id),
        }
    }
    /// Fires before accepting a user prompt operation.
    ///
    /// # Errors
    /// Returns a hook, block, or wrapped operation failure.
    pub async fn user_prompt_submit<T, E, O>(
        &self,
        payload: HookPayload,
        cancellation: CancellationToken,
        operation: O,
    ) -> Result<T, LifecycleError<E>>
    where
        O: LifecycleOperation<T, E>,
    {
        self.before(HookEvent::UserPromptSubmit, payload, cancellation)
            .await?;
        operation.run().await.map_err(LifecycleError::Operation)
    }
    /// Fires before emitting a notification.
    ///
    /// # Errors
    /// Returns a hook, block, or wrapped operation failure.
    pub async fn notification<T, E, O>(
        &self,
        payload: HookPayload,
        cancellation: CancellationToken,
        emitter: O,
    ) -> Result<T, LifecycleError<E>>
    where
        O: LifecycleOperation<T, E>,
    {
        self.before(HookEvent::Notification, payload, cancellation)
            .await?;
        emitter.run().await.map_err(LifecycleError::Operation)
    }
    /// Fires immediately before returning a completed agent turn.
    ///
    /// # Errors
    /// Returns a hook, block, or wrapped operation failure.
    pub async fn stop<T, E, O>(
        &self,
        payload: HookPayload,
        cancellation: CancellationToken,
        operation: O,
    ) -> Result<T, LifecycleError<E>>
    where
        O: LifecycleOperation<T, E>,
    {
        let value = operation.run().await.map_err(LifecycleError::Operation)?;
        self.before(HookEvent::Stop, payload, cancellation).await?;
        Ok(value)
    }
    /// Fires immediately before returning a completed subagent turn.
    ///
    /// # Errors
    /// Returns a hook, block, or wrapped operation failure.
    pub async fn subagent_stop<T, E, O>(
        &self,
        payload: HookPayload,
        cancellation: CancellationToken,
        operation: O,
    ) -> Result<T, LifecycleError<E>>
    where
        O: LifecycleOperation<T, E>,
    {
        let value = operation.run().await.map_err(LifecycleError::Operation)?;
        self.before(HookEvent::SubagentStop, payload, cancellation)
            .await?;
        Ok(value)
    }
    /// Fires before the compaction operation.
    ///
    /// # Errors
    /// Returns a hook, block, or wrapped operation failure.
    pub async fn pre_compact<T, E, O>(
        &self,
        payload: HookPayload,
        cancellation: CancellationToken,
        operation: O,
    ) -> Result<T, LifecycleError<E>>
    where
        O: LifecycleOperation<T, E>,
    {
        self.before(HookEvent::PreCompact, payload, cancellation)
            .await?;
        operation.run().await.map_err(LifecycleError::Operation)
    }
    /// Fires after session initialization succeeds.
    ///
    /// # Errors
    /// Returns a hook, block, or wrapped operation failure.
    pub async fn session_start<T, E, O>(
        &self,
        payload: HookPayload,
        cancellation: CancellationToken,
        operation: O,
    ) -> Result<T, LifecycleError<E>>
    where
        O: LifecycleOperation<T, E>,
    {
        let value = operation.run().await.map_err(LifecycleError::Operation)?;
        self.before(HookEvent::SessionStart, payload, cancellation)
            .await?;
        Ok(value)
    }
    /// Fires before session teardown.
    ///
    /// # Errors
    /// Returns a hook, block, or wrapped operation failure.
    pub async fn session_end<T, E, O>(
        &self,
        payload: HookPayload,
        cancellation: CancellationToken,
        operation: O,
    ) -> Result<T, LifecycleError<E>>
    where
        O: LifecycleOperation<T, E>,
    {
        self.before(HookEvent::SessionEnd, payload, cancellation)
            .await?;
        operation.run().await.map_err(LifecycleError::Operation)
    }
    async fn before<E>(
        &self,
        event: HookEvent,
        payload: HookPayload,
        cancellation: CancellationToken,
    ) -> Result<(), LifecycleError<E>> {
        if payload.event() != event {
            return Err(LifecycleError::Hook(boundary_failure("event_mismatch")));
        }
        let fired = if let Some(snapshot_id) = self.snapshot_id {
            self.runtime
                .fire_snapshot(snapshot_id, payload, cancellation)
                .await
        } else {
            self.runtime.fire(payload, cancellation).await
        };
        match fired {
            Ok(HookOutcome::Allow) => Ok(()),
            Ok(HookOutcome::Block(_)) => Err(LifecycleError::Blocked),
            Ok(HookOutcome::Modify(_)) => Err(LifecycleError::Hook(boundary_failure(
                "illegal_modification",
            ))),
            Err(error) => Err(LifecycleError::Hook(error)),
        }
    }
}

/// Failure returned by a lifecycle operation boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleError<E> {
    /// A hook failed with stable attribution.
    Hook(HookFailure),
    /// A hook blocked the operation.
    Blocked,
    /// The wrapped operation failed.
    Operation(E),
}

fn boundary_failure(code: &'static str) -> HookFailure {
    HookFailure {
        owner: HookOwner("runtime".into()),
        hook_id: HookId("lifecycle".into()),
        code,
    }
}

/// Explicit no-op capability for callers without hooks.
pub struct NoopHookRuntime;
impl HookRuntime for NoopHookRuntime {
    fn fire(&self, _: HookPayload, _: CancellationToken) -> HookFuture<'_> {
        Box::pin(async { Ok(HookOutcome::Allow) })
    }
}

/// Hook contract validation failure without retained external values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum HookContractError {
    /// Owner or hook identity was invalid.
    #[error("invalid hook identity")]
    Identity,
    /// Payload was malformed or overbound.
    #[error("invalid hook payload")]
    Payload,
    /// Block reason was malformed or overbound.
    #[error("invalid hook reason")]
    Reason,
    /// Modification is illegal for the event or schema.
    #[error("illegal hook modification")]
    Modification,
}
