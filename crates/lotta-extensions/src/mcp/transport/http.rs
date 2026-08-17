use super::jsonrpc::{encode_request, next_id};
use super::router::{ResponseReceiver, ResponseRouter, RouterCommand};
use super::sse::{await_ack, send_command};
use super::sse_codec::{
    SseParser, authenticated_request, join_chunks, json_headers, validate_origin,
};
use super::{
    Arc, BTreeMap, CancellationToken, CredentialRef, CredentialStorePort, HttpMethod,
    MCP_PENDING_REQUESTS_MAX, MCP_PROTOCOL_VERSION, MCP_STREAM_RECONNECTS_MAX, McpError, McpFuture,
    McpHttpPort, McpSseStream, SessionId, Url, Value, await_mcp, validate_url,
};

/// Streamable HTTP concrete adapter.
pub struct HttpTransport {
    pub(super) http: Arc<dyn McpHttpPort>,
    pub(super) url: Url,
    pub(super) session: tokio::sync::Mutex<Option<SessionId>>,
    pub(super) credential: Option<CredentialRef>,
    pub(super) store: Arc<dyn CredentialStorePort>,
    sequence: std::sync::atomic::AtomicU64,
    actor: tokio::sync::Mutex<Option<HttpActorHandle>>,
}
struct HttpActorHandle {
    commands: tokio::sync::mpsc::Sender<RouterCommand>,
    cancel: CancellationToken,
    join: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for HttpActorHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

impl HttpTransport {
    /// Creates a validated streamable HTTP adapter.
    pub fn new(
        http: Arc<dyn McpHttpPort>,
        url: &str,
        credential: Option<CredentialRef>,
        store: Arc<dyn CredentialStorePort>,
    ) -> Result<Self, McpError> {
        Ok(Self {
            http,
            url: validate_url(url).map_err(|_| McpError::Http)?,
            session: tokio::sync::Mutex::new(None),
            credential,
            store,
            sequence: std::sync::atomic::AtomicU64::new(1),
            actor: tokio::sync::Mutex::new(None),
        })
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
            let registered = self.register_if_active(id, &cancellation).await?;
            let session = self.session.lock().await.clone();
            let response = self
                .post(
                    encode_request(id, &method, params)?,
                    session.as_ref(),
                    cancellation.clone(),
                )
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    self.remove_registered(id, registered.as_ref()).await;
                    return Err(error);
                }
            };
            self.finish_response(id, response, registered, cancellation)
                .await
        })
    }

    async fn finish_response(
        &self,
        id: u64,
        response: super::McpHttpResponse,
        mut registered: Option<Registered>,
        cancellation: CancellationToken,
    ) -> Result<Value, McpError> {
        let established = self
            .accept_session(response.headers.get("mcp-session-id"))
            .await?;
        if response.status == 202 && established {
            self.ensure_actor(cancellation.clone()).await?;
        }
        if response.status == 202 {
            if registered.is_none() {
                registered = Some(self.register(id, &cancellation).await?);
            }
            let registered = registered.ok_or(McpError::Protocol)?;
            return await_registered(id, registered, cancellation).await;
        }
        let value = immediate_response(&response, id)?;
        match registered {
            Some(registered) => resolve_registered(id, value, registered, cancellation).await,
            None => super::jsonrpc::response_result(value, id),
        }
    }

    async fn register_if_active(
        &self,
        id: u64,
        cancellation: &CancellationToken,
    ) -> Result<Option<Registered>, McpError> {
        let commands = self
            .actor
            .lock()
            .await
            .as_ref()
            .map(|actor| actor.commands.clone());
        match commands {
            Some(commands) => Ok(Some(register_on(commands, id, cancellation).await?)),
            None => Ok(None),
        }
    }

    async fn register(
        &self,
        id: u64,
        cancellation: &CancellationToken,
    ) -> Result<Registered, McpError> {
        let commands = self
            .actor
            .lock()
            .await
            .as_ref()
            .ok_or(McpError::Protocol)?
            .commands
            .clone();
        register_on(commands, id, cancellation).await
    }

    async fn remove_registered(&self, id: u64, registered: Option<&Registered>) {
        if let Some(registered) = registered {
            let _ = registered.commands.send(RouterCommand::Remove(id)).await;
        }
    }

    async fn post(
        &self,
        body: Vec<u8>,
        session: Option<&SessionId>,
        cancellation: CancellationToken,
    ) -> Result<super::McpHttpResponse, McpError> {
        let request = authenticated_request(
            HttpMethod::Post,
            self.url.clone(),
            body,
            protocol_headers(session, true),
            self.credential.as_ref(),
            self.store.as_ref(),
        )
        .await?;
        let response = await_mcp(
            &cancellation,
            self.http.request(request, cancellation.clone()),
        )
        .await?;
        validate_origin(&self.url, &response.final_url)?;
        if (200..300).contains(&response.status) {
            Ok(response)
        } else {
            Err(McpError::Http)
        }
    }

    pub(super) async fn accept_session(&self, header: Option<&String>) -> Result<bool, McpError> {
        let Some(header) = header else {
            return Ok(false);
        };
        let candidate = SessionId::new(header.clone()).map_err(|_| McpError::Protocol)?;
        let mut session = self.session.lock().await;
        match session.as_ref() {
            Some(current) if current != &candidate => Err(McpError::Protocol),
            Some(_) => Ok(false),
            None => {
                *session = Some(candidate);
                Ok(true)
            }
        }
    }

    pub(super) async fn ensure_actor(
        &self,
        cancellation: CancellationToken,
    ) -> Result<(), McpError> {
        let mut actor = self.actor.lock().await;
        if actor.is_some() {
            return Ok(());
        }
        let session = self
            .session
            .lock()
            .await
            .clone()
            .ok_or(McpError::Protocol)?;
        let (commands, receiver) = tokio::sync::mpsc::channel(MCP_PENDING_REQUESTS_MAX);
        let cancel = CancellationToken::new();
        let resources = HttpActorResources {
            http: Arc::clone(&self.http),
            url: self.url.clone(),
            session,
            credential: self.credential.clone(),
            store: Arc::clone(&self.store),
        };
        let join = tokio::spawn(http_actor(
            resources,
            receiver,
            cancel.clone(),
            cancellation,
        ));
        *actor = Some(HttpActorHandle {
            commands,
            cancel,
            join: Some(join),
        });
        Ok(())
    }

    pub(super) fn close_session(&self) -> McpFuture<'_, ()> {
        Box::pin(async move {
            self.stop_actor().await?;
            let Some(session) = self.session.lock().await.take() else {
                return Ok(());
            };
            self.delete_session(&session).await
        })
    }

    async fn stop_actor(&self) -> Result<(), McpError> {
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
    }

    async fn delete_session(&self, session: &SessionId) -> Result<(), McpError> {
        let request = authenticated_request(
            HttpMethod::Delete,
            self.url.clone(),
            Vec::new(),
            protocol_headers(Some(session), false),
            self.credential.as_ref(),
            self.store.as_ref(),
        )
        .await?;
        let cancellation = CancellationToken::new();
        let response = await_mcp(
            &cancellation,
            self.http.request(request, cancellation.clone()),
        )
        .await?;
        validate_origin(&self.url, &response.final_url)?;
        if (200..300).contains(&response.status) {
            Ok(())
        } else {
            Err(McpError::Http)
        }
    }
}

