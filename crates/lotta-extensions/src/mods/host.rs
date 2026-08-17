use super::capabilities::{CapabilityBrokerTable, CapabilityCall};
use super::protocol::{
    JSON_RPC_VERSION, MOD_HOST_CALL_TIMEOUT_MS_DEFAULT, MOD_HOST_QUEUE_ITEMS_MAX, RpcError, RpcId,
    RpcMethod, RpcParams, RpcRequest, RpcResponse, RpcResult,
};
use super::types::{ModError, ModOwner};
use crate::sidecar::framing::write_frame;
use crate::sidecar::handshake::{SidecarSessionPolicy, ValidatedSidecarReader};
use crate::sidecar::{
    SIDECAR_PROTOCOL_VERSION, SIDECAR_TIMEOUT_MS_MAX, SidecarCapability, SidecarEnvelope,
    SidecarEnvelopeKind, SidecarFrameLimit, SidecarOwnerIdentity,
};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{fmt, future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Bounded wait for the distinct cancel and original responses after an interrupt.
pub const MOD_HOST_CANCEL_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Fixed deadline covering child stop and join together, and the actor join on shutdown.
pub const MOD_HOST_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum inbound child frames accepted while one host call is outstanding.
pub const MOD_HOST_INBOUND_ITEMS_MAX: usize = 128;

/// Future returned by one compatibility child call.
pub type HostFuture<'a> = Pin<Box<dyn Future<Output = Result<RpcResult, ModError>> + Send + 'a>>;

/// Callable mod host used by tools, commands, hooks, and lifecycle control.
pub trait ModHost: Send + Sync {
    /// Executes one strict JSON-RPC method for one owner generation.
    fn call(
        &self,
        owner: &ModOwner,
        method: RpcMethod,
        params: RpcParams,
        cancellation: CancellationToken,
    ) -> HostFuture<'_>;
    /// Requests graceful disposal, then stops and joins the owned child.
    fn dispose(&self) -> HostFuture<'_>;
    /// Immediately aborts and joins the owned child.
    fn abort(&self) -> HostFuture<'_>;
}

/// Bounded synchronous last-resort child termination.
///
/// A kill switch never waits: it signals the owned process group and returns. `Drop` uses it as the
/// backstop for a host that was never disposed, because a blocking join in `Drop` is unbounded.
#[derive(Clone)]
pub struct KillSwitch(Arc<dyn Fn() + Send + Sync>);
impl KillSwitch {
    /// Wraps one non-blocking termination effect.
    #[must_use]
    pub fn new(kill: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(kill))
    }
    /// Builds a switch for a child that owns no external process.
    #[must_use]
    pub fn inert() -> Self {
        Self(Arc::new(|| {}))
    }
    /// Signals the owned process group without waiting.
    pub fn kill(&self) {
        (self.0)();
    }
}
impl fmt::Debug for KillSwitch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KillSwitch")
    }
}

/// Exact compatibility child transport launched by the composition root.
pub trait ModHostChild: Send {
    /// Child read pipe.
    type Reader: AsyncRead + Unpin + Send;
    /// Child write pipe.
    type Writer: AsyncWrite + Unpin + Send;
    /// Stop future.
    type Stop<'a>: Future<Output = Result<(), ModError>> + Send + 'a
    where
        Self: 'a;
    /// Join future.
    type Join<'a>: Future<Output = Result<(), ModError>> + Send + 'a
    where
        Self: 'a;
    /// Splits off the exact local pipes.
    fn take_pipes(&mut self) -> Result<(Self::Reader, Self::Writer), ModError>;
    /// Stops this exact child.
    fn stop(&mut self) -> Self::Stop<'_>;
    /// Joins this exact child.
    fn join(&mut self) -> Self::Join<'_>;
    /// Returns the bounded non-blocking terminator for this exact child.
    fn kill_switch(&self) -> KillSwitch;
}

/// Injected capability for launching the pinned external TypeScript host.
pub trait ModHostLauncher: Send {
    /// Exact child type.
    type Child: ModHostChild;
    /// Launch future.
    type Launch<'a>: Future<Output = Result<Self::Child, ModError>> + Send + 'a
    where
        Self: 'a;
    /// Launches only after supervisor admission by the controller.
    fn launch(&mut self) -> Self::Launch<'_>;
}

