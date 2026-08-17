//! MCP JSON-RPC 2.0 session contract and concrete protocol adapters.

use super::{
    client::{CredentialRef, SessionId, validate_url},
    oauth::{CredentialError, CredentialStorePort, SecretValue},
};
use crate::sidecar::{
    SIDECAR_PROTOCOL_VERSION, SIDECAR_TIMEOUT_MS_MAX, SidecarCapability, SidecarEnvelope,
    SidecarEnvelopeKind, SidecarFrameLimit, SidecarOwnerIdentity,
    framing::write_frame,
    handshake::{SidecarSessionPolicy, ValidatedSidecarReader},
    supervisor::{RestartLimitError, SIDECAR_CHILD_JOIN_TIMEOUT},
};
use lotta_runtime::ports::ToolCallId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt, future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;
use url::Url;

/// Pinned MCP protocol version used by the v0.30.20 client.
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
/// Maximum JSON-RPC request or response body.
pub const MCP_MESSAGE_BYTES_MAX: usize = 8 * 1024 * 1024;
/// Maximum SSE line size.
pub const MCP_SSE_LINE_BYTES_MAX: usize = 64 * 1024;
/// Maximum SSE event aggregate size.
pub const MCP_SSE_EVENT_BYTES_MAX: usize = MCP_MESSAGE_BYTES_MAX;
/// Maximum response pagination pages.
pub const MCP_DISCOVERY_PAGES_MAX: usize = 128;
/// Default MCP I/O timeout.
pub const MCP_REQUEST_TIMEOUT: Duration = Duration::from_mins(5);
/// Bounded command queue owned by one stdio session actor.
pub const MCP_STDIO_REQUESTS_MAX: usize = 64;
/// Maximum pending requests owned by one retained network stream actor.
pub const MCP_PENDING_REQUESTS_MAX: usize = 64;
/// Maximum stream resume attempts after an established stream closes.
pub const MCP_STREAM_RECONNECTS_MAX: usize = 3;
/// Maximum retained SSE event identifier length.
pub const MCP_EVENT_ID_BYTES_MAX: usize = 4_096;

/// Stable secret-free transport failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum McpError {
    /// Transport endpoint was unreachable.
    #[error("MCP transport unavailable")]
    Unreachable,
    /// Request timed out.
    #[error("MCP request timed out")]
    Timeout,
    /// Request was cancelled.
    #[error("MCP request cancelled")]
    Cancelled,
    /// Peer closed before a complete response.
    #[error("MCP transport ended")]
    Eof,
    /// HTTP status, headers, redirect, or origin was invalid.
    #[error("MCP HTTP response was rejected")]
    Http,
    /// JSON-RPC or MCP response was malformed or mismatched.
    #[error("MCP protocol response was invalid")]
    Protocol,
    /// Message exceeded a named bound.
    #[error("MCP message exceeded its limit")]
    Limit,
    /// Server returned a JSON-RPC error.
    #[error("MCP server returned an error")]
    Remote,
    /// Credential resolution failed before transport use.
    #[error("MCP credential resolution failed")]
    Credential,
}
impl From<CredentialError> for McpError {
    fn from(_: CredentialError) -> Self {
        Self::Credential
    }
}

/// MCP client identity sent during initialization.
#[derive(Clone, Debug, Serialize)]
pub struct ClientInfo {
    /// Product name.
    pub name: String,
    /// Product version.
    pub version: String,
}

/// Validated server initialization result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Negotiated protocol version.
    pub protocol_version: String,
    /// Server capabilities object.
    pub capabilities: Value,
    /// Server implementation metadata.
    pub server_info: Value,
}

/// One remote MCP tool definition.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTool {
    /// Exact remote name.
    pub name: String,
    /// Optional title.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema input definition.
    pub input_schema: Value,
}

/// One bounded tools/list page.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsPage {
    /// Page members.
    pub tools: Vec<RemoteTool>,
    /// Opaque next cursor.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// Validated tools/call result.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    /// MCP content blocks.
    pub content: Vec<Value>,
    /// Server-defined failure marker.
    #[serde(default)]
    pub is_error: bool,
    /// Optional structured result.
    #[serde(default)]
    pub structured_content: Option<Value>,
}

