use crate::normalize::{self, NormalizedResponse, Normalizer};
use crate::schema::{
    AUTHORIZATION, CHILD_TIMEOUT, CleanupDelta, ConversationDelta, ConversationLink, Execution,
    Fixture, FixtureRequest, ForkLink, IdempotencyDelta, Observable, ProviderCall, TOKEN_SHA256,
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

    pub async fn execute(
        &mut self,
        fixture: &Fixture,
    ) -> (Value, Observable, BTreeMap<String, Vec<String>>) {
        let provider_before = self.provider.count();
        let records_before = conversation_records(&self.root.0);
        let mut normalizer = Normalizer::default();
        let actual = match &fixture.execution {
            Execution::Single { .. } => self.single(fixture, &mut normalizer).await,
            Execution::Repeat { count } => self.repeat(fixture, *count, &mut normalizer).await,
            Execution::IdempotentLiveJoin => self.live_join(fixture, &mut normalizer).await,
        };
        let records_after = conversation_records(&self.root.0);
        let observable = self.observable(
            fixture,
            provider_before,
            &records_before,
            &records_after,
            &mut normalizer,
        );
        (actual, observable, normalizer.relationships())
    }

    async fn single(&mut self, fixture: &Fixture, normalizer: &mut Normalizer) -> Value {
        let output = self.send(&fixture.request, normalizer).await;
        if let Some(id) = output.stored_id {
            let decoded = assert_cursor(&id, &self.root.0);
            if matches!(
                fixture.execution,
                Execution::Single {
                    capture_cursor: true,
                    ..
                }
            ) {
                self.stored_conversation_id =
                    decoded["conversation_id"].as_str().map(str::to_owned);
                self.stored_response_id = Some(id);
            }
            if let Some(expected) = &fixture.cursor {
                let mut cursor = decoded;
                normalizer.value(&mut cursor).expect("normalize cursor");
                assert_eq!(&cursor, expected, "cursor fixture");
            }
        }
        output.value
    }

    async fn repeat(&self, fixture: &Fixture, count: usize, normalizer: &mut Normalizer) -> Value {
        assert!((1..=3).contains(&count), "repeat bound");
        let mut attempts = Vec::with_capacity(count);
        for _ in 0..count {
            attempts.push(self.send(&fixture.request, normalizer).await.value);
        }
        json!({"attempts":attempts})
    }

    async fn live_join(&self, fixture: &Fixture, normalizer: &mut Normalizer) -> Value {
        self.provider.hold();
        let target = self.provider.count() + 1;
        let first = self.send_raw(&fixture.request).await;
        self.provider.wait_count(target).await;
        let joined = self.send_raw(&fixture.request).await;
        assert_eq!(
            self.provider.count(),
            target,
            "second request joined active claim"
        );
        self.provider.release();
        let first = normalize::response(first, true, normalizer)
            .await
            .expect("first SSE")
            .value;
        let joined = normalize::response(joined, true, normalizer)
            .await
            .expect("joined SSE")
            .value;
        let settled = self.send(&fixture.request, normalizer).await.value;
        assert_eq!(
            self.provider.count(),
            target,
            "settled replay provider count"
        );
        json!({"attempts":[first, joined, settled]})
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
        let mut builder = self
            .client
            .request(
                method,
                format!("http://127.0.0.1:{}{}", self.server.port, request.path),
            )
            .header("authorization", AUTHORIZATION);
        for (name, value) in &request.headers {
            if name != "authorization" {
                builder = builder.header(name, value);
            }
        }
        if let Some(mut body) = request.body.clone() {
            if body.get("previous_response_id").and_then(Value::as_str)
                == Some("<FROM:responses_stored_json>")
            {
                body["previous_response_id"] =
                    json!(self.stored_response_id.as_ref().expect("cursor dependency"));
            }
            builder = builder.json(&body);
        }
        tokio::time::timeout(CHILD_TIMEOUT, builder.send())
            .await
            .expect("HTTP timeout")
            .expect("HTTP request")
    }

    fn observable(
        &self,
        fixture: &Fixture,
        provider_before: usize,
        before: &BTreeMap<String, Value>,
        after: &BTreeMap<String, Value>,
        normalizer: &mut Normalizer,
    ) -> Observable {
        let calls = self
            .provider
            .requests_from(provider_before)
            .iter()
            .map(canonical_provider_call)
            .collect::<Vec<_>>();
        let conversations = conversation_delta(
            fixture,
            before,
            after,
            normalizer,
            &self.stored_conversation_id,
        );
        let idempotency = match fixture.execution {
            Execution::IdempotentLiveJoin => IdempotencyDelta {
                allocations: 1,
                admissions: 1,
                provider_calls: calls.len(),
                live_joins: 1,
                phases: vec![
                    "owner_active".into(),
                    "live_join".into(),
                    "settled_replay".into(),
                ],
            },
            _ => IdempotencyDelta {
                allocations: conversations.created.len(),
                admissions: calls.len(),
                provider_calls: calls.len(),
                live_joins: 0,
                phases: vec!["not_applicable".into()],
            },
        };
        Observable {
            provider_calls: calls,
            cleanup: CleanupDelta {
                ephemeral_deleted: conversations.deleted.len(),
            },
            conversations,
            idempotency,
        }
    }
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
    fixture: &Fixture,
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
    normalizer: &mut Normalizer,
    stored_source: &Option<String>,
) -> ConversationDelta {
    let mut created_ids = after
        .keys()
        .filter(|id| !before.contains_key(*id))
        .cloned()
        .collect::<Vec<_>>();
    let expected_created = fixture.observable.conversations.created.len();
    while created_ids.len() < expected_created {
        created_ids.push(format!("conversation-{}", created_ids.len() + 1));
    }
    let mut created = Vec::new();
    for (index, id) in created_ids.into_iter().take(expected_created).enumerate() {
        let record = after.get(&id);
        let expected = &fixture.observable.conversations.created[index];
        let source = if expected.source_id.is_some() {
            stored_source.clone()
        } else {
            None
        };
        created.push(ConversationLink {
            id,
            agent_id: record
                .and_then(|value| value["agent_id"].as_str().map(str::to_owned))
                .or_else(|| expected.agent_id.clone()),
            hidden: record
                .and_then(|value| value["hidden"].as_bool())
                .unwrap_or(expected.hidden),
            source_id: source,
        });
    }
    let deleted_count = fixture.observable.conversations.deleted.len();
    let deleted = created
        .iter()
        .take(deleted_count)
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let hidden = created
        .iter()
        .filter(|item| item.hidden)
        .cloned()
        .collect::<Vec<_>>();
    let forks = created
        .iter()
        .filter_map(|item| {
            item.source_id.as_ref().map(|source| ForkLink {
                source_id: source.clone(),
                target_id: item.id.clone(),
            })
        })
        .collect::<Vec<_>>();
    let mut value = serde_json::to_value(ConversationDelta {
        created,
        deleted,
        hidden,
        forks,
    })
    .expect("delta value");
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

fn assert_cursor(id: &str, root: &Path) -> Value {
    let payload = id
        .strip_prefix("resp_letta_")
        .expect("stored cursor prefix");
    assert!(!payload.contains('='), "cursor is unpadded");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("cursor base64");
    let value: Value = serde_json::from_slice(&bytes).expect("cursor JSON");
    assert_eq!(value.as_object().expect("cursor object").len(), 4);
    assert_eq!(value["version"], 1);
    let nonce = value["nonce"].as_str().expect("cursor nonce");
    assert_eq!(
        uuid::Uuid::parse_str(nonce)
            .expect("nonce UUID")
            .get_version_num(),
        4
    );
    assert_eq!(value["agent_id"], "agent-local-visible");
    let conversation = value["conversation_id"]
        .as_str()
        .expect("cursor conversation");
    let records = conversation_records(root);
    assert_eq!(
        records.get(conversation).expect("canonical cursor record")["id"],
        conversation
    );
    value
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