enum ActorCommand {
    Call {
        method: RpcMethod,
        params: RpcParams,
        cancellation: CancellationToken,
        reply: oneshot::Sender<Result<RpcResult, ModError>>,
    },
    Dispose {
        graceful: bool,
        reply: oneshot::Sender<Result<RpcResult, ModError>>,
    },
}

/// Bounded owned RPC actor over Task 42's only codec and validated reader.
pub struct FramedModHost<C: ModHostChild> {
    owner: ModOwner,
    sender: mpsc::Sender<ActorCommand>,
    actor: std::sync::Mutex<Option<JoinHandle<()>>>,
    kill: KillSwitch,
    disposed: AtomicBool,
    _child: std::marker::PhantomData<C>,
}
impl<C: ModHostChild + 'static> FramedModHost<C> {
    /// Accepts the mandatory v1 hello before returning a callable host.
    pub async fn accept(
        child: C,
        owner: ModOwner,
        sidecar_owner: SidecarOwnerIdentity,
        timeout: Duration,
    ) -> Result<Arc<Self>, ModError> {
        let brokers = Arc::new(CapabilityBrokerTable::new());
        Self::accept_with_brokers(child, owner, sidecar_owner, timeout, brokers).await
    }

    /// Accepts a host whose actor resolves inbound capability calls through the shared table.
    ///
    /// The controller owns the table and mints the handle before this call, so the child can only
    /// reach the runtime scope its own handle resolves to.
    pub async fn accept_with_brokers(
        mut child: C,
        owner: ModOwner,
        sidecar_owner: SidecarOwnerIdentity,
        timeout: Duration,
        brokers: Arc<CapabilityBrokerTable>,
    ) -> Result<Arc<Self>, ModError> {
        let kill = child.kill_switch();
        let (reader, writer) = child.take_pipes()?;
        let policy = SidecarSessionPolicy::new(
            SIDECAR_PROTOCOL_VERSION,
            sidecar_owner.clone(),
            [SidecarCapability::Mod],
            SIDECAR_TIMEOUT_MS_MAX,
            SidecarFrameLimit::mod_host(),
        );
        let mut reader = ValidatedSidecarReader::new(reader, policy);
        if !matches!(
            tokio::time::timeout(timeout, reader.accept_handshake()).await,
            Ok(Ok(()))
        ) {
            cleanup_child(&mut child, MOD_HOST_SHUTDOWN_TIMEOUT).await;
            return Err(ModError::Protocol);
        }
        let (sender, receiver) = mpsc::channel(MOD_HOST_QUEUE_ITEMS_MAX);
        let context = ActorContext::new(owner.clone(), sidecar_owner, timeout, brokers);
        let actor = tokio::spawn(run_actor(receiver, reader, writer, child, context));
        Ok(Arc::new(Self {
            owner,
            sender,
            actor: std::sync::Mutex::new(Some(actor)),
            kill,
            disposed: AtomicBool::new(false),
            _child: std::marker::PhantomData,
        }))
    }

    async fn enqueue_call(
        &self,
        owner: &ModOwner,
        method: RpcMethod,
        params: RpcParams,
        cancellation: CancellationToken,
    ) -> Result<RpcResult, ModError> {
        if owner != &self.owner {
            return Err(ModError::InvalidScope);
        }
        let (reply, response) = oneshot::channel();
        self.sender
            .try_send(ActorCommand::Call {
                method,
                params,
                cancellation,
                reply,
            })
            .map_err(|_| ModError::Unavailable)?;
        response.await.map_err(|_| ModError::Unavailable)?
    }

    async fn shutdown(&self, graceful: bool) -> Result<RpcResult, ModError> {
        let (reply, response) = oneshot::channel();
        let dispatched = self
            .sender
            .send(ActorCommand::Dispose { graceful, reply })
            .await
            .is_ok();
        // A terminal actor has already stopped and joined its child, so a refused command is a
        // completed disposal rather than a failure to report to the controller.
        let result = if dispatched {
            response.await.unwrap_or(Ok(RpcResult::Disposed))
        } else {
            Ok(RpcResult::Disposed)
        };
        let actor = self.actor.lock().map_err(|_| ModError::Unavailable)?.take();
        if let Some(mut actor) = actor
            && tokio::time::timeout(MOD_HOST_SHUTDOWN_TIMEOUT, &mut actor)
                .await
                .is_err()
        {
            actor.abort();
            self.kill.kill();
        }
        self.disposed.store(true, Ordering::Release);
        result
    }
}