struct Registered {
    commands: tokio::sync::mpsc::Sender<RouterCommand>,
    response: ResponseReceiver,
}

async fn register_on(
    commands: tokio::sync::mpsc::Sender<RouterCommand>,
    id: u64,
    cancellation: &CancellationToken,
) -> Result<Registered, McpError> {
    let (reply, response) = tokio::sync::oneshot::channel();
    let (ack, registered) = tokio::sync::oneshot::channel();
    send_command(
        &commands,
        RouterCommand::Register { id, reply, ack },
        cancellation,
    )
    .await?;
    await_ack(registered, cancellation).await?;
    Ok(Registered { commands, response })
}

async fn resolve_registered(
    id: u64,
    value: Value,
    registered: Registered,
    cancellation: CancellationToken,
) -> Result<Value, McpError> {
    let (ack, resolved) = tokio::sync::oneshot::channel();
    send_command(
        &registered.commands,
        RouterCommand::Resolve { id, value, ack },
        &cancellation,
    )
    .await?;
    await_ack(resolved, &cancellation).await?;
    await_registered(id, registered, cancellation).await
}

async fn await_registered(
    id: u64,
    registered: Registered,
    cancellation: CancellationToken,
) -> Result<Value, McpError> {
    let result = await_mcp(&cancellation, async {
        registered.response.await.map_err(|_| McpError::Eof)?
    })
    .await;
    if result.is_err() {
        let _ = registered.commands.send(RouterCommand::Remove(id)).await;
    }
    result
}

