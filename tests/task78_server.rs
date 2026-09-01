use lotta_runtime::ports::{AgentStore, ConversationStore};
use lotta_store::{LocalStore, StorePaths};
use std::{
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

const CAPTURE_BYTES_MAX: usize = 256 * 1024;
const CAPTURE_LINE_BYTES_MAX: usize = 16 * 1024;

type Capture = std::sync::Arc<std::sync::Mutex<Vec<u8>>>;

struct ServerGuard {
    child: Option<std::process::Child>,
    readers: Vec<std::thread::JoinHandle<()>>,
    stdout: Capture,
    stderr: Capture,
}

impl ServerGuard {
    fn spawn(mut command: Command, pid_sender: std::sync::mpsc::Sender<u32>) -> Self {
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let stdout_capture = Capture::default();
        let stderr_capture = Capture::default();
        let stdout_observed = std::sync::Arc::clone(&stdout_capture);
        let stderr_observed = std::sync::Arc::clone(&stderr_capture);
        let stdout_reader = std::thread::spawn(move || {
            drain_bounded(stdout, stdout_observed, Some(pid_sender));
        });
        let stderr_reader = std::thread::spawn(move || {
            drain_bounded(stderr, stderr_observed, None);
        });
        Self {
            child: Some(child),
            readers: vec![stdout_reader, stderr_reader],
            stdout: stdout_capture,
            stderr: stderr_capture,
        }
    }

    fn id(&self) -> u32 {
        self.child.as_ref().unwrap().id()
    }

    fn finish(&mut self) -> std::process::ExitStatus {
        let status = self.child.as_mut().unwrap().wait().unwrap();
        self.child.take();
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        status
    }

    fn diagnostics(&self) -> String {
        format!(
            "bounded stdout:\n{}\nbounded stderr:\n{}",
            String::from_utf8_lossy(&self.stdout.lock().unwrap()),
            String::from_utf8_lossy(&self.stderr.lock().unwrap())
        )
    }

    fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().unwrap()).into_owned()
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

fn drain_bounded<R: std::io::Read>(
    mut reader: R,
    capture: Capture,
    pid_sender: Option<std::sync::mpsc::Sender<u32>>,
) {
    let mut chunk = [0_u8; 4 * 1024];
    let mut line = Vec::new();
    while let Ok(length) = reader.read(&mut chunk) {
        if length == 0 {
            break;
        }
        {
            let mut retained = capture.lock().unwrap();
            let remaining = CAPTURE_BYTES_MAX.saturating_sub(retained.len());
            retained.extend_from_slice(&chunk[..length.min(remaining)]);
        }
        if let Some(sender) = &pid_sender {
            for byte in &chunk[..length] {
                if *byte == b'\n' {
                    if let Ok(value) = std::str::from_utf8(&line)
                        && let Some(pid) = host_pid(value)
                    {
                        let _ = sender.send(pid);
                    }
                    line.clear();
                } else if line.len() < CAPTURE_LINE_BYTES_MAX {
                    line.push(*byte);
                }
            }
        }
    }
}

struct Root(std::path::PathBuf);
impl Root {
    fn new() -> Self {
        let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("lotta-task78-server-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for path in ["backend", "workspace", "pi-ai"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        let bun = root.join("inert-bun");
        std::fs::write(&bun, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(&bun).unwrap().permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&bun, permissions).unwrap();
        }
        seed_channels(&root);
        Self(root.canonicalize().unwrap())
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn seed_channels(root: &std::path::Path) {
    let channel = root.join("channels/telegram");
    std::fs::create_dir_all(&channel).unwrap();
    std::fs::write(channel.join("config.yaml"), "token: redacted\n").unwrap();
    std::fs::write(
        channel.join("accounts.json"),
        r#"{"accounts":[{"channel":"telegram","accountId":"main","enabled":true,"dmPolicy":"pairing","allowedUsers":[],"binding":{"agentId":null,"conversationId":null},"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}]}"#,
    )
    .unwrap();
    std::fs::write(
        channel.join("routing.yaml"),
        r#"{"routes":[{"accountId":"main","chatId":"chat","agentId":"agent-task78-server","conversationId":"default","enabled":true,"outboundEnabled":true,"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}]}"#,
    )
    .unwrap();
}

async fn seed_runtime(root: &std::path::Path) {
    let paths = StorePaths::new(root.to_path_buf()).unwrap();
    let store = LocalStore::new(paths);
    let agent: lotta_domain::Agent = serde_json::from_value(serde_json::json!({
        "id":"agent-task78-server", "name":"Task78", "description":null,
        "system":"test", "tags":[], "model":"openai/gpt-5.4", "model_settings":{},
        "hidden":false, "compaction_settings":null
    }))
    .unwrap();
    let conversation: lotta_domain::Conversation = serde_json::from_value(serde_json::json!({
        "id":"default", "agent_id":"agent-task78-server", "archived":false,
        "created_at":"2026-01-01T00:00:00Z", "updated_at":"2026-01-01T00:00:00Z",
        "last_message_at":null, "summary":null, "in_context_message_ids":[],
        "model":null, "model_settings":null, "context_window_limit":null,
        "hidden":false, "tags":[]
    }))
    .unwrap();
    AgentStore::save(&store, &agent).await.unwrap();
    ConversationStore::save(&store, &conversation)
        .await
        .unwrap();
}

fn unused_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn host_pid(line: &str) -> Option<u32> {
    line.strip_prefix("Channel host PID: ")?.parse().ok()
}

fn signal(pid: u32, name: &str) {
    assert!(
        Command::new("/bin/kill")
            .args([name, &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
}

fn process_exists(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn replacement_child(parent: u32, old: u32) -> Option<u32> {
    let output = Command::new("/usr/bin/pgrep")
        .args(["-P", &parent.to_string()])
        .output()
        .ok()?;
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .find(|pid| *pid != old)
}

fn channel_events(output: &str) -> Vec<serde_json::Value> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("Channel event: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn connected(output: &str, pid: u32) -> bool {
    channel_events(output)
        .iter()
        .any(|event| event["event"] == "child_connected" && event["pid"] == u64::from(pid))
}

fn generation_event(output: &str, generation: u64, name: &str) -> bool {
    channel_events(output)
        .iter()
        .any(|event| event["generation"] == generation && event["event"] == name)
}

fn assert_full_o6_events(
    output: &str,
    server_pid: u32,
    generation_one_pid: u32,
    generation_two_pid: u32,
) {
    let events = channel_events(output);
    let exact_channels = serde_json::json!([{
        "id": "telegram",
        "enabled": true,
        "accounts": 1,
        "routes": 1,
        "pending_pairings": 0,
        "targets": 0
    }]);
    let position = |generation: u64, name: &str| {
        events
            .iter()
            .position(|event| event["generation"] == generation && event["event"] == name)
            .unwrap_or_else(|| panic!("missing {name} for generation {generation}: {output}"))
    };
    for (generation, pid) in [(1, generation_one_pid), (2, generation_two_pid)] {
        let connected = &events[position(generation, "child_connected")];
        assert_eq!(connected["pid"], u64::from(pid));
        assert_eq!(
            connected["owner"],
            format!("channel-host-{server_pid}-{generation}")
        );
        let isolation = &events[position(generation, "startup_isolation")];
        assert_eq!(isolation["channels_write"], true);
        assert_eq!(isolation["backend_sibling_write_denied"], true);
        assert_eq!(
            events[position(generation, "channels_response")]["channels"],
            exact_channels
        );
        let publication = &events[position(generation, "runtime_tools_published")];
        assert_eq!(
            publication["runtime"],
            serde_json::json!({
                "agent_id": "agent-task78-server",
                "conversation_id": "default"
            })
        );
        assert_eq!(
            publication["manager_tools"],
            serde_json::json!(["ChannelHostAvailability"])
        );
        let message_channel = &events[position(generation, "message_channel_registered")];
        assert_eq!(message_channel["runtime"], publication["runtime"]);
        assert_eq!(
            message_channel["manager_tools"],
            serde_json::json!(["MessageChannel"])
        );
    }
    let release_one = position(1, "runtime_tools_released");
    assert_eq!(events[release_one]["released"], 1);
    let revoke_one = position(1, "capability_revoked");
    let reap_one = position(1, "child_reaped");
    let publish_two = position(2, "runtime_tools_published");
    assert!(release_one < revoke_one && revoke_one < reap_one && reap_one < publish_two);
    let release_two = position(2, "runtime_tools_released");
    assert_eq!(events[release_two]["released"], 1);
    assert!(
        release_two < position(2, "capability_revoked")
            && position(2, "capability_revoked") < position(2, "child_reaped")
    );
}

#[tokio::test]
async fn full_built_server_auto_spawns_rotates_and_reaps_channel_child() {
    let root = Root::new();
    seed_runtime(&root.0).await;
    let port = unused_port();
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut command = Command::new(env!("CARGO_BIN_EXE_lotta"));
    command
        .args([
            "server",
            "--backend",
            "local",
            "--listen",
            &format!("ws://127.0.0.1:{port}/ws"),
            "--storage-dir",
            root.0.to_str().unwrap(),
            "--workspace-dir",
            root.0.join("workspace").to_str().unwrap(),
        ])
        .env("LETTA_HOME", &root.0)
        .env("LOTTA_BUN", root.0.join("inert-bun"))
        .env("LOTTA_PI_AI_ROOT", root.0.join("pi-ai"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut server = ServerGuard::spawn(command, sender);
    let server_pid = server.id();
    let generation_one = receiver.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(process_exists(generation_one));
    for _ in 0..400 {
        if generation_event(&server.stdout_text(), 1, "message_channel_registered") {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        generation_event(&server.stdout_text(), 1, "message_channel_registered"),
        "{}",
        server.diagnostics()
    );
    signal(generation_one, "-KILL");
    let generation_two = (0..400)
        .find_map(|_| {
            let pid = replacement_child(server.id(), generation_one);
            if let Some(candidate) = pid {
                std::thread::sleep(Duration::from_millis(200));
                if process_exists(candidate) {
                    return Some(candidate);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
            None
        })
        .expect("supervisor did not spawn generation two");
    assert_ne!(generation_one, generation_two);
    assert!(process_exists(generation_two));
    for _ in 0..400 {
        if connected(&server.stdout_text(), generation_two)
            && generation_event(&server.stdout_text(), 2, "message_channel_registered")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        connected(&server.stdout_text(), generation_two)
            && generation_event(&server.stdout_text(), 2, "message_channel_registered"),
        "{}",
        server.diagnostics()
    );
    for _ in 0..100 {
        if !process_exists(generation_one) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!process_exists(generation_one));
    signal(server.id(), "-INT");
    let status = server.finish();
    assert!(status.success(), "{}", server.diagnostics());
    assert!(!process_exists(generation_two), "{}", server.diagnostics());
    assert_full_o6_events(
        &server.stdout_text(),
        server_pid,
        generation_one,
        generation_two,
    );
}