impl<C: ModHostChild + Sync + 'static> ModHost for FramedModHost<C> {
    fn call(
        &self,
        owner: &ModOwner,
        method: RpcMethod,
        params: RpcParams,
        cancellation: CancellationToken,
    ) -> HostFuture<'_> {
        let owner = owner.clone();
        Box::pin(async move {
            self.enqueue_call(&owner, method, params, cancellation)
                .await
        })
    }
    fn dispose(&self) -> HostFuture<'_> {
        Box::pin(async move { self.shutdown(true).await })
    }
    fn abort(&self) -> HostFuture<'_> {
        Box::pin(async move { self.shutdown(false).await })
    }
}

impl<C: ModHostChild> Drop for FramedModHost<C> {
    /// Bounded backstop for a host dropped without disposal: abort the actor, signal the group.
    ///
    /// Blocking here would be unbounded and can deadlock a caller already inside a runtime, so a
    /// dropped host never waits. A disposed host has already joined its child and skips the signal.
    fn drop(&mut self) {
        if let Ok(mut guard) = self.actor.lock()
            && let Some(actor) = guard.take()
        {
            actor.abort();
        }
        if !self.disposed.load(Ordering::Acquire) {
            self.kill.kill();
        }
    }
}

struct ActorContext {
    owner: ModOwner,
    sidecar_owner: SidecarOwnerIdentity,
    next_id: u64,
    timeout: Duration,
    brokers: Arc<CapabilityBrokerTable>,
}
impl ActorContext {
    fn new(
        owner: ModOwner,
        sidecar_owner: SidecarOwnerIdentity,
        timeout: Duration,
        brokers: Arc<CapabilityBrokerTable>,
    ) -> Self {
        Self {
            owner,
            sidecar_owner,
            next_id: 1,
            timeout,
            brokers,
        }
    }
    fn next_request_id(&mut self) -> Result<RpcId, ModError> {
        let id = RpcId::new(self.next_id)?;
        self.next_id = self.next_id.checked_add(1).ok_or(ModError::Unavailable)?;
        Ok(id)
    }
    fn timeout_ms(&self) -> u64 {
        u64::try_from(self.timeout.as_millis())
            .unwrap_or(u64::MAX)
            .min(MOD_HOST_CALL_TIMEOUT_MS_DEFAULT)
    }
}

/// One completed actor call and whether the session may continue.
struct CallOutcome {
    result: Result<RpcResult, ModError>,
    terminal: bool,
}
impl CallOutcome {
    const fn value(result: RpcResult) -> Self {
        Self {
            result: Ok(result),
            terminal: false,
        }
    }
    const fn survivable(error: ModError) -> Self {
        Self {
            result: Err(error),
            terminal: false,
        }
    }
    const fn terminal(error: ModError) -> Self {
        Self {
            result: Err(error),
            terminal: true,
        }
    }
}

async fn run_actor<C: ModHostChild>(
    mut receiver: mpsc::Receiver<ActorCommand>,
    mut reader: ValidatedSidecarReader<C::Reader>,
    mut writer: C::Writer,
    mut child: C,
    mut context: ActorContext,
) {
    while let Some(command) = receiver.recv().await {
        match command {
            ActorCommand::Call {
                method,
                params,
                cancellation,
                reply,
            } => {
                let outcome = actor_call(
                    &mut reader,
                    &mut writer,
                    &mut context,
                    method,
                    params,
                    cancellation,
                )
                .await;
                let terminal = outcome.terminal;
                let _ = reply.send(outcome.result);
                if terminal {
                    break;
                }
            }
            ActorCommand::Dispose { graceful, reply } => {
                if graceful {
                    let params = RpcParams::Dispose {
                        owner: context.owner.clone(),
                    };
                    let cancellation = CancellationToken::new();
                    let method = RpcMethod::Dispose;
                    let _ = actor_call(
                        &mut reader,
                        &mut writer,
                        &mut context,
                        method,
                        params,
                        cancellation,
                    )
                    .await;
                }
                cleanup_child(&mut child, MOD_HOST_SHUTDOWN_TIMEOUT).await;
                let _ = reply.send(Ok(RpcResult::Disposed));
                reject_queue(&mut receiver).await;
                return;
            }
        }
    }
    cleanup_child(&mut child, MOD_HOST_SHUTDOWN_TIMEOUT).await;
    reject_queue(&mut receiver).await;
}

