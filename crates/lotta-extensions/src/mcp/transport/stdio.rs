use super::jsonrpc::{next_id, response_result};
use super::{
    Arc, AsyncRead, AsyncWrite, CancellationToken, Duration, Future, JsonRpcNotification,
    JsonRpcRequest, MCP_MESSAGE_BYTES_MAX, MCP_REQUEST_TIMEOUT, MCP_STDIO_REQUESTS_MAX, McpError,
    McpFuture, McpStdioFactoryPort, Pin, RestartLimitError, SIDECAR_CHILD_JOIN_TIMEOUT,
    SIDECAR_PROTOCOL_VERSION, SIDECAR_TIMEOUT_MS_MAX, SecretValue, SidecarCapability,
    SidecarEnvelope, SidecarEnvelopeKind, SidecarFrameLimit, SidecarOwnerIdentity,
    SidecarSessionPolicy, ValidatedSidecarReader, Value, await_mcp, fmt, write_frame,
};
/// Owned async pipe reader returned by a Task 42 child launcher.
pub type McpStdioReader = Box<dyn AsyncRead + Unpin + Send>;
/// Owned async pipe writer returned by a Task 42 child launcher.
pub type McpStdioWriter = Box<dyn AsyncWrite + Unpin + Send>;
/// Lifecycle future returned by one exact child handle.
pub type McpChildFuture<'a> = Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>>;
/// Exact stop/join capability for one launched Task 42 child.
pub trait McpStdioChild: Send {
    /// Requests stop for this exact child.
    fn stop(&mut self) -> McpChildFuture<'_>;
    /// Joins this exact child.
    fn join(&mut self) -> McpChildFuture<'_>;
}
/// Fully owned Task 42 child connection.
/// Redacted launch request carrying a just-in-time credential at the process boundary.
pub struct McpStdioLaunch<'a> {
    /// Validated non-secret process configuration.
    pub config: &'a crate::mcp::client::StdioConfig,
    /// Optional secret for exact child environment injection.
    pub credential: Option<SecretValue>,
}
impl fmt::Debug for McpStdioLaunch<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpStdioLaunch")
            .field("config", self.config)
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

/// Fully owned Task 42 child connection.
pub struct McpStdioChildConnection {
    /// Child stdout reader.
    pub reader: McpStdioReader,
    /// Child stdin writer.
    pub writer: McpStdioWriter,
    /// Exact lifecycle handle.
    pub child: Box<dyn McpStdioChild>,
    /// Exact runtime owner identity declared to the child.
    pub owner: SidecarOwnerIdentity,
}
/// Launch future for a Task 42 child.
pub type McpLaunchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<McpStdioChildConnection, McpError>> + Send + 'a>>;
/// Injected production capability that admits and launches an MCP stdio child.
pub trait McpStdioChildLauncher: Send + Sync {
    /// Performs Task 42 supervisor admission before launching the child.
    fn admit(&self) -> Result<(), RestartLimitError>;
    /// Launches one owned child after successful admission.
    fn launch(&self, request: McpStdioLaunch<'_>) -> McpLaunchFuture<'_>;
}

/// Production MCP stdio factory framed exclusively by the Task 42 codec.
pub struct Task42McpStdioFactory {
    launcher: Arc<dyn McpStdioChildLauncher>,
    timeout: Duration,
    join_timeout: Duration,
}
impl Task42McpStdioFactory {
    /// Binds an injected Task 42 child-launch capability.
    #[must_use]
    pub fn new(launcher: Arc<dyn McpStdioChildLauncher>) -> Self {
        Self {
            launcher,
            timeout: MCP_REQUEST_TIMEOUT,
            join_timeout: SIDECAR_CHILD_JOIN_TIMEOUT,
        }
    }
    /// Tightens host I/O and lifecycle waits for a deployment or deterministic test.
    #[must_use]
    pub const fn with_timeouts(mut self, timeout: Duration, join_timeout: Duration) -> Self {
        self.timeout = timeout;
        self.join_timeout = join_timeout;
        self
    }
}
impl McpStdioFactoryPort for Task42McpStdioFactory {
    fn launch<'a>(
        &'a self,
        config: &'a crate::mcp::client::StdioConfig,
        credential: Option<SecretValue>,
        cancellation: CancellationToken,
    ) -> McpFuture<'a, Arc<dyn McpStdioPort>> {
        Box::pin(async move {
            self.launcher.admit().map_err(|_| McpError::Unreachable)?;
            let connection = self
                .launcher
                .launch(McpStdioLaunch { config, credential })
                .await?;
            let policy = SidecarSessionPolicy::new(
                SIDECAR_PROTOCOL_VERSION,
                connection.owner.clone(),
                [SidecarCapability::Mcp],
                SIDECAR_TIMEOUT_MS_MAX,
                SidecarFrameLimit::bounded(MCP_MESSAGE_BYTES_MAX),
            )
            .with_deadline_ms(duration_ms(self.timeout));
            let mut reader = ValidatedSidecarReader::new(connection.reader, policy);
            let handshake = tokio::select! {
                biased;
                () = cancellation.cancelled() => Err(McpError::Cancelled),
                result = tokio::time::timeout(self.timeout, reader.accept_handshake()) =>
                    result.map_err(|_| McpError::Timeout)?.map_err(|_| McpError::Protocol),
            };
            if let Err(error) = handshake {
                cleanup_child(connection.child, self.join_timeout).await?;
                return Err(error);
            }
            Ok(Arc::new(Task42McpStdioSession::spawn(
                reader,
                connection.writer,
                connection.child,
                connection.owner,
                self.timeout,
                self.join_timeout,
            )) as Arc<dyn McpStdioPort>)
        })
    }
}

