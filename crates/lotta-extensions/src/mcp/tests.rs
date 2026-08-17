use super::discovery::decode_namespace;
use super::{client::*, discovery::namespace, oauth::*, transport::*};

use serde_json::json;
use std::{
    collections::{BTreeMap, VecDeque},
    future,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;
use url::Url;

struct Store;
impl CredentialStorePort for Store {
    fn resolve(&self, _: &CredentialRef) -> CredentialFuture<'_, VersionedSecret> {
        Box::pin(async { Err(CredentialError::Unknown) })
    }
    fn compare_replace(
        &self,
        _: &CredentialRef,
        _: u64,
        _: SecretValue,
    ) -> CredentialFuture<'_, u64> {
        Box::pin(async { Err(CredentialError::RevisionMismatch) })
    }
}

#[test]
fn omitted_type_defaults_to_stdio() {
    let value = json!({"name":"local","command":"/usr/bin/server"});
    assert!(matches!(
        serde_json::from_value::<McpServerConfig>(value).unwrap(),
        McpServerConfig::Stdio(_)
    ));
    assert!(
        serde_json::from_value::<McpServerConfig>(
            json!({"type":"stdio","name":"x","command":"relative"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<McpServerConfig>(
            json!({"type":"http","name":"x","url":"https://a.test","command":"/bin/x"})
        )
        .is_err()
    );
}

#[test]
fn config_transport_discriminants_and_credentials() {
    assert!(matches!(
        serde_json::from_value::<McpServerConfig>(
            json!({"type":"sse","name":"s","url":"https://a.test/events","credential":"store-id"})
        )
        .unwrap(),
        McpServerConfig::Sse(_)
    ));
    assert!(matches!(
        serde_json::from_value::<McpServerConfig>(
            json!({"type":"http","name":"h","url":"https://a.test/mcp"})
        )
        .unwrap(),
        McpServerConfig::Http(_)
    ));
    for url in [
        "https://a.test/mcp?token=secret",
        "https://a.test/mcp?safe=value",
        "https://user@a.test/mcp",
        "https://a.test/mcp#fragment",
        "file:///tmp/mcp",
    ] {
        assert!(
            serde_json::from_value::<McpServerConfig>(json!({"type":"http","name":"h","url":url}))
                .is_err()
        );
    }
}

#[test]
fn namespaces_per_server() {
    assert_eq!(namespace("one", "same").unwrap(), "mcp__one__same");
    assert_eq!(namespace("two", "same").unwrap(), "mcp__two__same");
    assert_ne!(
        namespace("a b", "x").unwrap(),
        namespace("a_b", "x").unwrap()
    );
    for (server, tool) in [
        ("literal_u20_", "space value"),
        ("under_score", "雪/_u20_"),
        ("dash-safe", "emoji-🦀"),
    ] {
        let encoded = namespace(server, tool).unwrap();
        assert!(encoded.starts_with("mcp__"));
        assert_eq!(
            decode_namespace(&encoded).unwrap(),
            (server.into(), tool.into())
        );
    }
}

#[test]
fn credentials_are_redacted() {
    let marker = b"marker-secret".to_vec();
    let secret = SecretValue::new(marker).unwrap();
    assert!(!format!("{secret:?} {secret}").contains("marker-secret"));
    let reference = CredentialRef::new("marker-reference".into()).unwrap();
    assert!(!format!("{reference:?} {reference}").contains("marker-reference"));
}

struct ScriptHttp {
    responses: Mutex<VecDeque<McpHttpResponse>>,
    requests: Mutex<Vec<McpHttpRequest>>,
}
struct ScriptStream {
    chunks: Mutex<VecDeque<Vec<u8>>>,
    wake: tokio::sync::Notify,
}
impl McpSseStream for ScriptStream {
    fn recv(&self, cancellation: CancellationToken) -> SseFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            if let Some(chunk) = self.chunks.lock().unwrap().pop_front() {
                return Ok(Some(chunk));
            }
            tokio::select! {
                () = cancellation.cancelled() => Ok(None),
                () = self.wake.notified() => Ok(self.chunks.lock().unwrap().pop_front()),
            }
        })
    }
    fn close(&self) -> SseFuture<'_, ()> {
        Box::pin(future::ready(Ok(())))
    }
}
impl McpHttpPort for ScriptHttp {
    fn request(&self, request: McpHttpRequest, _: CancellationToken) -> HttpFuture<'_> {
        self.requests.lock().unwrap().push(request);
        let response = self.responses.lock().unwrap().pop_front().unwrap();
        Box::pin(future::ready(Ok(response)))
    }
    fn open_sse(
        &self,
        request: McpHttpRequest,
        _: CancellationToken,
    ) -> SseFuture<'_, (McpHttpResponse, Arc<dyn McpSseStream>)> {
        self.requests.lock().unwrap().push(request);
        let response = self.responses.lock().unwrap().pop_front().unwrap();
        let stream: Arc<dyn McpSseStream> = Arc::new(ScriptStream {
            chunks: Mutex::new(VecDeque::from(response.chunks.clone())),
            wake: tokio::sync::Notify::new(),
        });
        let response = McpHttpResponse {
            chunks: Vec::new(),
            ..response
        };
        Box::pin(future::ready(Ok((response, stream))))
    }
}
fn response(content: &str, body: &str) -> McpHttpResponse {
    McpHttpResponse {
        status: 200,
        final_url: Url::parse("https://a.test/mcp").unwrap(),
        headers: BTreeMap::from([("content-type".into(), content.into())]),
        chunks: vec![body.as_bytes().to_vec()],
    }
}