/// Future returned by MCP transport sessions.
pub type McpFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, McpError>> + Send + 'a>>;

/// Awaits one MCP operation with the canonical timeout and caller cancellation.
pub async fn await_mcp<T>(
    cancellation: &CancellationToken,
    operation: impl Future<Output = Result<T, McpError>>,
) -> Result<T, McpError> {
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(McpError::Cancelled),
        result = tokio::time::timeout(MCP_REQUEST_TIMEOUT, operation) =>
            result.map_err(|_| McpError::Timeout)?,
    }
}

/// Validated JSON-RPC request envelope for the local stdio boundary.
#[derive(Clone, Debug)]
pub struct JsonRpcRequest {
    id: u64,
    value: Value,
}
impl JsonRpcRequest {
    /// Constructs one bounded request with fixed JSON-RPC version and numeric correlation ID.
    pub fn new(id: u64, method: &str, params: Value) -> Result<Self, McpError> {
        if method.is_empty() {
            return Err(McpError::Protocol);
        }
        let value = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        if serde_json::to_vec(&value)
            .map_err(|_| McpError::Protocol)?
            .len()
            > MCP_MESSAGE_BYTES_MAX
        {
            return Err(McpError::Limit);
        }
        Ok(Self { id, value })
    }
    /// Returns the fixed numeric request ID.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }
    /// Returns the validated JSON value for the authoritative Task 42 codec.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// Validated JSON-RPC notification envelope for the local stdio boundary.
#[derive(Clone, Debug)]
pub struct JsonRpcNotification(Value);
impl JsonRpcNotification {
    /// Constructs one bounded notification with no request ID.
    pub fn new(method: &str, params: Value) -> Result<Self, McpError> {
        if method.is_empty() {
            return Err(McpError::Protocol);
        }
        let value = json!({"jsonrpc":"2.0","method":method,"params":params});
        if serde_json::to_vec(&value)
            .map_err(|_| McpError::Protocol)?
            .len()
            > MCP_MESSAGE_BYTES_MAX
        {
            return Err(McpError::Limit);
        }
        Ok(Self(value))
    }
    /// Returns the validated JSON value for the authoritative Task 42 codec.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.0
    }
}

/// One initialized MCP transport session.
pub trait McpTransport: Send + Sync {
    /// Performs initialize and notifications/initialized.
    fn initialize(
        &self,
        client: ClientInfo,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, InitializeResult>;
    /// Requests one tools/list page.
    fn tools_list(
        &self,
        cursor: Option<&str>,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, ToolsPage>;
    /// Calls one exact remote tool.
    fn tools_call(
        &self,
        call_id: &ToolCallId,
        name: &str,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, ToolCallResult>;
    /// Closes this exact session.
    fn shutdown(&self) -> McpFuture<'_, ()>;
}

/// Public production factory selecting one distinct protocol adapter from validated config.
pub struct McpClientFactory {
    http: Arc<dyn McpHttpPort>,
    stdio: Arc<dyn McpStdioFactoryPort>,
    credentials: Arc<dyn CredentialStorePort>,
}
/// Injected Task 42 child-session capability used only for stdio launch.
pub trait McpStdioFactoryPort: Send + Sync {
    /// Launches a validated stdio configuration through the Task 42 lifecycle.
    fn launch<'a>(
        &'a self,
        config: &'a super::client::StdioConfig,
        credential: Option<SecretValue>,
        cancellation: CancellationToken,
    ) -> McpFuture<'a, Arc<dyn McpStdioPort>>;
}
impl McpClientFactory {
    /// Binds canonical network, child-process, and credential capabilities.
    #[must_use]
    pub fn new(
        http: Arc<dyn McpHttpPort>,
        stdio: Arc<dyn McpStdioFactoryPort>,
        credentials: Arc<dyn CredentialStorePort>,
    ) -> Self {
        Self {
            http,
            stdio,
            credentials,
        }
    }
    /// Creates the exact transport selected by validated configuration.
    #[must_use]
    pub fn create<'a>(
        &'a self,
        config: &'a super::client::McpServerConfig,
        cancellation: CancellationToken,
    ) -> McpFuture<'a, Arc<dyn McpTransport>> {
        Box::pin(async move {
            match config {
                super::client::McpServerConfig::Stdio(config) => {
                    let credential =
                        resolve_secret(config.credential.as_ref(), self.credentials.as_ref())
                            .await?;
                    Ok(Arc::new(StdioTransport::new(
                        self.stdio.launch(config, credential, cancellation).await?,
                    )) as Arc<dyn McpTransport>)
                }
                super::client::McpServerConfig::Sse(config) => Ok(Arc::new(SseTransport::new(
                    Arc::clone(&self.http),
                    &config.url,
                    config.credential.clone(),
                    Arc::clone(&self.credentials),
                )?)
                    as Arc<dyn McpTransport>),
                super::client::McpServerConfig::Http(config) => Ok(Arc::new(HttpTransport::new(
                    Arc::clone(&self.http),
                    &config.url,
                    config.credential.clone(),
                    Arc::clone(&self.credentials),
                )?)
                    as Arc<dyn McpTransport>),
            }
        })
    }
}
/// HTTP method required by an MCP adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpMethod {
    /// Initial stream request.
    Get,
    /// JSON-RPC request.
    Post,
    /// Session shutdown request.
    Delete,
}