/// Closes the command queue and fails every already-queued call for a dead session.
async fn reject_queue(receiver: &mut mpsc::Receiver<ActorCommand>) {
    receiver.close();
    while let Some(command) = receiver.recv().await {
        match command {
            ActorCommand::Call { reply, .. } => {
                let _ = reply.send(Err(ModError::Unavailable));
            }
            ActorCommand::Dispose { reply, .. } => {
                let _ = reply.send(Ok(RpcResult::Disposed));
            }
        }
    }
}

async fn actor_call<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut ValidatedSidecarReader<R>,
    writer: &mut W,
    context: &mut ActorContext,
    method: RpcMethod,
    params: RpcParams,
    cancellation: CancellationToken,
) -> CallOutcome {
    let id = match context.next_request_id() {
        Ok(id) => id,
        Err(error) => return CallOutcome::terminal(error),
    };
    let request = RpcRequest::new(id, method, params);
    if let Err(error) = request.validate() {
        return CallOutcome::terminal(error);
    }
    let timeout_ms = context.timeout_ms();
    let envelope = request_envelope(&context.sidecar_owner, id, &request, timeout_ms);
    if write_frame(writer, SidecarFrameLimit::mod_host(), &envelope)
        .await
        .is_err()
    {
        return CallOutcome::terminal(ModError::Protocol);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let interrupted = tokio::select! {
        biased;
        () = cancellation.cancelled() => ModError::Cancelled,
        () = tokio::time::sleep_until(deadline) => ModError::Timeout,
        first = reader.read_payload() => {
            let Ok(first) = first else {
                return CallOutcome::terminal(ModError::Protocol);
            };
            return actor_receive(reader, writer, context, id, first, deadline).await;
        }
    };
    actor_interrupt(reader, writer, context, id, interrupted).await
}

/// Consumes inbound child requests until the correlated response for `id` arrives.
async fn actor_receive<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut ValidatedSidecarReader<R>,
    writer: &mut W,
    context: &mut ActorContext,
    id: RpcId,
    first: SidecarEnvelope,
    deadline: tokio::time::Instant,
) -> CallOutcome {
    let mut envelope = first;
    for _ in 0..MOD_HOST_INBOUND_ITEMS_MAX {
        if envelope.kind != SidecarEnvelopeKind::Request {
            if !correlates(&envelope, id) {
                return CallOutcome::terminal(ModError::Protocol);
            }
            return match validate_response_envelope(
                &context.owner,
                &context.sidecar_owner,
                id,
                envelope,
            ) {
                Ok(result) => CallOutcome::value(result),
                Err(remote @ ModError::Remote { .. }) => CallOutcome::survivable(remote),
                Err(error) => CallOutcome::terminal(error),
            };
        }
        if let Err(error) = dispatch_inbound(writer, context, envelope).await {
            return CallOutcome::terminal(error);
        }
        envelope = match tokio::time::timeout_at(deadline, reader.read_payload()).await {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => return CallOutcome::terminal(ModError::Protocol),
            Err(_) => return CallOutcome::terminal(ModError::Timeout),
        };
    }
    CallOutcome::terminal(ModError::Protocol)
}