/// Exact local child request contract. Implementations must use Task 42 codec/profile.
pub trait McpStdioPort: Send + Sync {
    /// Sends one validated, correlation-aware request through the Task 42 framing session.
    fn request(
        &self,
        request: JsonRpcRequest,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, Value>;
    /// Sends one validated notification without reading a response.
    fn notify(
        &self,
        notification: JsonRpcNotification,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, ()>;
    /// Stops and joins the exact Task 42 supervised child.
    fn close(&self) -> McpFuture<'_, ()>;
}

enum StdioActorCommand {
    Request(
        JsonRpcRequest,
        tokio::sync::oneshot::Sender<Result<Value, McpError>>,
    ),
    Notify(
        JsonRpcNotification,
        tokio::sync::oneshot::Sender<Result<(), McpError>>,
    ),
}

struct Task42McpStdioSession {
    commands: tokio::sync::Mutex<Option<tokio::sync::mpsc::Sender<StdioActorCommand>>>,
    actor: tokio::sync::Mutex<Option<tokio::task::JoinHandle<Result<(), McpError>>>>,
    cancel: CancellationToken,
}
impl Task42McpStdioSession {
    fn spawn(
        reader: ValidatedSidecarReader<McpStdioReader>,
        writer: McpStdioWriter,
        child: Box<dyn McpStdioChild>,
        owner: SidecarOwnerIdentity,
        timeout: Duration,
        join_timeout: Duration,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(MCP_STDIO_REQUESTS_MAX);
        let cancel = CancellationToken::new();
        let actor_cancel = cancel.clone();
        let actor = tokio::spawn(stdio_actor(
            StdioActorResources {
                reader,
                writer,
                child,
                owner,
                timeout,
                join_timeout,
            },
            receiver,
            actor_cancel,
        ));
        Self {
            commands: tokio::sync::Mutex::new(Some(sender)),
            actor: tokio::sync::Mutex::new(Some(actor)),
            cancel,
        }
    }
    async fn sender(&self) -> Result<tokio::sync::mpsc::Sender<StdioActorCommand>, McpError> {
        self.commands.lock().await.clone().ok_or(McpError::Eof)
    }
}
impl McpStdioPort for Task42McpStdioSession {
    fn request(
        &self,
        request: JsonRpcRequest,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, Value> {
        Box::pin(async move {
            let sender = self.sender().await?;
            let (reply, response) = tokio::sync::oneshot::channel();
            await_mcp(&cancellation, async {
                sender
                    .send(StdioActorCommand::Request(request, reply))
                    .await
                    .map_err(|_| McpError::Eof)
            })
            .await?;
            await_mcp(&cancellation, async {
                response.await.map_err(|_| McpError::Eof)?
            })
            .await
        })
    }
    fn notify(
        &self,
        notification: JsonRpcNotification,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, ()> {
        Box::pin(async move {
            let sender = self.sender().await?;
            let (reply, response) = tokio::sync::oneshot::channel();
            await_mcp(&cancellation, async {
                sender
                    .send(StdioActorCommand::Notify(notification, reply))
                    .await
                    .map_err(|_| McpError::Eof)
            })
            .await?;
            await_mcp(&cancellation, async {
                response.await.map_err(|_| McpError::Eof)?
            })
            .await
        })
    }
    fn close(&self) -> McpFuture<'_, ()> {
        Box::pin(async move {
            self.commands.lock().await.take();
            self.cancel.cancel();
            if let Some(actor) = self.actor.lock().await.take() {
                actor.await.map_err(|_| McpError::Unreachable)??;
            }
            Ok(())
        })
    }
}
impl Drop for Task42McpStdioSession {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Ok(mut commands) = self.commands.try_lock() {
            commands.take();
        }
        if let Ok(mut actor) = self.actor.try_lock()
            && let Some(join) = actor.take()
        {
            join.abort();
        }
    }
}

struct StdioActorResources {
    reader: ValidatedSidecarReader<McpStdioReader>,
    writer: McpStdioWriter,
    child: Box<dyn McpStdioChild>,
    owner: SidecarOwnerIdentity,
    timeout: Duration,
    join_timeout: Duration,
}

async fn stdio_actor(
    resources: StdioActorResources,
    mut commands: tokio::sync::mpsc::Receiver<StdioActorCommand>,
    cancel: CancellationToken,
) -> Result<(), McpError> {
    let StdioActorResources {
        mut reader,
        mut writer,
        mut child,
        owner,
        timeout,
        join_timeout,
    } = resources;
    let mut sequence = 1_u64;
    let result = loop {
        let command = tokio::select! {
            biased;
            () = cancel.cancelled() => break Ok(()),
            command = commands.recv() => match command {
                Some(command) => command,
                None => break Ok(()),
            },
        };
        let request_id = sequence.to_string();
        sequence = sequence.checked_add(1).ok_or(McpError::Protocol)?;
        if let Err(error) = handle_stdio_command(
            command,
            &mut reader,
            &mut writer,
            &owner,
            timeout,
            request_id,
        )
        .await
        {
            break Err(error);
        }
    };
    finish_stdio_actor(result, &mut *child, join_timeout).await
}

async fn handle_stdio_command(
    command: StdioActorCommand,
    reader: &mut ValidatedSidecarReader<McpStdioReader>,
    writer: &mut McpStdioWriter,
    owner: &SidecarOwnerIdentity,
    timeout: Duration,
    request_id: String,
) -> Result<(), McpError> {
    match command {
        StdioActorCommand::Request(payload, reply) => {
            let envelope =
                mcp_envelope(owner.clone(), timeout, request_id, payload.value().clone());
            let response = async {
                write_frame(
                    writer,
                    SidecarFrameLimit::bounded(MCP_MESSAGE_BYTES_MAX),
                    &envelope,
                )
                .await
                .map_err(|_| McpError::Unreachable)?;
                let inbound = reader
                    .read_payload()
                    .await
                    .map_err(|_| McpError::Protocol)?;
                Ok(inbound.payload)
            };
            let value = tokio::time::timeout(timeout, response)
                .await
                .map_err(|_| McpError::Timeout)?;
            let terminal = value.is_err();
            let _ = reply.send(value);
            if terminal {
                return Err(McpError::Protocol);
            }
        }
        StdioActorCommand::Notify(payload, reply) => {
            let envelope =
                mcp_envelope(owner.clone(), timeout, request_id, payload.value().clone());
            let value = tokio::time::timeout(
                timeout,
                write_frame(
                    writer,
                    SidecarFrameLimit::bounded(MCP_MESSAGE_BYTES_MAX),
                    &envelope,
                ),
            )
            .await
            .map_err(|_| McpError::Timeout)?
            .map_err(|_| McpError::Unreachable);
            let terminal = value.is_err();
            let _ = reply.send(value);
            if terminal {
                return Err(McpError::Unreachable);
            }
        }
    }
    Ok(())
}

async fn finish_stdio_actor(
    result: Result<(), McpError>,
    child: &mut dyn McpStdioChild,
    join_timeout: Duration,
) -> Result<(), McpError> {
    result.and(stop_join_child(child, join_timeout).await)
}

fn mcp_envelope(
    owner: SidecarOwnerIdentity,
    timeout: Duration,
    request_id: String,
    payload: Value,
) -> SidecarEnvelope {
    SidecarEnvelope {
        version: SIDECAR_PROTOCOL_VERSION,
        owner,
        capability: SidecarCapability::Mcp,
        timeout_ms: duration_ms(timeout),
        request_id,
        correlation_id: None,
        kind: SidecarEnvelopeKind::Request,
        payload,
    }
}
fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
async fn stop_join_child(
    child: &mut dyn McpStdioChild,
    join_timeout: Duration,
) -> Result<(), McpError> {
    child.stop().await?;
    tokio::time::timeout(join_timeout, child.join())
        .await
        .map_err(|_| McpError::Timeout)??;
    Ok(())
}
async fn cleanup_child(
    mut child: Box<dyn McpStdioChild>,
    join_timeout: Duration,
) -> Result<(), McpError> {
    stop_join_child(&mut *child, join_timeout).await
}

/// Concrete stdio adapter over a Task 42 session port.
pub struct StdioTransport {
    pub(super) port: Arc<dyn McpStdioPort>,
    pub(super) sequence: std::sync::atomic::AtomicU64,
}
impl StdioTransport {
    /// Binds the exact supervised Task 42 child session.
    #[must_use]
    pub fn new(port: Arc<dyn McpStdioPort>) -> Self {
        Self {
            port,
            sequence: std::sync::atomic::AtomicU64::new(1),
        }
    }
    pub(super) fn request(
        &self,
        method: &str,
        params: Value,
        cancel: CancellationToken,
    ) -> McpFuture<'_, Value> {
        let id = next_id(&self.sequence);
        let method = method.to_owned();
        Box::pin(async move {
            let response = self
                .port
                .request(JsonRpcRequest::new(id, &method, params)?, cancel)
                .await?;
            response_result(response, id)
        })
    }
}