/// Redacted HTTP request passed to the injected network adapter.
pub struct McpHttpRequest {
    /// HTTP method.
    pub method: HttpMethod,
    /// Validated URL.
    pub url: Url,
    /// Non-secret protocol headers.
    pub headers: BTreeMap<String, String>,
    /// Optional JSON body already bounded.
    pub body: Vec<u8>,
    authorization: Option<SecretValue>,
}
impl fmt::Debug for McpHttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpHttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("body_bytes", &self.body.len())
            .field("authorization", &"[REDACTED]")
            .finish()
    }
}
impl McpHttpRequest {
    /// Exposes Authorization only to the concrete request port.
    pub fn with_authorization<T>(&self, operation: impl FnOnce(Option<&[u8]>) -> T) -> T {
        match self.authorization.as_ref() {
            Some(value) => value.expose(|bytes| operation(Some(bytes))),
            None => operation(None),
        }
    }
}

/// One bounded complete HTTP response.
pub struct McpHttpResponse {
    /// Status code.
    pub status: u16,
    /// Final URL after redirects.
    pub final_url: Url,
    /// Case-insensitive headers normalized to lowercase.
    pub headers: BTreeMap<String, String>,
    /// Bounded response chunks.
    pub chunks: Vec<Vec<u8>>,
}

/// Future returned by the injected HTTP adapter.
pub type HttpFuture<'a> =
    Pin<Box<dyn Future<Output = Result<McpHttpResponse, McpError>> + Send + 'a>>;
/// Future returned by one persistent SSE stream operation.
pub type SseFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, McpError>> + Send + 'a>>;
/// Persistent, bounded SSE byte stream.
pub trait McpSseStream: Send + Sync {
    /// Receives one already bounded network chunk.
    fn recv(&self, cancellation: CancellationToken) -> SseFuture<'_, Option<Vec<u8>>>;
    /// Closes the retained stream.
    fn close(&self) -> SseFuture<'_, ()>;
}
/// Production-ready bounded HTTP request port.
pub trait McpHttpPort: Send + Sync {
    /// Sends one bounded non-streaming request with redirects disabled.
    fn request(&self, request: McpHttpRequest, cancellation: CancellationToken) -> HttpFuture<'_>;
    /// Opens one retained SSE response stream with redirects disabled.
    fn open_sse(
        &self,
        request: McpHttpRequest,
        cancellation: CancellationToken,
    ) -> SseFuture<'_, (McpHttpResponse, Arc<dyn McpSseStream>)>;
}

mod http;
mod jsonrpc;
mod router;
mod sse;
mod sse_codec;
mod stdio;

pub use http::HttpTransport;
pub use sse::SseTransport;
use sse_codec::resolve_secret;
pub use stdio::{
    McpChildFuture, McpLaunchFuture, McpStdioChild, McpStdioChildConnection, McpStdioChildLauncher,
    McpStdioLaunch, McpStdioPort, StdioTransport, Task42McpStdioFactory,
};