/// Cancels one outstanding request, then settles both responses within a fixed bound.
async fn actor_interrupt<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut ValidatedSidecarReader<R>,
    writer: &mut W,
    context: &mut ActorContext,
    original: RpcId,
    interrupted: ModError,
) -> CallOutcome {
    let cancel_id = match context.next_request_id() {
        Ok(id) => id,
        Err(error) => return CallOutcome::terminal(error),
    };
    if send_cancel(writer, context, cancel_id, original)
        .await
        .is_err()
    {
        return CallOutcome::terminal(ModError::Protocol);
    }
    match settle_interrupt(reader, writer, context, original, cancel_id).await {
        // A child that answered both the cancel and the original request is still coherent, so a
        // cancelled call leaves the session usable. A timeout never proves the child recovered.
        Ok(()) if interrupted == ModError::Cancelled => {
            CallOutcome::survivable(ModError::Cancelled)
        }
        // An expired drain is reported as the reason the call was interrupted, not as the drain.
        Ok(()) | Err(ModError::Timeout) => CallOutcome::terminal(interrupted),
        Err(error) => CallOutcome::terminal(error),
    }
}

/// Waits boundedly for the distinct cancel and original responses, dispatching inbound meanwhile.
async fn settle_interrupt<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut ValidatedSidecarReader<R>,
    writer: &mut W,
    context: &mut ActorContext,
    original: RpcId,
    cancel_id: RpcId,
) -> Result<(), ModError> {
    let deadline = tokio::time::Instant::now() + MOD_HOST_CANCEL_DRAIN_TIMEOUT;
    let mut settled_original = false;
    let mut settled_cancel = false;
    for _ in 0..MOD_HOST_INBOUND_ITEMS_MAX {
        if settled_original && settled_cancel {
            return Ok(());
        }
        let envelope = match tokio::time::timeout_at(deadline, reader.read_payload()).await {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => return Err(ModError::Protocol),
            Err(_) => return Err(ModError::Timeout),
        };
        if envelope.kind == SidecarEnvelopeKind::Request {
            dispatch_inbound(writer, context, envelope).await?;
            continue;
        }
        let id = if correlates(&envelope, original) {
            settled_original = true;
            original
        } else if correlates(&envelope, cancel_id) {
            settled_cancel = true;
            cancel_id
        } else {
            return Err(ModError::Protocol);
        };
        match validate_response_envelope(&context.owner, &context.sidecar_owner, id, envelope) {
            Ok(_) | Err(ModError::Remote { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    if settled_original && settled_cancel {
        Ok(())
    } else {
        Err(ModError::Protocol)
    }
}

async fn dispatch_inbound<W: AsyncWrite + Unpin>(
    writer: &mut W,
    context: &ActorContext,
    envelope: SidecarEnvelope,
) -> Result<(), ModError> {
    if envelope.owner != context.sidecar_owner || envelope.capability != SidecarCapability::Mod {
        return Err(ModError::Protocol);
    }
    let timeout_ms = envelope.timeout_ms.min(MOD_HOST_CALL_TIMEOUT_MS_DEFAULT);
    let request: RpcRequest =
        serde_json::from_value(envelope.payload).map_err(|_| ModError::Protocol)?;
    request.validate()?;
    let id = request.id;
    let outcome = authorize_inbound(context, request, timeout_ms).await;
    let response = match outcome {
        Ok(result) => RpcResponse {
            jsonrpc: JSON_RPC_VERSION.into(),
            id,
            result: Some(result),
            error: None,
        },
        Err(error) => RpcResponse {
            jsonrpc: JSON_RPC_VERSION.into(),
            id,
            result: None,
            error: Some(RpcError {
                code: inbound_error_code(&error),
                message: "capability refused".into(),
            }),
        },
    };
    let response_envelope = SidecarEnvelope {
        version: SIDECAR_PROTOCOL_VERSION,
        owner: context.sidecar_owner.clone(),
        capability: SidecarCapability::Mod,
        timeout_ms: timeout_ms.max(1),
        request_id: format!("response-{}", id.value()),
        correlation_id: Some(id.value().to_string()),
        kind: SidecarEnvelopeKind::Response,
        payload: serde_json::to_value(response).map_err(|_| ModError::Protocol)?,
    };
    write_frame(writer, SidecarFrameLimit::mod_host(), &response_envelope)
        .await
        .map_err(|_| ModError::Protocol)
}

/// Resolves one inbound child request through the shared broker table under a local bound.
async fn authorize_inbound(
    context: &ActorContext,
    request: RpcRequest,
    timeout_ms: u64,
) -> Result<RpcResult, ModError> {
    let RpcParams::CapabilityCall {
        owner,
        conversation_handle,
        capability,
        operation,
        params,
    } = request.params
    else {
        return Err(ModError::Protocol);
    };
    if request.method != RpcMethod::CapabilityCall {
        return Err(ModError::Protocol);
    }
    if owner != context.owner {
        return Err(ModError::InvalidScope);
    }
    let call = CapabilityCall {
        owner,
        handle: conversation_handle,
        capability,
        operation,
        params,
        cancellation: CancellationToken::new(),
    };
    let bound = Duration::from_millis(timeout_ms.max(1));
    match tokio::time::timeout(bound, context.brokers.call(call)).await {
        Ok(value) => value.map(|value| RpcResult::Value { value }),
        Err(_) => Err(ModError::Timeout),
    }
}

fn inbound_error_code(error: &ModError) -> i32 {
    match error {
        ModError::UndeclaredCapability => -32010,
        ModError::InvalidScope => -32011,
        ModError::Timeout => -32012,
        ModError::Cancelled => -32013,
        _ => -32000,
    }
}

fn correlates(envelope: &SidecarEnvelope, id: RpcId) -> bool {
    envelope.correlation_id.as_deref() == Some(id.value().to_string().as_str())
}

async fn send_cancel<W: AsyncWrite + Unpin>(
    writer: &mut W,
    context: &ActorContext,
    cancel_id: RpcId,
    original: RpcId,
) -> Result<(), ModError> {
    let request = RpcRequest::new(
        cancel_id,
        RpcMethod::Cancel,
        RpcParams::Cancel {
            owner: context.owner.clone(),
            request_id: original,
        },
    );
    let timeout_ms = context.timeout_ms();
    let envelope = request_envelope(&context.sidecar_owner, cancel_id, &request, timeout_ms);
    write_frame(writer, SidecarFrameLimit::mod_host(), &envelope)
        .await
        .map_err(|_| ModError::Protocol)
}

fn request_envelope(
    owner: &SidecarOwnerIdentity,
    id: RpcId,
    request: &RpcRequest,
    timeout_ms: u64,
) -> SidecarEnvelope {
    SidecarEnvelope {
        version: SIDECAR_PROTOCOL_VERSION,
        owner: owner.clone(),
        capability: SidecarCapability::Mod,
        timeout_ms,
        request_id: id.value().to_string(),
        correlation_id: None,
        kind: SidecarEnvelopeKind::Request,
        payload: serde_json::to_value(request).unwrap_or(Value::Null),
    }
}

fn validate_response_envelope(
    owner: &ModOwner,
    sidecar_owner: &SidecarOwnerIdentity,
    id: RpcId,
    envelope: SidecarEnvelope,
) -> Result<RpcResult, ModError> {
    if envelope.owner != *sidecar_owner
        || envelope.kind != SidecarEnvelopeKind::Response
        || !correlates(&envelope, id)
    {
        return Err(ModError::Protocol);
    }
    let response: RpcResponse =
        serde_json::from_value(envelope.payload).map_err(|_| ModError::Protocol)?;
    response.validate()?;
    if response.id != id || response.jsonrpc != JSON_RPC_VERSION {
        return Err(ModError::Protocol);
    }
    if let Some(result) = response.result {
        Ok(result)
    } else if let Some(error) = response.error {
        Err(ModError::Remote {
            owner: owner.id.clone(),
            code: error.code,
        })
    } else {
        Err(ModError::Protocol)
    }
}

/// Stops and joins one child under a single fixed deadline, then kills what outlived it.
async fn cleanup_child<C: ModHostChild>(child: &mut C, budget: Duration) {
    let deadline = tokio::time::Instant::now() + budget;
    let stopped = tokio::time::timeout_at(deadline, child.stop()).await;
    let joined = tokio::time::timeout_at(deadline, child.join()).await;
    let settled = matches!(stopped, Ok(Ok(()))) && matches!(joined, Ok(Ok(())));
    if !settled {
        child.kill_switch().kill();
    }
}
