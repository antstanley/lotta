use crate::normalize::{self, NormalizedResponse, Normalizer};
use crate::schema::{
    AUTHORIZATION, CHILD_TIMEOUT, CleanupDelta, ConversationDelta, ConversationLink, Execution,
    FORK_SOURCE_FIELD, Fixture, FixtureRequest, ForkLink, IdempotencyDelta, IdempotencyPhase,
    Observable, ProviderCall, RelationshipMap, TOKEN_SHA256,
};
use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use base64::Engine as _;
use lotta_domain::Agent;
use lotta_runtime::ports::AgentStore;
use lotta_store::{LocalStore, StorePaths};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

#[derive(Clone)]
struct ProviderState {
    requests: Arc<Mutex<Vec<Value>>>,
    arrived: Arc<Notify>,
    release: Arc<Notify>,
    held: Arc<AtomicBool>,
    count: Arc<AtomicUsize>,
}

pub struct ProviderStub {
    pub port: u16,
    state: ProviderState,
    task: tokio::task::JoinHandle<()>,
}

impl ProviderStub {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind provider stub");
        let port = listener.local_addr().expect("provider address").port();
        let state = ProviderState {
            requests: Arc::new(Mutex::new(Vec::new())),
            arrived: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            held: Arc::new(AtomicBool::new(false)),
            count: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/v1/chat/completions", post(provider_response))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve provider stub");
        });
        Self { port, state, task }
    }

    fn hold(&self) {
        self.state.held.store(true, Ordering::Release);
    }

    fn release(&self) {
        self.state.held.store(false, Ordering::Release);
        self.state.release.notify_waiters();
    }

    pub fn count(&self) -> usize {
        self.state.count.load(Ordering::Acquire)
    }

    async fn wait_count(&self, target: usize) {
        tokio::time::timeout(CHILD_TIMEOUT, async {
            loop {
                let notified = self.state.arrived.notified();
                if self.count() >= target {
                    return;
                }
                notified.await;
            }
        })
        .await
        .expect("provider request arrival");
    }

    fn requests_from(&self, before: usize) -> Vec<Value> {
        self.state
            .requests
            .lock()
            .expect("provider requests")
            .iter()
            .skip(before)
            .cloned()
            .collect()
    }
}

