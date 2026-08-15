use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{Mutex, MutexGuard},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TOKEN: &str = "task14-external-capability-token";
const TOKEN_SHA256: &str = "8a222007f29b695e35260b6b7f477d0eced7ce256e5a6432bf25d6ce7cf1fb4d";
const DEADLINE: Duration = Duration::from_secs(5);
static SERIAL: Mutex<()> = Mutex::new(());

struct Server {
    child: Child,
    port: u16,
    token_file: Option<PathBuf>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(path) = &self.token_file {
            let _ = fs::remove_file(path);
        }
    }
}

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn reserve_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
    let port = listener.local_addr().expect("reserved address").port();
    (listener, port)
}

fn temp_token_file() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time after epoch")
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("lotta-task14-{}-{nonce}.token", std::process::id()));
    fs::write(&path, TOKEN).expect("write token file");
    path
}

fn spawn(args: &[String]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_lotta"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lotta")
}

fn wait_output(mut child: Child) -> Output {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if child.try_wait().expect("poll child").is_some() {
            return child.wait_with_output().expect("collect child output");
        }
        assert!(
            Instant::now() < deadline,
            "process did not exit by deadline"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn start_capability(use_digest: bool) -> Server {
    let (reservation, port) = reserve_port();
    let token_file = (!use_digest).then(temp_token_file);
    let mut args = vec![
        "server".into(),
        "--backend".into(),
        "local".into(),
        "--listen".into(),
        format!("ws://127.0.0.1:{port}"),
        "--openai-api".into(),
        "--ws-auth".into(),
        "capability-token".into(),
    ];
    if let Some(path) = &token_file {
        args.extend(["--ws-token-file".into(), path.display().to_string()]);
    } else {
        args.extend(["--ws-token-sha256".into(), TOKEN_SHA256.into()]);
    }
    drop(reservation);
    let child = spawn(&args);
    let mut server = Server {
        child,
        port,
        token_file,
    };
    wait_ready(&mut server);
    server
}

fn wait_ready(server: &mut Server) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        assert_eq!(
            server.child.try_wait().expect("poll server"),
            None,
            "server exited early"
        );
        if TcpStream::connect(("127.0.0.1", server.port)).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "server readiness deadline exceeded"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn request(port: u16, path: &str, authorization: Option<&str>, origin: Option<&str>) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout");
    let mut headers = format!(
        concat!(
            "GET {} HTTP/1.1\r\n",
            "Host: 127.0.0.1:{}\r\n",
            "Connection: Upgrade\r\n",
            "Upgrade: websocket\r\n",
            "Sec-WebSocket-Version: 13\r\n",
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        ),
        path, port
    );
    if let Some(value) = authorization {
        headers.push_str(&format!("Authorization: {value}\r\n"));
    }
    if let Some(value) = origin {
        headers.push_str(&format!("Origin: {value}\r\n"));
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes()).expect("write request");
    let mut bytes = Vec::with_capacity(512);
    loop {
        let mut chunk = [0_u8; 512];
        let count = stream.read(&mut chunk).expect("read response headers");
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() <= 8_192, "response headers exceeded test bound");
        if count == 0 || bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(bytes).expect("HTTP response is UTF-8")
}

fn status(response: &str) -> u16 {
    response
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status")
}

fn captured(server: &mut Server) -> (String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();
    server
        .child
        .stdout
        .as_mut()
        .expect("stdout pipe")
        .read_to_string(&mut stdout)
        .expect("read stdout");
    server
        .child
        .stderr
        .as_mut()
        .expect("stderr pipe")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    (stdout, stderr)
}

#[test]
fn nonloopback_without_auth_fails_before_bind() {
    let _guard = serial();
    let (reservation, port) = reserve_port();
    drop(reservation);
    let args = vec![
        "server".into(),
        "--backend".into(),
        "local".into(),
        "--listen".into(),
        format!("ws://0.0.0.0:{port}"),
    ];
    let output = wait_output(spawn(&args));
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr UTF-8");
    assert_eq!(
        stderr,
        concat!(
            "config_invalid: invalid server configuration: ",
            "non-loopback listeners require websocket authentication\n",
        )
    );
    let rebound = TcpListener::bind(("0.0.0.0", port)).expect("configuration failed before bind");
    drop(rebound);
}

#[test]
fn capability_file_listener_advertises_and_enforces_routes() {
    let _guard = serial();
    let mut server = start_capability(false);
    let bearer = format!("Bearer {TOKEN}");
    assert_eq!(
        status(&request(server.port, "/ws", Some(&bearer), None)),
        101
    );
    assert_eq!(status(&request(server.port, "/", Some(&bearer), None)), 101);
    assert_eq!(
        status(&request(
            server.port,
            "/ws",
            None,
            Some("http://example.test")
        )),
        401
    );
    assert_eq!(
        status(&request(server.port, "/ws", Some("Bearer incorrect"), None)),
        401
    );
    assert_eq!(
        status(&request(server.port, "/missing", Some(&bearer), None)),
        404
    );
    assert_eq!(status(&request(server.port, "/healthz", None, None)), 200);
    server.child.kill().expect("kill server");
    server.child.wait().expect("wait server");
    let (stdout, stderr) = captured(&mut server);
    let expected = format!(
        concat!(
            "Base URL: ws://127.0.0.1:{}\n",
            "WebSocket URL: ws://127.0.0.1:{}/ws\n",
            "OpenAI URL: http://127.0.0.1:{}/v1\n",
        ),
        server.port, server.port, server.port
    );
    assert_eq!(stdout, expected);
    assert!(!stdout.contains(TOKEN));
    assert!(!stderr.contains(TOKEN));
}

#[test]
fn capability_digest_listener_accepts_matching_token() {
    let _guard = serial();
    let mut server = start_capability(true);
    let bearer = format!("Bearer {TOKEN}");
    assert_eq!(
        status(&request(server.port, "/ws", Some(&bearer), None)),
        101
    );
    server.child.kill().expect("kill server");
    server.child.wait().expect("wait server");
    let (stdout, stderr) = captured(&mut server);
    assert!(!stdout.contains(TOKEN));
    assert!(!stderr.contains(TOKEN));
}

#[test]
fn invalid_cli_modes_exit_before_bind_with_safe_errors() {
    let _guard = serial();
    for extra in [
        vec!["--unknown"],
        vec![
            "--ws-auth",
            "capability-token",
            "--ws-token-file",
            "/tmp/a",
            "--ws-token-sha256",
            TOKEN_SHA256,
        ],
    ] {
        let (reservation, port) = reserve_port();
        drop(reservation);
        let mut args = vec![
            "server".into(),
            "--backend".into(),
            "local".into(),
            "--listen".into(),
            format!("ws://127.0.0.1:{port}"),
        ];
        args.extend(extra.into_iter().map(String::from));
        let output = wait_output(spawn(&args));
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8(output.stderr).expect("stderr UTF-8");
        assert!(stderr.starts_with("config_invalid: invalid server configuration: "));
        assert!(!stderr.contains(TOKEN));
        let rebound =
            TcpListener::bind(("127.0.0.1", port)).expect("CLI failure happened before bind");
        drop(rebound);
    }
}
