use super::{
    BTreeMap, CredentialRef, CredentialStorePort, HttpMethod, MCP_EVENT_ID_BYTES_MAX,
    MCP_MESSAGE_BYTES_MAX, MCP_SSE_EVENT_BYTES_MAX, MCP_SSE_LINE_BYTES_MAX, McpError,
    McpHttpRequest, McpHttpResponse, SecretValue, Url,
};
pub(super) async fn authenticated_request(
    method: HttpMethod,
    url: Url,
    body: Vec<u8>,
    headers: BTreeMap<String, String>,
    credential: Option<&CredentialRef>,
    store: &dyn CredentialStorePort,
) -> Result<McpHttpRequest, McpError> {
    let authorization = match resolve_secret(credential, store).await? {
        Some(secret) => Some(secret.expose(|value| {
            let mut header = b"Bearer ".to_vec();
            header.extend_from_slice(value);
            SecretValue::new(header)
        })?),
        None => None,
    };
    Ok(McpHttpRequest {
        method,
        url,
        headers,
        body,
        authorization,
    })
}
pub(super) async fn resolve_secret(
    credential: Option<&CredentialRef>,
    store: &dyn CredentialStorePort,
) -> Result<Option<SecretValue>, McpError> {
    match credential {
        Some(reference) => Ok(Some(store.resolve(reference).await?.value)),
        None => Ok(None),
    }
}

pub(super) fn json_headers() -> BTreeMap<String, String> {
    BTreeMap::from([("content-type".into(), "application/json".into())])
}
pub(super) fn validate_response(
    expected: &Url,
    response: &McpHttpResponse,
    content: &str,
) -> Result<(), McpError> {
    validate_origin(expected, &response.final_url)?;
    if !(200..300).contains(&response.status)
        || !response
            .headers
            .get("content-type")
            .is_some_and(|value| value.starts_with(content))
    {
        return Err(McpError::Http);
    }
    Ok(())
}
pub(super) fn validate_origin(expected: &Url, actual: &Url) -> Result<(), McpError> {
    if expected.scheme() != actual.scheme()
        || expected.host_str() != actual.host_str()
        || expected.port_or_known_default() != actual.port_or_known_default()
    {
        Err(McpError::Http)
    } else {
        Ok(())
    }
}
pub(super) fn join_chunks(chunks: &[Vec<u8>]) -> Result<Vec<u8>, McpError> {
    let length = chunks
        .iter()
        .try_fold(0usize, |sum, chunk| sum.checked_add(chunk.len()))
        .ok_or(McpError::Limit)?;
    if length > MCP_MESSAGE_BYTES_MAX {
        return Err(McpError::Limit);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| McpError::Limit)?;
    for chunk in chunks {
        bytes.extend_from_slice(chunk);
    }
    Ok(bytes)
}

pub(super) struct SseEvent {
    pub(super) event: String,
    pub(super) data: String,
    pub(super) id: Option<String>,
}
pub(super) struct SseParser {
    pending: Vec<u8>,
    pub(super) event: String,
    pub(super) data: String,
    id: Option<String>,
    ready: std::collections::VecDeque<SseEvent>,
}
impl SseParser {
    pub(super) const fn new() -> Self {
        Self {
            pending: Vec::new(),
            event: String::new(),
            data: String::new(),
            id: None,
            ready: std::collections::VecDeque::new(),
        }
    }
    pub(super) fn push(&mut self, chunk: &[u8]) -> Result<(), McpError> {
        if chunk.len() > MCP_MESSAGE_BYTES_MAX
            || self
                .pending
                .len()
                .checked_add(chunk.len())
                .ok_or(McpError::Limit)?
                > MCP_MESSAGE_BYTES_MAX
        {
            return Err(McpError::Limit);
        }
        self.pending.extend_from_slice(chunk);
        while let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
            if position > MCP_SSE_LINE_BYTES_MAX {
                return Err(McpError::Limit);
            }
            let mut line: Vec<u8> = self.pending.drain(..=position).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.line(&line)?;
        }
        if self.pending.len() > MCP_SSE_LINE_BYTES_MAX {
            return Err(McpError::Limit);
        }
        Ok(())
    }
    fn line(&mut self, line: &[u8]) -> Result<(), McpError> {
        let line = std::str::from_utf8(line).map_err(|_| McpError::Protocol)?;
        if line.is_empty() {
            if !self.data.is_empty() {
                self.ready.push_back(SseEvent {
                    event: if self.event.is_empty() {
                        "message".into()
                    } else {
                        std::mem::take(&mut self.event)
                    },
                    data: std::mem::take(&mut self.data),
                    id: self.id.take(),
                });
            }
            self.event.clear();
            return Ok(());
        }
        if let Some(value) = line.strip_prefix("event:") {
            value.trim_start().clone_into(&mut self.event);
        } else if let Some(value) = line.strip_prefix("id:") {
            let value = value.trim_start();
            if value.len() > MCP_EVENT_ID_BYTES_MAX || value.contains('\0') {
                return Err(McpError::Limit);
            }
            self.id = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            let value = value.trim_start();
            let added = value.len() + usize::from(!self.data.is_empty());
            if self.data.len().checked_add(added).ok_or(McpError::Limit)? > MCP_SSE_EVENT_BYTES_MAX
            {
                return Err(McpError::Limit);
            }
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(value);
        }
        Ok(())
    }
    pub(super) fn pop(&mut self) -> Option<SseEvent> {
        self.ready.pop_front()
    }
}
pub(super) fn endpoint_url(initial: &Url, value: &str) -> Result<Url, McpError> {
    let endpoint = initial.join(value).map_err(|_| McpError::Http)?;
    validate_origin(initial, &endpoint)?;
    Ok(endpoint)
}