impl Drop for ProviderStub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn provider_response(
    State(state): State<ProviderState>,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    state
        .requests
        .lock()
        .expect("provider request")
        .push(request);
    state.count.fetch_add(1, Ordering::Release);
    state.arrived.notify_waiters();
    while state.held.load(Ordering::Acquire) {
        state.release.notified().await;
    }
    let body = concat!(
        "data: {\"id\":\"chatcmpl-11111111-1111-4111-8111-111111111111\",",
        "\"object\":\"chat.completion.chunk\",\"created\":1767225600,",
        "\"model\":\"fixture-model\",\"choices\":[{\"index\":0,\"delta\":",
        "{\"role\":\"assistant\",\"content\":\"<ASSISTANT_TEXT>\"},",
        "\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-11111111-1111-4111-8111-111111111111\",",
        "\"object\":\"chat.completion.chunk\",\"created\":1767225600,",
        "\"model\":\"fixture-model\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":0,",
        "\"completion_tokens\":0,\"total_tokens\":0}}\n\n",
        "data: [DONE]\n\n",
    );
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        body,
    )
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(case: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "lotta-openai-golden-{}-{case}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("store/workspace")).expect("create workspace");
        fs::create_dir_all(path.join("home")).expect("create home");
        fs::create_dir_all(path.join("pi-ai")).expect("create provider package root");
        Self(path.canonicalize().expect("canonical test root"))
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Harness {
    root: TempRoot,
    server: Server,
    pub provider: ProviderStub,
    client: reqwest::Client,
    stored_response_id: Option<String>,
    stored_conversation_id: Option<String>,
}

impl Harness {
    pub async fn start(case: &str) -> Self {
        let root = TempRoot::new(case);
        let provider = ProviderStub::start().await;
        seed(&root.0, provider.port).await;
        let server = spawn_server(&root.0);
        wait_ready(&server).await;
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("golden client");
        Self {
            root,
            server,
            provider,
            client,
            stored_response_id: None,
            stored_conversation_id: None,
        }
    }

    pub async fn shutdown(mut self) {
        let _ = self.server.child.kill();
        let _ = self.server.child.wait();
        self.provider.task.abort();
        let _ = (&mut self.provider.task).await;
    }

    pub async fn execute(&mut self, fixture: &Fixture) -> (Value, Observable, RelationshipMap) {
        let provider_before = self.provider.count();
        let before = StoreSnapshot::capture(&self.root.0);
        let mut normalizer = Normalizer::default();
        let execution = match &fixture.execution {
            Execution::Single { .. } => self.single(fixture, &mut normalizer).await,
            Execution::Repeat { count } => self.repeat(fixture, *count, &mut normalizer).await,
            Execution::IdempotentLiveJoin => self.live_join(fixture, &mut normalizer).await,
        };
        let after = StoreSnapshot::capture(&self.root.0);
        let observable = self.observable(
            provider_before,
            &before,
            &after,
            execution.phases,
            &mut normalizer,
        );
        (execution.value, observable, normalizer.relationships())
    }

    async fn single(&mut self, fixture: &Fixture, normalizer: &mut Normalizer) -> ExecutionResult {
        let output = self.send(&fixture.request, normalizer).await;
        if let Some(id) = output.stored_id {
            let decoded = assert_cursor(&id, &self.root.0);
            self.inspect_cursor(fixture, &id, &decoded, normalizer);
        }
        if matches!(
            fixture.execution,
            Execution::Single {
                previous_cursor: Some(_),
                ..
            }
        ) {
            assert_previous_fork(
                &self.root.0,
                self.stored_conversation_id
                    .as_deref()
                    .expect("cursor source"),
            );
        }
        ExecutionResult::ordinary(output.value)
    }

    fn inspect_cursor(
        &mut self,
        fixture: &Fixture,
        id: &str,
        decoded: &Value,
        normalizer: &mut Normalizer,
    ) {
        if matches!(
            fixture.execution,
            Execution::Single {
                capture_cursor: true,
                ..
            }
        ) {
            self.stored_conversation_id = decoded["conversation_id"].as_str().map(str::to_owned);
            self.stored_response_id = Some(id.to_owned());
        }
        if let Some(expected) = &fixture.cursor {
            let mut cursor = decoded.clone();
            normalizer.value(&mut cursor).expect("normalize cursor");
            assert_eq!(&cursor, expected, "cursor fixture");
        }
    }

    async fn repeat(
        &self,
        fixture: &Fixture,
        count: usize,
        normalizer: &mut Normalizer,
    ) -> ExecutionResult {
        assert!((1..=3).contains(&count), "repeat bound");
        let mut attempts = Vec::with_capacity(count);
        for _ in 0..count {
            attempts.push(self.send(&fixture.request, normalizer).await.value);
        }
        ExecutionResult::ordinary(json!({"attempts":attempts}))
    }

    async fn live_join(&self, fixture: &Fixture, normalizer: &mut Normalizer) -> ExecutionResult {
        self.provider.hold();
        let target = self.provider.count() + 1;
        let first = self.send_raw(&fixture.request).await;
        self.provider.wait_count(target).await;
        let owner = StoreSnapshot::capture(&self.root.0);
        let joined = self.send_raw(&fixture.request).await;
        let joined_snapshot = StoreSnapshot::capture(&self.root.0);
        assert_eq!(joined.status(), StatusCode::OK, "live join accepted");
        assert_eq!(self.provider.count(), target, "live join provider count");
        assert_eq!(owner, joined_snapshot, "live join changed durable input");
        self.provider.release();
        let first = finish_sse(first, normalizer, "first SSE").await;
        let joined = finish_sse(joined, normalizer, "joined SSE").await;
        let settled_before = StoreSnapshot::capture(&self.root.0);
        let settled = self.send(&fixture.request, normalizer).await.value;
        let settled_after = StoreSnapshot::capture(&self.root.0);
        assert_eq!(self.provider.count(), target, "settled provider count");
        assert_eq!(
            settled_before, settled_after,
            "settled replay changed store"
        );
        ExecutionResult {
            value: json!({"attempts":[first, joined, settled]}),
            phases: vec![
                IdempotencyPhase::OwnerActive,
                IdempotencyPhase::LiveJoin,
                IdempotencyPhase::SettledReplay,
            ],
        }
    }

    async fn send(
        &self,
        request: &FixtureRequest,
        normalizer: &mut Normalizer,
    ) -> NormalizedResponse {
        let response = self.send_raw(request).await;
        normalize::response(response, request.mode == "sse", normalizer)
            .await
            .expect("normalize response")
    }

    async fn send_raw(&self, request: &FixtureRequest) -> reqwest::Response {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).expect("method");
        let url = format!("http://127.0.0.1:{}{}", self.server.port, request.path);
        let mut builder = self
            .client
            .request(method, url)
            .header("authorization", AUTHORIZATION);
        for (name, value) in &request.headers {
            if name != "authorization" {
                builder = builder.header(name, value);
            }
        }
        builder = materialize_body(builder, request, self.stored_response_id.as_ref());
        tokio::time::timeout(CHILD_TIMEOUT, builder.send())
            .await
            .expect("HTTP timeout")
            .expect("HTTP request")
    }

    fn observable(
        &self,
        provider_before: usize,
        before: &StoreSnapshot,
        after: &StoreSnapshot,
        phases: Vec<IdempotencyPhase>,
        normalizer: &mut Normalizer,
    ) -> Observable {
        let calls = self
            .provider
            .requests_from(provider_before)
            .iter()
            .map(canonical_provider_call)
            .collect::<Vec<_>>();
        let conversations = conversation_delta(before, after, normalizer);
        let allocations = allocated_ids(before.sequence, after.sequence).len();
        let admissions = role_delta(before, after, "user");
        let turns = role_delta(before, after, "assistant");
        let live_joins = usize::from(phases.contains(&IdempotencyPhase::LiveJoin));
        Observable {
            provider_calls: calls.clone(),
            cleanup: CleanupDelta {
                ephemeral_deleted: conversations.deleted.len(),
            },
            conversations,
            idempotency: IdempotencyDelta {
                allocations,
                admissions,
                turns,
                provider_calls: calls.len(),
                live_joins,
                phases,
            },
        }
    }
}