#[tokio::test]
pub(super) async fn http() {
    let http = Arc::new(ScriptHttp {
        responses: Mutex::new(VecDeque::from([
            response(
                "application/json",
                concat!(
                    r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","#,
                    r#""capabilities":{},"serverInfo":{}}}"#
                ),
            ),
            McpHttpResponse {
                status: 202,
                final_url: Url::parse("https://a.test/mcp").unwrap(),
                headers: BTreeMap::new(),
                chunks: Vec::new(),
            },
            response(
                "application/json",
                r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#,
            ),
        ])),
        requests: Mutex::new(Vec::new()),
    });
    let transport =
        HttpTransport::new(http.clone(), "https://a.test/mcp", None, Arc::new(Store)).unwrap();
    transport
        .initialize(
            ClientInfo {
                name: "lotta".into(),
                version: "1".into(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        transport
            .tools_list(None, CancellationToken::new())
            .await
            .unwrap()
            .tools
            .is_empty()
    );
    assert_eq!(http.requests.lock().unwrap()[0].method, HttpMethod::Post);
}

#[tokio::test]
pub(super) async fn sse() {
    let http = Arc::new(ScriptHttp {
        responses: Mutex::new(VecDeque::from([
            response(
                "text/event-stream",
                concat!(
                    "event: endpoint\ndata: /mcp\n\n",
                    "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{",
                    "\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},",
                    "\"serverInfo\":{}}}\n\n",
                    "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,",
                    "\"result\":{\"tools\":[]}}\n\n"
                ),
            ),
            response("application/json", ""),
            response("application/json", ""),
            response("application/json", ""),
        ])),
        requests: Mutex::new(Vec::new()),
    });
    let transport =
        SseTransport::new(http.clone(), "https://a.test/mcp", None, Arc::new(Store)).unwrap();
    transport
        .initialize(
            ClientInfo {
                name: "lotta".into(),
                version: "1".into(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        transport
            .tools_list(None, CancellationToken::new())
            .await
            .unwrap()
            .tools
            .is_empty()
    );
    assert_eq!(http.requests.lock().unwrap()[0].method, HttpMethod::Get);
    assert_eq!(http.requests.lock().unwrap()[1].method, HttpMethod::Post);
}

struct ScriptStdio {
    requests: Mutex<Vec<serde_json::Value>>,
}
impl McpStdioPort for ScriptStdio {
    fn request(
        &self,
        request: JsonRpcRequest,
        _: CancellationToken,
    ) -> McpFuture<'_, serde_json::Value> {
        let request = request.value().clone();
        let id = request.get("id").cloned();
        self.requests.lock().unwrap().push(request);
        let Some(id) = id else {
            return Box::pin(future::ready(Ok(json!({}))));
        };
        let result = if id == json!(1) {
            json!({"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"serverInfo":{}})
        } else {
            json!({"tools":[]})
        };
        Box::pin(future::ready(Ok(
            json!({"jsonrpc":"2.0","id":id,"result":result}),
        )))
    }
    fn notify(&self, notification: JsonRpcNotification, _: CancellationToken) -> McpFuture<'_, ()> {
        self.requests
            .lock()
            .unwrap()
            .push(notification.value().clone());
        Box::pin(future::ready(Ok(())))
    }
    fn close(&self) -> McpFuture<'_, ()> {
        Box::pin(future::ready(Ok(())))
    }
}

#[tokio::test]
async fn stdio() {
    let port = Arc::new(ScriptStdio {
        requests: Mutex::new(Vec::new()),
    });
    let transport = StdioTransport::new(port.clone());
    transport
        .initialize(
            ClientInfo {
                name: "lotta".into(),
                version: "1".into(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        transport
            .tools_list(None, CancellationToken::new())
            .await
            .unwrap()
            .tools
            .is_empty()
    );
    let requests = port.requests.lock().unwrap();
    assert_eq!(requests[0]["method"], "initialize");
    assert!(requests[1].get("id").is_none());
}
