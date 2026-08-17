use super::jsonrpc::{encode_notification, encode_request, next_id};
use super::router::{ResponseReceiver, ResponseRouter, RouterCommand};
use super::sse_codec::{
    SseParser, authenticated_request, endpoint_url, json_headers, validate_origin,
    validate_response,
};
use super::{
    Arc, BTreeMap, CancellationToken, CredentialRef, CredentialStorePort, HttpMethod,
    MCP_PENDING_REQUESTS_MAX, McpError, McpFuture, McpHttpPort, McpSseStream, Url, Value,
    await_mcp, validate_url,
};

/// Legacy SSE concrete adapter.
pub struct SseTransport {
    http: Arc<dyn McpHttpPort>,
    initial_url: Url,
    credential: Option<CredentialRef>,
    store: Arc<dyn CredentialStorePort>,
    sequence: std::sync::atomic::AtomicU64,
    actor: tokio::sync::Mutex<Option<SseActorHandle>>,
}
struct SseActorHandle {
    endpoint: Url,
    commands: tokio::sync::mpsc::Sender<RouterCommand>,
    cancel: CancellationToken,
    join: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for SseActorHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

impl SseTransport {
    /// Creates a validated legacy SSE adapter.
    pub fn new(
        http: Arc<dyn McpHttpPort>,
        url: &str,
        credential: Option<CredentialRef>,
        store: Arc<dyn CredentialStorePort>,
    ) -> Result<Self, McpError> {
        Ok(Self {
            http,
            initial_url: validate_url(url).map_err(|_| McpError::Http)?,
            credential,
            store,
            sequence: std::sync::atomic::AtomicU64::new(1),
            actor: tokio::sync::Mutex::new(None),
        })
    }

    async fn ensure_actor(&self, cancellation: CancellationToken) -> Result<Url, McpError> {
        let mut guard = self.actor.lock().await;
        if let Some(actor) = guard.as_ref() {
            return Ok(actor.endpoint.clone());
        }
        let request = authenticated_request(
            HttpMethod::Get,
            self.initial_url.clone(),
            Vec::new(),
            BTreeMap::from([("accept".into(), "text/event-stream".into())]),
            self.credential.as_ref(),
            self.store.as_ref(),
        )
        .await?;
        let (response, stream) = await_mcp(
            &cancellation,
            self.http.open_sse(request, cancellation.clone()),
        )
        .await?;
        validate_response(&self.initial_url, &response, "text/event-stream")?;
        let mut parser = SseParser::new();
        for chunk in response.chunks {
            parser.push(&chunk)?;
        }
        let endpoint =
            discover_endpoint(&self.initial_url, &stream, &mut parser, &cancellation).await?;
        let (commands, receiver) = tokio::sync::mpsc::channel(MCP_PENDING_REQUESTS_MAX);
        let cancel = CancellationToken::new();
        let join = tokio::spawn(sse_actor(stream, parser, receiver, cancel.clone()));
        *guard = Some(SseActorHandle {
            endpoint: endpoint.clone(),
            commands,
            cancel,
            join: Some(join),
        });
        Ok(endpoint)
    }

