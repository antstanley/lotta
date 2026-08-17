//! Deterministic single-connection HTTP/1.1 loopback used to replay the fixture corpus.
//!
//! The loopback replays one `raw-stream.txt` verbatim under the status and headers its index
//! record pins, so a non-2xx capture drives the adapter's real response-status path instead of a
//! test-side shortcut. The body is framed as HTTP chunks whose sizes cycle through
//! [`CHUNK_BYTES`], which splits SSE records, `data:` lines and multi-byte UTF-8 sequences across
//! transport reads. Nothing here sleeps, and only `127.0.0.1` is bound.

use lotta_testkit::fixtures::providers::ProviderCase;
use std::fmt::Write as _;
use std::sync::Arc;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Chunk sizes cycled over the response body; coprime-ish and smaller than any record.
const CHUNK_BYTES: [usize; 6] = [1, 7, 4, 13, 2, 29];
/// Maximum bytes accepted from one adapter request before the loopback gives up.
const REQUEST_BYTES_MAX: usize = 64 * 1024 * 1024;
/// Read buffer size for the recorded request.
const READ_BUFFER_BYTES: usize = 256 * 1024;

/// One complete request an adapter put on the wire.
pub(crate) struct RecordedRequest {
    /// Request method token.
    pub(crate) method: String,
    /// Request target, including the adapter-composed path.
    pub(crate) target: String,
    /// Lowercased header names paired with their exact values, in arrival order.
    pub(crate) headers: Vec<(String, String)>,
    /// Exact request body bytes.
    pub(crate) body: Vec<u8>,
}

/// A bound loopback listener serving exactly one recorded exchange.
#[derive(Clone)]
pub(crate) enum ResponseScript {
    Complete(Vec<u8>),
    WaitForNotify {
        prefix: Vec<u8>,
        notify: Arc<Notify>,
    },
    Truncate(Vec<u8>),
}

pub(crate) struct Loopback {
    base: reqwest::Url,
    server: JoinHandle<RecordedRequest>,
}

impl Loopback {
    /// Binds a loopback port and serves this case's captured response to the first connection.
    pub(crate) async fn start(case: &ProviderCase) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener");
        let address = listener.local_addr().expect("loopback address");
        let base = reqwest::Url::parse(&format!("http://{address}/v1")).expect("loopback base");
        let response = response_bytes(case);
        Self {
            base,
            server: tokio::spawn(async move {
                serve_once(&listener, ResponseScript::Complete(response)).await
            }),
        }
    }

    pub(crate) async fn scripted(script: ResponseScript) -> Self {
        Self::sequence([script]).await
    }

    pub(crate) async fn sequence(scripts: impl IntoIterator<Item = ResponseScript>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener");
        let address = listener.local_addr().expect("loopback address");
        let base = reqwest::Url::parse(&format!("http://{address}/v1")).expect("loopback base");
        let scripts = scripts.into_iter().collect::<Vec<_>>();
        Self {
            base,
            server: tokio::spawn(async move {
                let mut last = None;
                for script in scripts {
                    last = Some(serve_once(&listener, script).await);
                }
                last.expect("at least one loopback response")
            }),
        }
    }

    /// Aborts a server when a preflight adapter rejection correctly sends no request.
    pub(crate) fn assert_no_request(self) {
        assert!(!self.server.is_finished(), "unexpected provider request");
        self.server.abort();
    }

    /// Borrows the base URL the adapter must be constructed against.
    pub(crate) const fn base(&self) -> &reqwest::Url {
        &self.base
    }

    /// Awaits the served exchange and returns the request the adapter sent.
    pub(crate) async fn recorded(self) -> RecordedRequest {
        self.server.await.expect("loopback server")
    }
}

/// Renders the captured status line, headers, and chunk-framed body.
fn response_bytes(case: &ProviderCase) -> Vec<u8> {
    let response = case
        .record
        .response
        .as_ref()
        .expect("native fixtures pin explicit response metadata");
    let mut head = format!("HTTP/1.1 {} FIXTURE\r\n", response.status);
    for header in response.headers.as_slice() {
        write!(head, "{}: {}\r\n", header.name, header.value).expect("string write");
    }
    head.push_str("transfer-encoding: chunked\r\nconnection: close\r\n\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(&chunk_framed(case.raw_stream.as_bytes()));
    out
}

/// Cumulative chunk boundaries the loopback imposes on a body of `length` bytes.
fn boundaries(length: usize) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut offset = 0;
    while offset < length {
        let size = CHUNK_BYTES[offsets.len() % CHUNK_BYTES.len()].min(length - offset);
        offset += size;
        offsets.push(offset);
    }
    offsets
}