struct ExecutionResult {
    value: Value,
    phases: Vec<IdempotencyPhase>,
}

impl ExecutionResult {
    fn ordinary(value: Value) -> Self {
        Self {
            value,
            phases: vec![IdempotencyPhase::NotApplicable],
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct StoreSnapshot {
    sequence: u64,
    records: BTreeMap<String, Value>,
    transcripts: BTreeMap<String, Vec<Value>>,
}

impl StoreSnapshot {
    fn capture(root: &Path) -> Self {
        let records = conversation_records(root);
        let transcripts = records
            .keys()
            .map(|id| (id.clone(), transcript_records(root, id)))
            .collect();
        Self {
            sequence: allocation_sequence(root),
            records,
            transcripts,
        }
    }
}

async fn finish_sse(
    response: reqwest::Response,
    normalizer: &mut Normalizer,
    context: &str,
) -> Value {
    normalize::response(response, true, normalizer)
        .await
        .unwrap_or_else(|error| panic!("{context}: {error}"))
        .value
}

fn materialize_body(
    mut builder: reqwest::RequestBuilder,
    request: &FixtureRequest,
    stored: Option<&String>,
) -> reqwest::RequestBuilder {
    if let Some(mut body) = request.body.clone() {
        if body.get("previous_response_id").and_then(Value::as_str)
            == Some("<FROM:responses_stored_json>")
        {
            body["previous_response_id"] = json!(stored.expect("cursor dependency"));
        }
        builder = builder.json(&body);
    }
    builder
}

fn canonical_provider_call(value: &Value) -> ProviderCall {
    let mut inputs = Vec::new();
    collect_placeholders(value.get("messages").unwrap_or(&Value::Null), &mut inputs);
    ProviderCall {
        model: value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        inputs,
        stream: value.get("stream") == Some(&Value::Bool(true)),
        store: value.get("store") == Some(&Value::Bool(true)),
    }
}

fn collect_placeholders(value: &Value, output: &mut Vec<String>) {
    let mut stack = vec![value];
    while let Some(current) = stack.pop() {
        match current {
            Value::String(text) if approved_placeholder(text) => output.push(text.clone()),
            Value::Array(values) => stack.extend(values.iter().rev()),
            Value::Object(values) => stack.extend(values.values().rev()),
            _ => {}
        }
    }
}

fn approved_placeholder(value: &str) -> bool {
    value.starts_with('<')
        && value.ends_with('>')
        && value[1..value.len() - 1].bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b':')
        })
}