    pub(super) fn request(
        &self,
        method: &str,
        params: Value,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, Value> {
        let id = next_id(&self.sequence);
        let method = method.to_owned();
        Box::pin(async move {
            let endpoint = self.ensure_actor(cancellation.clone()).await?;
            let (commands, response) = self.register(id, &cancellation).await?;
            let posted = self
                .post(
                    endpoint,
                    encode_request(id, &method, params)?,
                    cancellation.clone(),
                )
                .await;
            if let Err(error) = posted {
                let _ = commands.send(RouterCommand::Remove(id)).await;
                return Err(error);
            }
            let result = await_mcp(&cancellation, async {
                response.await.map_err(|_| McpError::Eof)?
            })
            .await;
            if result.is_err() {
                let _ = commands.send(RouterCommand::Remove(id)).await;
            }
            result
        })
    }

    pub(super) fn notify(
        &self,
        method: &str,
        params: Value,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, ()> {
        let method = method.to_owned();
        Box::pin(async move {
            let endpoint = self.ensure_actor(cancellation.clone()).await?;
            self.post(
                endpoint,
                encode_notification(&method, params)?,
                cancellation,
            )
            .await
        })
    }

    async fn register(
        &self,
        id: u64,
        cancellation: &CancellationToken,
    ) -> Result<(tokio::sync::mpsc::Sender<RouterCommand>, ResponseReceiver), McpError> {
        let commands = self
            .actor
            .lock()
            .await
            .as_ref()
            .ok_or(McpError::Eof)?
            .commands
            .clone();
        let (reply, response) = tokio::sync::oneshot::channel();
        let (ack, registered) = tokio::sync::oneshot::channel();
        send_command(
            &commands,
            RouterCommand::Register { id, reply, ack },
            cancellation,
        )
        .await?;
        await_ack(registered, cancellation).await?;
        Ok((commands, response))
    }

    async fn post(
        &self,
        endpoint: Url,
        body: Vec<u8>,
        cancellation: CancellationToken,
    ) -> Result<(), McpError> {
        let request = authenticated_request(
            HttpMethod::Post,
            endpoint.clone(),
            body,
            json_headers(),
            self.credential.as_ref(),
            self.store.as_ref(),
        )
        .await?;
        let response = await_mcp(
            &cancellation,
            self.http.request(request, cancellation.clone()),
        )
        .await?;
        validate_origin(&endpoint, &response.final_url)?;
        if (200..300).contains(&response.status) {
            Ok(())
        } else {
            Err(McpError::Http)
        }
    }

    pub(super) fn close_transport(&self) -> McpFuture<'_, ()> {
        Box::pin(async move {
            let Some(mut actor) = self.actor.lock().await.take() else {
                return Ok(());
            };
            actor.cancel.cancel();
            if let Some(join) = actor.join.take() {
                await_mcp(&CancellationToken::new(), async {
                    join.await.map_err(|_| McpError::Unreachable)
                })
                .await?;
            }
            Ok(())
        })
    }
}

pub(super) async fn send_command(
    commands: &tokio::sync::mpsc::Sender<RouterCommand>,
    command: RouterCommand,
    cancellation: &CancellationToken,
) -> Result<(), McpError> {
    await_mcp(cancellation, async {
        commands.send(command).await.map_err(|_| McpError::Eof)
    })
    .await
}

pub(super) async fn await_ack(
    ack: tokio::sync::oneshot::Receiver<Result<(), McpError>>,
    cancellation: &CancellationToken,
) -> Result<(), McpError> {
    await_mcp(cancellation, async {
        ack.await.map_err(|_| McpError::Eof)?
    })
    .await
}

async fn discover_endpoint(
    initial: &Url,
    stream: &Arc<dyn McpSseStream>,
    parser: &mut SseParser,
    cancellation: &CancellationToken,
) -> Result<Url, McpError> {
    loop {
        while let Some(event) = parser.pop() {
            if event.event == "endpoint" {
                return endpoint_url(initial, &event.data);
            }
        }
        let chunk = await_mcp(cancellation, stream.recv(cancellation.clone()))
            .await?
            .ok_or(McpError::Eof)?;
        parser.push(&chunk)?;
    }
}

async fn sse_actor(
    stream: Arc<dyn McpSseStream>,
    mut parser: SseParser,
    mut commands: tokio::sync::mpsc::Receiver<RouterCommand>,
    cancel: CancellationToken,
) {
    let mut router = ResponseRouter::new();
    let terminal = loop {
        route_events(&mut parser, &mut router);
        tokio::select! {
            biased;
            () = cancel.cancelled() => break McpError::Cancelled,
            command = commands.recv() => match command {
                Some(command) => router.handle(command),
                None => break McpError::Eof,
            },
            chunk = await_mcp(&cancel, stream.recv(cancel.clone())) => match chunk {
                Ok(Some(chunk)) => {
                    if parser.push(&chunk).is_err() {
                        break McpError::Protocol;
                    }
                }
                Ok(None) => break McpError::Eof,
                Err(error) => break error,
            },
        }
    };
    router.drain(terminal);
    let _ = await_mcp(&CancellationToken::new(), stream.close()).await;
}

fn route_events(parser: &mut SseParser, router: &mut ResponseRouter) {
    while let Some(event) = parser.pop() {
        if event.event != "message" {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&event.data) {
            router.route(value);
        }
    }
}
