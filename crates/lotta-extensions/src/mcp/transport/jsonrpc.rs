use super::http::HttpTransport;
use super::sse::SseTransport;
use super::sse_codec::{authenticated_request, join_chunks, json_headers, validate_origin};
use super::stdio::StdioTransport;
use super::{
    CancellationToken, ClientInfo, HttpMethod, InitializeResult, JsonRpcNotification,
    MCP_MESSAGE_BYTES_MAX, MCP_PROTOCOL_VERSION, McpError, McpFuture, McpTransport, ToolCallId,
    ToolCallResult, ToolsPage, Value, await_mcp, json,
};
pub(super) fn validate_initialize(value: Value) -> Result<InitializeResult, McpError> {
    let result: InitializeResult = serde_json::from_value(value).map_err(|_| McpError::Protocol)?;
    if result.protocol_version != MCP_PROTOCOL_VERSION
        || !result.capabilities.is_object()
        || !result.server_info.is_object()
    {
        return Err(McpError::Protocol);
    }
    Ok(result)
}

macro_rules! impl_transport {
    ($transport:ty, $request:ident, $notify:ident, $shutdown:ident) => {
        impl McpTransport for $transport {
            fn initialize(
                &self,
                client: ClientInfo,
                cancellation: CancellationToken,
            ) -> McpFuture<'_, InitializeResult> {
                Box::pin(async move {
                    let value = self
                        .$request(
                            "initialize",
                            json!({"protocolVersion":MCP_PROTOCOL_VERSION,
                                "capabilities":{},"clientInfo":client}),
                            cancellation.clone(),
                        )
                        .await?;
                    let result = validate_initialize(value)?;
                    self.$notify("notifications/initialized", json!({}), cancellation)
                        .await?;
                    Ok(result)
                })
            }
            fn tools_list(
                &self,
                cursor: Option<&str>,
                cancellation: CancellationToken,
            ) -> McpFuture<'_, ToolsPage> {
                let params = cursor.map_or_else(|| json!({}), |value| json!({"cursor":value}));
                Box::pin(async move {
                    serde_json::from_value(self.$request("tools/list", params, cancellation).await?)
                        .map_err(|_| McpError::Protocol)
                })
            }
            fn tools_call(
                &self,
                call_id: &ToolCallId,
                name: &str,
                arguments: Value,
                cancellation: CancellationToken,
            ) -> McpFuture<'_, ToolCallResult> {
                let name = name.to_owned();
                let call_id = call_id.as_str().to_owned();
                Box::pin(async move {
                    serde_json::from_value(
                        self.$request(
                            "tools/call",
                            json!({"name":name,"arguments":arguments,
                                "_meta":{"progressToken":call_id}}),
                            cancellation,
                        )
                        .await?,
                    )
                    .map_err(|_| McpError::Protocol)
                })
            }
            fn shutdown(&self) -> McpFuture<'_, ()> {
                self.$shutdown()
            }
        }
    };
}

impl StdioTransport {
    fn notify(
        &self,
        method: &str,
        params: Value,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, ()> {
        let method = method.to_owned();
        Box::pin(async move {
            self.port
                .notify(JsonRpcNotification::new(&method, params)?, cancellation)
                .await
        })
    }
}

impl HttpTransport {
    pub(super) fn notify(
        &self,
        method: &str,
        params: Value,
        cancellation: CancellationToken,
    ) -> McpFuture<'_, ()> {
        let method = method.to_owned();
        Box::pin(async move {
            let body = encode_notification(&method, params)?;
            let mut headers = json_headers();
            headers.insert(
                "accept".into(),
                "application/json, text/event-stream".into(),
            );
            headers.insert("mcp-protocol-version".into(), MCP_PROTOCOL_VERSION.into());
            if let Some(session) = self.session.lock().await.as_ref() {
                headers.insert("mcp-session-id".into(), session.as_str().into());
            }
            let request = authenticated_request(
                HttpMethod::Post,
                self.url.clone(),
                body,
                headers,
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
            let established = self
                .accept_session(response.headers.get("mcp-session-id"))
                .await?;
            if established {
                self.ensure_actor(cancellation.clone()).await?;
            }
            if response.status == 202
                || ((200..300).contains(&response.status)
                    && join_chunks(&response.chunks)?.is_empty())
            {
                Ok(())
            } else {
                Err(McpError::Http)
            }
        })
    }
}

impl StdioTransport {
    fn close_transport(&self) -> McpFuture<'_, ()> {
        self.port.close()
    }
}
impl_transport!(StdioTransport, request, notify, close_transport);
impl_transport!(SseTransport, request, notify, close_transport);
impl_transport!(HttpTransport, request, notify, close_session);

pub(super) fn next_id(sequence: &std::sync::atomic::AtomicU64) -> u64 {
    sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
pub(super) fn encode_notification(method: &str, params: Value) -> Result<Vec<u8>, McpError> {
    encode_message(json!({"jsonrpc":"2.0","method":method,"params":params}))
}
pub(super) fn encode_request(id: u64, method: &str, params: Value) -> Result<Vec<u8>, McpError> {
    encode_message(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
}
pub(super) fn encode_message(value: Value) -> Result<Vec<u8>, McpError> {
    let bytes = serde_json::to_vec(&value).map_err(|_| McpError::Protocol)?;
    if bytes.len() > MCP_MESSAGE_BYTES_MAX {
        Err(McpError::Limit)
    } else {
        Ok(bytes)
    }
}
pub(super) fn response_result(value: Value, id: u64) -> Result<Value, McpError> {
    if value.get("jsonrpc") != Some(&Value::String("2.0".into()))
        || value.get("id") != Some(&json!(id))
    {
        return Err(McpError::Protocol);
    }
    match (value.get("result"), value.get("error")) {
        (Some(result), None) => Ok(result.clone()),
        (None, Some(error)) if error.is_object() => Err(McpError::Remote),
        _ => Err(McpError::Protocol),
    }
}