fn immediate_response(response: &super::McpHttpResponse, id: u64) -> Result<Value, McpError> {
    let content = response.headers.get("content-type").ok_or(McpError::Http)?;
    if content.starts_with("application/json") {
        parse_response_value(&join_chunks(&response.chunks)?)
    } else if content.starts_with("text/event-stream") {
        parse_post_sse(&response.chunks, id)
    } else {
        Err(McpError::Http)
    }
}

fn parse_response_value(bytes: &[u8]) -> Result<Value, McpError> {
    if bytes.len() > super::MCP_MESSAGE_BYTES_MAX {
        return Err(McpError::Limit);
    }
    serde_json::from_slice(bytes).map_err(|_| McpError::Protocol)
}

fn protocol_headers(session: Option<&SessionId>, accepts_body: bool) -> BTreeMap<String, String> {
    let mut headers = if accepts_body {
        json_headers()
    } else {
        BTreeMap::new()
    };
    headers.insert("mcp-protocol-version".into(), MCP_PROTOCOL_VERSION.into());
    headers.insert(
        "accept".into(),
        if accepts_body {
            "application/json, text/event-stream".into()
        } else {
            "text/event-stream".into()
        },
    );
    if let Some(session) = session {
        headers.insert("mcp-session-id".into(), session.as_str().into());
    }
    headers
}

fn parse_post_sse(chunks: &[Vec<u8>], id: u64) -> Result<Value, McpError> {
    let mut parser = SseParser::new();
    for chunk in chunks {
        parser.push(chunk)?;
    }
    while let Some(event) = parser.pop() {
        if event.event != "message" {
            continue;
        }
        let value = serde_json::from_str::<Value>(&event.data).map_err(|_| McpError::Protocol)?;
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return Ok(value);
        }
    }
    Err(McpError::Protocol)
}

struct HttpActorResources {
    http: Arc<dyn McpHttpPort>,
    url: Url,
    session: SessionId,
    credential: Option<CredentialRef>,
    store: Arc<dyn CredentialStorePort>,
}

async fn open_http_stream(
    resources: &HttpActorResources,
    last_id: Option<&str>,
    cancellation: &CancellationToken,
) -> Result<(super::McpHttpResponse, Arc<dyn McpSseStream>), McpError> {
    let mut headers = protocol_headers(Some(&resources.session), false);
    if let Some(last_id) = last_id {
        headers.insert("last-event-id".into(), last_id.into());
    }
    let request = authenticated_request(
        HttpMethod::Get,
        resources.url.clone(),
        Vec::new(),
        headers,
        resources.credential.as_ref(),
        resources.store.as_ref(),
    )
    .await?;
    let (response, stream) = await_mcp(
        cancellation,
        resources.http.open_sse(request, cancellation.clone()),
    )
    .await?;
    validate_stream_response(resources, &response)?;
    Ok((response, stream))
}