fn conversation_delta(
    before: &StoreSnapshot,
    after: &StoreSnapshot,
    normalizer: &mut Normalizer,
) -> ConversationDelta {
    let ids = allocated_ids(before.sequence, after.sequence);
    let created = ids
        .iter()
        .map(|id| conversation_link(id, after.records.get(id)))
        .collect::<Vec<_>>();
    let retained = created
        .iter()
        .filter(|link| after.records.contains_key(&link.id))
        .cloned()
        .collect::<Vec<_>>();
    let deleted = ids
        .iter()
        .filter(|id| !after.records.contains_key(*id))
        .cloned()
        .collect::<Vec<_>>();
    let hidden = retained
        .iter()
        .filter(|link| link.hidden == Some(true))
        .cloned()
        .collect::<Vec<_>>();
    let forks = retained
        .iter()
        .filter_map(|link| {
            link.source_id.as_ref().map(|source| ForkLink {
                source_id: source.clone(),
                target_id: link.id.clone(),
            })
        })
        .collect::<Vec<_>>();
    normalize_delta(
        ConversationDelta {
            created,
            deleted,
            retained,
            hidden,
            forks,
        },
        normalizer,
    )
}

fn allocated_ids(before: u64, after: u64) -> Vec<String> {
    assert!(after >= before, "allocation sequence regressed");
    ((before + 1)..=after)
        .map(|sequence| format!("local-conv-{sequence}"))
        .collect()
}

