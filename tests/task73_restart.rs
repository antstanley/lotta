use futures_util::{SinkExt, StreamExt};
use lotta_domain::{
    Agent, AgentId, BoundedJsonValue, Conversation, ConversationId, NonEmptyString, RunId,
    RuntimeScope, Timestamp,
};
use lotta_runtime::ports::{AgentStore, ConversationStore, ToolInputSchema, ValidatedToolInput};
use lotta_runtime::{
    ApprovalManager, ApprovalRequest, ApprovalResolution, ApprovalResolutionInput, ApprovalState,
    EditedInputValidator, RuntimeError,
};
use lotta_store::{LocalStore, StorePaths};
use serde_json::{Value, json};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Validator;
impl EditedInputValidator for Validator {
    fn validate(
        &self,
        _: &ApprovalRequest,
        input: BoundedJsonValue,
    ) -> Result<BoundedJsonValue, RuntimeError> {
        Ok(input)
    }
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct RestartTestLock(std::fs::File);

impl RestartTestLock {
    fn acquire() -> Self {
        let path = std::env::temp_dir().join("lotta-task73-restart-tests.lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .expect("open restart test lock");
        file.lock().expect("acquire restart test lock");
        Self(file)
    }
}

impl Drop for RestartTestLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

struct TempRoot(std::path::PathBuf);
impl TempRoot {
    fn new() -> Self {
        let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("lotta-task73-restart-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("workspace")).expect("create restart root");
        std::fs::create_dir_all(path.join("pi-ai")).expect("create inert provider package");
        let bun = path.join("inert-bun");
        std::fs::write(&bun, "#!/bin/sh\nexit 1\n").expect("create inert Bun executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(&bun).expect("Bun metadata").permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&bun, permissions).expect("mark inert Bun executable");
        }
        Self(path.canonicalize().expect("canonical restart root"))
    }
}
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent-task73-restart").expect("agent id"),
        ConversationId::accept("conversation-task73-restart").expect("conversation id"),
        None,
    )
}

async fn seed(paths: &StorePaths, owner: &RuntimeScope) {
    let agent: Agent = serde_json::from_value(json!({
        "id": owner.agent_id.as_str(), "name": "Task73", "description": null,
        "system": "test", "tags": [], "model": "openai/gpt-5.4",
        "model_settings": {}, "hidden": false, "compaction_settings": null
    }))
    .expect("agent fixture");
    let conversation: Conversation = serde_json::from_value(json!({
        "id": owner.conversation_id.as_str(), "agent_id": owner.agent_id.as_str(),
        "archived": false, "created_at": "2026-08-18T00:00:00Z",
        "updated_at": "2026-08-18T00:00:00Z", "last_message_at": null,
        "summary": null, "in_context_message_ids": [], "model": null,
        "model_settings": null, "context_window_limit": null, "hidden": false,
        "tags": []
    }))
    .expect("conversation fixture");
    let store = LocalStore::new(paths.clone());
    AgentStore::save(&store, &agent).await.expect("save agent");
    ConversationStore::save(&store, &conversation)
        .await
        .expect("save conversation");
    manager(paths)
        .store_request(approval(owner.clone()))
        .expect("store pending approval");
}

async fn seed_executing(paths: &StorePaths, owner: &RuntimeScope) {
    seed(paths, owner).await;
    let manager = manager(paths);
    let request = manager
        .get_request(owner, &text("approval-process-restart"))
        .expect("load pending")
        .expect("pending approval");
    manager
        .resolve(
            &ApprovalResolutionInput {
                scope: owner.clone(),
                request_id: request.request_id.clone(),
                tool_call_id: request.tool_call_id.clone(),
                lease_generation: request.lease_generation,
                revision: request.revision,
                resolution: ApprovalResolution::Allow,
                edited_input: None,
            },
            request.lease_generation,
        )
        .expect("claim executing approval");
}

fn manager(paths: &StorePaths) -> ApprovalManager {
    ApprovalManager::new(
        LocalStore::new(paths.clone()).approval_journal(),
        Arc::new(Validator),
    )
}

fn approval(owner: RuntimeScope) -> ApprovalRequest {
    ApprovalRequest {
        request_id: text("approval-process-restart"),
        tool_call_id: text("call-process-restart"),
        scope: owner,
        run_id: RunId::accept("run-process-restart").expect("run id"),
        turn_id: text("turn-process-restart"),
        input_id: text("input-process-restart"),
        lease_generation: 7,
        tool_name: text("Read"),
        original_input: ValidatedToolInput::new(
            BoundedJsonValue::new(json!({"path":"a"})).expect("bounded input"),
        )
        .expect("validated input"),
        original_schema: ToolInputSchema::new(
            BoundedJsonValue::new(json!({"type":"object"})).expect("bounded schema"),
        )
        .expect("tool schema"),
        created_at: Timestamp::parse_persisted_rfc3339("2026-08-18T00:00:00Z").expect("created at"),
        expires_at: Timestamp::parse_persisted_rfc3339("2099-08-18T00:00:00Z").expect("expires at"),
        state: ApprovalState::Pending,
        revision: 0,
    }
}

fn text(value: &str) -> NonEmptyString {
    NonEmptyString::new(value.to_owned()).expect("nonempty")
}

fn spawn(root: &std::path::Path, port: u16) -> ChildGuard {
    let child = Command::new(env!("CARGO_BIN_EXE_lotta"))
        .args([
            "server",
            "--backend",
            "local",
            "--listen",
            &format!("ws://127.0.0.1:{port}/ws"),
            "--storage-dir",
            root.to_str().expect("storage path"),
            "--workspace-dir",
            root.join("workspace").to_str().expect("workspace path"),
        ])
        .env("LOTTA_BUN", root.join("inert-bun"))
        .env("LOTTA_PI_AI_ROOT", root.join("pi-ai"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn production server");
    ChildGuard(child)
}

async fn connect(url: &str) -> Socket {
    for _ in 0..200 {
        if let Ok((socket, _)) = tokio_tungstenite::connect_async(url).await {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("production server did not bind");
}

async fn send(socket: &mut Socket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("send websocket frame");
}

fn approval_diagnostic(paths: &StorePaths, owner: &RuntimeScope) -> String {
    match manager(paths).get_request(owner, &text("approval-process-restart")) {
        Ok(Some(request)) => format!(
            "state={:?}, revision={}, lease_generation={}",
            request.state, request.revision, request.lease_generation
        ),
        Ok(None) => "approval row absent".to_owned(),
        Err(error) => format!("approval read failed: {error:?}"),
    }
}

async fn receive(
    socket: &mut Socket,
    phase: &str,
    paths: &StorePaths,
    owner: &RuntimeScope,
) -> Value {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match socket
                .next()
                .await
                .expect("socket open")
                .expect("socket frame")
            {
                Message::Text(body) => return serde_json::from_str(&body).expect("json frame"),
                Message::Ping(body) => socket.send(Message::Pong(body)).await.expect("pong"),
                _ => {}
            }
        }
    })
    .await;
    result.unwrap_or_else(|_| {
        panic!(
            "websocket frame timeout during {phase}; {}",
            approval_diagnostic(paths, owner)
        )
    })
}

async fn start_and_replay(
    socket: &mut Socket,
    owner: &RuntimeScope,
    paths: &StorePaths,
    phase: &str,
) {
    start_runtime(socket, owner, paths, phase).await;
    let replay = receive(
        socket,
        &format!("{phase} control_request replay"),
        paths,
        owner,
    )
    .await;
    assert_eq!(replay["type"], "control_request");
    assert_eq!(replay["request_id"], "approval-process-restart");
}

fn assert_pending_revision(paths: &StorePaths, owner: &RuntimeScope, revision: u64) {
    let pending = manager(paths)
        .get_request(owner, &text("approval-process-restart"))
        .expect("load replayed approval")
        .expect("replayed approval retained");
    assert_eq!(pending.state, ApprovalState::Pending);
    assert_eq!(pending.revision, revision);
}

async fn start_runtime(socket: &mut Socket, owner: &RuntimeScope, paths: &StorePaths, phase: &str) {
    send(
        socket,
        json!({
            "type":"runtime_start", "request_id":"restart", "agent_id":owner.agent_id,
            "conversation_id":owner.conversation_id, "recover_approvals":true
        }),
    )
    .await;
    let response = receive(
        socket,
        &format!("{phase} runtime_start_response"),
        paths,
        owner,
    )
    .await;
    assert_eq!(response["type"], "runtime_start_response");
    assert_eq!(response["success"], true);
}

fn assert_runtime_input_wire(value: &Value) {
    let frame = lotta_app_server::framing::decode_text(&value.to_string())
        .expect("bounded approval response frame");
    let decoded =
        lotta_app_server::ws::command::decode(&frame).expect("typed approval response command");
    assert!(
        matches!(
            decoded,
            Some(lotta_app_server::ws::RuntimeCommand::Input(_))
        ),
        "approval response classification: {:?}",
        frame.effects.outcome
    );
}

async fn resolve_restarted_pending(
    socket: &mut Socket,
    paths: &StorePaths,
    owner: &RuntimeScope,
    case: &str,
    decision: Value,
    expected_state: &str,
) {
    let response = json!({
        "type":"input", "request_id":"resolve-once", "runtime":owner,
        "payload":{
            "kind":"approval_response", "request_id":"approval-process-restart",
            "decision":decision
        }
    });
    assert_runtime_input_wire(&response);
    send(socket, response.clone()).await;
    assert_eq!(
        receive(
            socket,
            &format!("{case} resolution input_accepted"),
            paths,
            owner,
        )
        .await["accepted"],
        true
    );
    let recovery = receive(socket, &format!("{case} approval_recovery"), paths, owner).await;
    assert_eq!(recovery["type"], "approval_recovery");
    assert_eq!(recovery["state"], expected_state);
    assert_eq!(
        receive(
            socket,
            &format!("{case} recovery turn_finished"),
            paths,
            owner,
        )
        .await["stop_reason"],
        "approval_recovery_interrupted"
    );
    send(socket, response).await;
    let duplicate = receive(
        socket,
        &format!("{case} duplicate input rejection"),
        paths,
        owner,
    )
    .await;
    assert_eq!(duplicate["type"], "input_accepted");
    assert_eq!(duplicate["accepted"], false);
}

fn assert_pending_restart_terminal(paths: &StorePaths, owner: &RuntimeScope, expected_state: &str) {
    let terminal = manager(paths)
        .get_request(owner, &text("approval-process-restart"))
        .expect("load terminal");
    if expected_state == "denied" {
        assert!(terminal.is_none());
    } else {
        let terminal = terminal.expect("interrupted terminal retained");
        assert_eq!(terminal.state, ApprovalState::Interrupted);
        assert_eq!(terminal.revision, 4);
    }
}

async fn exercise_pending_restart(case: &str, decision: Value, expected_state: &str) {
    let root = TempRoot::new();
    let paths = StorePaths::new(root.0.clone()).expect("store paths");
    let owner = scope();
    seed(&paths, &owner).await;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("reserve port")
        .local_addr()
        .expect("local address")
        .port();
    let url = format!("ws://127.0.0.1:{port}/ws");
    let first = spawn(&root.0, port);
    let mut first_socket = connect(&url).await;
    start_and_replay(&mut first_socket, &owner, &paths, "first process").await;
    assert_pending_revision(&paths, &owner, 1);
    drop(first_socket);
    drop(first);

    let second = spawn(&root.0, port);
    let mut second_socket = connect(&url).await;
    start_and_replay(&mut second_socket, &owner, &paths, "second process").await;
    assert_pending_revision(&paths, &owner, 2);
    resolve_restarted_pending(
        &mut second_socket,
        &paths,
        &owner,
        case,
        decision,
        expected_state,
    )
    .await;
    drop(second_socket);
    drop(second);
    assert_pending_restart_terminal(&paths, &owner, expected_state);
}

#[tokio::test]
async fn real_process_restart_accepts_allow_deny_and_edit_exactly_once() {
    let _serial = RestartTestLock::acquire();
    for (case, decision, state) in [
        ("allow", json!({"behavior":"allow"}), "interrupted"),
        (
            "deny",
            json!({"behavior":"deny","message":"denied by restart test"}),
            "denied",
        ),
        (
            "edit",
            json!({"behavior":"allow","updated_input":{"path":"edited"}}),
            "interrupted",
        ),
    ] {
        exercise_pending_restart(case, decision, state).await;
    }
}

#[tokio::test]
async fn real_process_restart_interrupts_executing_without_replay() {
    let _serial = RestartTestLock::acquire();
    let root = TempRoot::new();
    let paths = StorePaths::new(root.0.clone()).expect("store paths");
    let owner = scope();
    seed_executing(&paths, &owner).await;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("reserve port")
        .local_addr()
        .expect("local address")
        .port();
    let server = spawn(&root.0, port);
    let mut socket = connect(&format!("ws://127.0.0.1:{port}/ws")).await;
    start_runtime(&mut socket, &owner, &paths, "executing recovery").await;
    let recovery = receive(&mut socket, "executing approval_recovery", &paths, &owner).await;
    assert_eq!(recovery["type"], "approval_recovery");
    assert_eq!(recovery["state"], "interrupted");
    assert_eq!(recovery["original_state"], "executing");
    drop(socket);
    drop(server);
    let terminal = manager(&paths)
        .get_request(&owner, &text("approval-process-restart"))
        .expect("load terminal")
        .expect("interrupted terminal retained");
    assert_eq!(terminal.state, ApprovalState::Interrupted);
    assert_eq!(terminal.revision, 2);
}