fn validate_stream_response(
    resources: &HttpActorResources,
    response: &super::McpHttpResponse,
) -> Result<(), McpError> {
    validate_origin(&resources.url, &response.final_url)?;
    let content = response.headers.get("content-type");
    if !(200..300).contains(&response.status)
        || !content.is_some_and(|value| value.starts_with("text/event-stream"))
    {
        return Err(McpError::Http);
    }
    if response
        .headers
        .get("mcp-session-id")
        .is_some_and(|value| value != resources.session.as_str())
    {
        return Err(McpError::Protocol);
    }
    Ok(())
}

async fn http_actor(
    resources: HttpActorResources,
    commands: tokio::sync::mpsc::Receiver<RouterCommand>,
    cancel: CancellationToken,
    startup: CancellationToken,
) {
    let mut state = HttpActorState::new(commands);
    let stream = open_http_stream(&resources, None, &startup).await;
    let mut stream = match stream {
        Ok((response, stream)) => {
            if !state.push_chunks(response.chunks) {
                state.router.drain(McpError::Protocol);
                return;
            }
            stream
        }
        Err(error) => {
            state.router.drain(error);
            return;
        }
    };
    let terminal = actor_loop(&resources, &mut state, &mut stream, &cancel).await;
    state.router.drain(terminal);
    let _ = await_mcp(&CancellationToken::new(), stream.close()).await;
}

struct HttpActorState {
    commands: tokio::sync::mpsc::Receiver<RouterCommand>,
    router: ResponseRouter,
    parser: SseParser,
    last_id: Option<String>,
    retries: usize,
}
impl HttpActorState {
    fn new(commands: tokio::sync::mpsc::Receiver<RouterCommand>) -> Self {
        Self {
            commands,
            router: ResponseRouter::new(),
            parser: SseParser::new(),
            last_id: None,
            retries: 0,
        }
    }
    fn push_chunks(&mut self, chunks: Vec<Vec<u8>>) -> bool {
        chunks
            .into_iter()
            .all(|chunk| self.parser.push(&chunk).is_ok())
    }
    fn route(&mut self) {
        while let Some(event) = self.parser.pop() {
            if let Some(id) = event.id {
                self.last_id = Some(id);
            }
            if event.event == "message"
                && let Ok(value) = serde_json::from_str::<Value>(&event.data)
            {
                self.router.route(value);
            }
        }
    }
}

async fn actor_loop(
    resources: &HttpActorResources,
    state: &mut HttpActorState,
    stream: &mut Arc<dyn McpSseStream>,
    cancel: &CancellationToken,
) -> McpError {
    loop {
        state.route();
        tokio::select! {
            biased;
            () = cancel.cancelled() => return McpError::Cancelled,
            command = state.commands.recv() => match command {
                Some(command) => state.router.handle(command),
                None => return McpError::Eof,
            },
            chunk = await_mcp(cancel, stream.recv(cancel.clone())) => {
                if let Some(error) = handle_chunk(resources, state, stream, cancel, chunk).await {
                    return error;
                }
            },
        }
    }
}

async fn handle_chunk(
    resources: &HttpActorResources,
    state: &mut HttpActorState,
    stream: &mut Arc<dyn McpSseStream>,
    cancel: &CancellationToken,
    chunk: Result<Option<Vec<u8>>, McpError>,
) -> Option<McpError> {
    match chunk {
        Ok(Some(chunk)) if state.parser.push(&chunk).is_ok() => None,
        Ok(Some(_)) => Some(McpError::Protocol),
        Ok(None) if state.retries < MCP_STREAM_RECONNECTS_MAX => {
            let _ = await_mcp(cancel, stream.close()).await;
            state.retries += 1;
            match open_http_stream(resources, state.last_id.as_deref(), cancel).await {
                Ok((response, resumed)) => {
                    if state.push_chunks(response.chunks) {
                        *stream = resumed;
                        None
                    } else {
                        Some(McpError::Protocol)
                    }
                }
                Err(error) => Some(error),
            }
        }
        Ok(None) => Some(McpError::Eof),
        Err(error) => Some(error),
    }
}