fn conversation_link(id: &str, record: Option<&Value>) -> ConversationLink {
    ConversationLink {
        id: id.to_owned(),
        agent_id: record
            .and_then(|value| value.get("agent_id"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        hidden: record
            .and_then(|value| value.get("hidden"))
            .and_then(Value::as_bool),
        source_id: record
            .and_then(|value| value.get(FORK_SOURCE_FIELD))
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn normalize_delta(delta: ConversationDelta, normalizer: &mut Normalizer) -> ConversationDelta {
    let mut value = serde_json::to_value(delta).expect("delta value");
    normalizer.value(&mut value).expect("normalize delta");
    serde_json::from_value(value).expect("normalized delta")
}

fn conversation_records(root: &Path) -> BTreeMap<String, Value> {
    let directory = root.join("store/conversations");
    let mut records = BTreeMap::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return records;
    };
    for entry in entries.flatten() {
        let path = entry.path().join("conversation.json");
        let Ok(bytes) = fs::read(path) else { continue };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            records.insert(id.to_owned(), value);
        }
    }
    records
}

fn allocation_sequence(root: &Path) -> u64 {
    let path = root.join("store/.conversation-sequence");
    match fs::read_to_string(path) {
        Ok(value) => value.trim().parse().expect("conversation sequence"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("conversation sequence: {error}"),
    }
}

fn transcript_records(root: &Path, conversation: &str) -> Vec<Value> {
    let key = format!("conversation:{conversation}");
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
    let path = root
        .join("store/conversations")
        .join(encoded)
        .join("messages.jsonl");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(|line| serde_json::from_str(line).expect("canonical transcript row"))
        .collect()
}

fn role_delta(before: &StoreSnapshot, after: &StoreSnapshot, role: &str) -> usize {
    let old = before
        .transcripts
        .values()
        .map(|rows| count_role(rows, role))
        .sum::<usize>();
    let new = after
        .transcripts
        .values()
        .map(|rows| count_role(rows, role))
        .sum::<usize>();
    new.saturating_sub(old)
}

fn count_role(rows: &[Value], wanted: &str) -> usize {
    rows.iter().filter(|row| contains_role(row, wanted)).count()
}

fn contains_role(value: &Value, wanted: &str) -> bool {
    match value {
        Value::Object(fields) => {
            fields.get("role").and_then(Value::as_str) == Some(wanted)
                || fields.values().any(|child| contains_role(child, wanted))
        }
        Value::Array(values) => values.iter().any(|child| contains_role(child, wanted)),
        _ => false,
    }
}

fn assert_cursor(id: &str, root: &Path) -> Value {
    let payload = id
        .strip_prefix("resp_letta_")
        .expect("stored cursor prefix");
    assert!(!payload.contains('='), "cursor is unpadded");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("cursor base64");
    let value: Value = serde_json::from_slice(&bytes).expect("cursor JSON");
    validate_cursor_shape(&value);
    let conversation = value["conversation_id"]
        .as_str()
        .expect("cursor conversation");
    let record = conversation_records(root)
        .remove(conversation)
        .expect("canonical cursor record");
    assert_eq!(record["id"], value["conversation_id"]);
    assert_eq!(record["agent_id"], value["agent_id"]);
    value
}

fn validate_cursor_shape(value: &Value) {
    assert_eq!(value.as_object().expect("cursor object").len(), 4);
    assert_eq!(value["version"], 1);
    let nonce = value["nonce"].as_str().expect("cursor nonce");
    let parsed = uuid::Uuid::parse_str(nonce).expect("nonce UUID");
    assert_eq!(parsed.get_version_num(), 4);
    assert_eq!(parsed.hyphenated().to_string(), nonce);
}

fn assert_previous_fork(root: &Path, source: &str) {
    let snapshot = StoreSnapshot::capture(root);
    let (_, record) = snapshot.records.last_key_value().expect("fork record");
    assert_eq!(record["hidden"], true, "previous response fork visibility");
    assert_eq!(
        record[FORK_SOURCE_FIELD], source,
        "previous response raw cursor source"
    );
}

async fn seed(root: &Path, provider_port: u16) {
    let store_root = root.join("store");
    fs::create_dir_all(store_root.join("providers")).expect("provider directory");
    let provider = json!({"version":1,"providers":{"fixture-provider":{
        "id":"local-provider-fixture-provider","name":"fixture-provider",
        "provider_type":"openai-compatible","provider_category":"byok",
        "auth":{"type":"api","key":"fixture-provider-credential"},
        "base_url":format!("http://127.0.0.1:{provider_port}/v1/"),
        "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}}});
    fs::write(
        store_root.join("providers/auth.json"),
        serde_json::to_vec_pretty(&provider).expect("provider JSON"),
    )
    .expect("provider auth");
    let store = LocalStore::new(StorePaths::new(&store_root).expect("store paths"));
    for value in agent_values() {
        let agent: Agent = serde_json::from_value(value).expect("agent fixture");
        AgentStore::save(&store, &agent).await.expect("save agent");
    }
}

fn agent_values() -> [Value; 4] {
    [
        agent_value(
            "agent-local-visible",
            "fixture-visible",
            false,
            "2026-01-01T00:00:00Z",
        ),
        agent_value(
            "agent-local-collision-a",
            "fixture-collision",
            false,
            "2026-01-02T00:00:00Z",
        ),
        agent_value(
            "agent-local-collision-b",
            "fixture-collision",
            false,
            "2026-01-03T00:00:00Z",
        ),
        agent_value(
            "agent-local-hidden",
            "fixture-hidden",
            true,
            "2026-01-04T00:00:00Z",
        ),
    ]
}

fn agent_value(id: &str, name: &str, hidden: bool, created_at: &str) -> Value {
    json!({"id":id,"name":name,"description":null,"system":"","tags":[],
        "model":"local-provider-fixture-provider/default","model_settings":{},
        "hidden":hidden,"compaction_settings":null,"created_at":created_at})
}

fn spawn_server(root: &Path) -> Server {
    let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
    let port = reservation.local_addr().expect("reserved address").port();
    drop(reservation);
    let child = Command::new(env!("CARGO_BIN_EXE_lotta"))
        .args([
            "server",
            "--backend",
            "local",
            "--listen",
            &format!("ws://127.0.0.1:{port}/ws"),
            "--openai-api",
            "--ws-auth",
            "capability-token",
            "--ws-token-sha256",
            TOKEN_SHA256,
            "--storage-dir",
            root.join("store").to_str().expect("storage path"),
            "--workspace-dir",
            root.join("store/workspace")
                .to_str()
                .expect("workspace path"),
        ])
        .env("HOME", root.join("home"))
        .env("LOTTA_BUN", "/Users/stan/.bun/bin/bun")
        .env("LOTTA_PI_AI_ROOT", root.join("pi-ai"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn production lotta --openai-api");
    Server { child, port }
}

async fn wait_ready(server: &Server) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("readiness client");
    tokio::time::timeout(CHILD_TIMEOUT, async {
        loop {
            if client
                .get(format!("http://127.0.0.1:{}/v1/models", server.port))
                .header("authorization", AUTHORIZATION)
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("production readiness");
}