/// Frames `body` as HTTP chunks whose sizes cycle through [`CHUNK_BYTES`].
fn chunk_framed(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut start = 0;
    for end in boundaries(body.len()) {
        out.extend_from_slice(format!("{:x}\r\n", end - start).as_bytes());
        out.extend_from_slice(&body[start..end]);
        out.extend_from_slice(b"\r\n");
        start = end;
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

/// Reports whether some chunk boundary falls inside a multi-byte UTF-8 sequence.
pub(super) fn splits_multibyte(body: &str) -> bool {
    boundaries(body.len())
        .into_iter()
        .any(|offset| offset < body.len() && !body.is_char_boundary(offset))
}

/// Reports whether `body` contains any multi-byte UTF-8 sequence at all.
pub(super) fn has_multibyte(body: &str) -> bool {
    body.chars().any(|value| value.len_utf8() > 1)
}

/// Accepts one connection, records the request, writes the response, and closes.
async fn serve_once(listener: &TcpListener, response: ResponseScript) -> RecordedRequest {
    let (mut socket, _) = listener.accept().await.expect("loopback accept");
    let raw = read_request(&mut socket).await;
    match response {
        ResponseScript::Complete(response) => {
            if let Err(error) = socket.write_all(&response).await {
                assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
            }
            let _ = socket.shutdown().await;
        }
        ResponseScript::WaitForNotify { prefix, notify } => {
            socket.write_all(&prefix).await.expect("loopback prefix");
            notify.notify_waiters();
            std::future::pending::<()>().await;
        }
        ResponseScript::Truncate(response) => {
            socket
                .write_all(&response)
                .await
                .expect("loopback truncate");
            socket.shutdown().await.expect("loopback shutdown");
        }
    }
    parse_request(&raw)
}

/// Reads exactly one request: its head, then the body its `content-length` declares.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut raw = Vec::new();
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES].into_boxed_slice();
    loop {
        if let Some(end) = head_end(&raw)
            && raw.len() >= end + content_length(&raw[..end])
        {
            return raw;
        }
        let read = socket.read(&mut buffer).await.expect("loopback read");
        if read == 0 {
            return raw;
        }
        assert!(raw.len() + read <= REQUEST_BYTES_MAX, "request bound");
        raw.extend_from_slice(&buffer[..read]);
    }
}

/// Returns the offset just past the request head terminator.
fn head_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|start| start + 4)
}

/// Reads the declared body length from a request head.
fn content_length(head: &[u8]) -> usize {
    let head = std::str::from_utf8(head).expect("request head is UTF-8");
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}

/// Splits a recorded request into its method, target, headers, and body.
fn parse_request(raw: &[u8]) -> RecordedRequest {
    let Some(end) = head_end(raw) else {
        return RecordedRequest {
            method: String::new(),
            target: String::new(),
            headers: Vec::new(),
            body: Vec::new(),
        };
    };
    let head = std::str::from_utf8(&raw[..end]).expect("request head is UTF-8");
    let mut lines = head.lines();
    let start = lines.next().expect("request line");
    let mut fields = start.split(' ');
    let method = fields.next().expect("request method").to_owned();
    let target = fields.next().expect("request target").to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    RecordedRequest {
        method,
        target,
        headers,
        body: raw[end..].to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::{boundaries, chunk_framed, has_multibyte, splits_multibyte};

    #[test]
    fn chunking_covers_every_byte_exactly_once() {
        let body = "abcdefghijklmnopqrstuvwxyz0123456789";
        assert_eq!(boundaries(body.len()).last().copied(), Some(body.len()));
        let framed = chunk_framed(body.as_bytes());
        assert!(framed.ends_with(b"0\r\n\r\n"));
        assert!(framed.starts_with(b"1\r\na\r\n7\r\nbcdefgh\r\n"));
        assert!(chunk_framed(b"").starts_with(b"0\r\n\r\n"));
    }

    #[test]
    fn chunking_splits_multibyte_sequences() {
        let body = "SANITIZED_FIXTURE_TEXT_é☃𝄞";
        assert!(has_multibyte(body));
        assert!(splits_multibyte(body));
        assert!(!splits_multibyte("SANITIZED_FIXTURE_TEXT"));
        assert!(!has_multibyte("SANITIZED_FIXTURE_TEXT"));
    }
}
