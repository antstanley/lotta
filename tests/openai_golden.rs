use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::post,
};
use base64::Engine as _;
use futures_util::StreamExt as _;
use lotta_domain::Agent;
use lotta_runtime::ports::AgentStore;
use lotta_store::{LocalStore, StorePaths};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

const BASELINE_COMMIT: &str = "300f923f16cc8eee50656d7da732902c1dea2b65";
const FIXTURE_CASES_MAX: usize = 16;
const FIXTURE_BYTES_MAX: usize = 1_048_576;
const SSE_EVENTS_MAX: usize = 64;
const JSON_DEPTH_MAX: usize = 32;
const CHILD_TIMEOUT: Duration = Duration::from_secs(15);
const READINESS_ATTEMPTS_MAX: usize = 300;
const READINESS_DELAY: Duration = Duration::from_millis(20);
const AUTHORIZATION: &str = "Bearer fixture-transport-credential";
const TOKEN_SHA256: &str = "1947de502481746d5dc98a64e8fa1d743d6c3da164f5b031cbf9ee9e0fb05ffb";

#[derive(Debug, Deserialize, Serialize)]
struct FixtureIndex {
    schema_version: u8,
    baseline_commit: String,
    bounds: FixtureBounds,
    cases: Vec<IndexCase>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FixtureBounds {
    cases_max: usize,
    fixture_bytes_max: usize,
    events_per_case_max: usize,
    json_depth_max: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct IndexCase {
    name: String,
    route: String,
    mode: String,
    dependencies: Vec<String>,
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Fixture {
    schema_version: u8,
    name: String,
    route: String,
    mode: String,
    dependencies: Vec<String>,
    request: FixtureRequest,
    expected: Value,
    observable: Value,
    cursor: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FixtureRequest {
    method: String,
    path: String,
    mode: String,
    headers: BTreeMap<String, String>,
    body: Option<Value>,
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

#[derive(Clone)]
struct ProviderState {
    requests: Arc<Mutex<Vec<Value>>>,
    arrived: Arc<Notify>,
    release: Arc<Notify>,
    hold: Arc<Mutex<bool>>,
}

struct ProviderStub {
    port: u16,
    state: ProviderState,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ProviderStub {
    fn drop(&mut self) {
        self.task.abort();
    }
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
            hold: Arc::new(Mutex::new(false)),
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

    fn count(&self) -> usize {
        self.state.requests.lock().expect("provider requests").len()
    }

    fn requests(&self) -> Vec<Value> {
        self.state
            .requests
            .lock()
            .expect("provider requests")
            .clone()
    }

    fn hold(&self) {
        *self.state.hold.lock().expect("provider hold") = true;
    }

    async fn wait_arrived(&self) {
        tokio::time::timeout(CHILD_TIMEOUT, self.state.arrived.notified())
            .await
            .expect("provider request arrival");
    }

    fn release(&self) {
        *self.state.hold.lock().expect("provider hold") = false;
        self.state.release.notify_waiters();
    }
}

async fn provider_response(
    State(state): State<ProviderState>,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    state
        .requests
        .lock()
        .expect("provider request record")
        .push(request);
    state.arrived.notify_waiters();
    let held = *state.hold.lock().expect("provider hold");
    if held {
        state.release.notified().await;
    }
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":",
        "\"<ASSISTANT_TEXT>\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":0,",
        "\"completion_tokens\":0,\"total_tokens\":0}}\n\n",
        "data: [DONE]\n\n",
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream")],
        body,
    )
}

struct Harness {
    root: TempRoot,
    server: Server,
    provider: ProviderStub,
    client: reqwest::Client,
    stored_response_id: Option<String>,
}

impl Harness {
    async fn start(case: &str) -> Self {
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
        }
    }

    async fn execute(&mut self, fixture: &Fixture) -> Value {
        if fixture.name == "chat_idempotent_retry" {
            return self.execute_chat_idempotent(&fixture.request).await;
        }
        if fixture.name == "responses_no_idempotency" {
            let first = self.send(&fixture.request).await;
            let second = self.send(&fixture.request).await;
            return json!({"attempts":[first, second]});
        }
        let mut actual = self.send(&fixture.request).await;
        if let Some(id) = actual["stored_response_id"].as_str().map(str::to_owned) {
            if fixture.name == "responses_stored_json" {
                assert_cursor(&id, fixture.cursor.as_ref().expect("cursor fixture"));
                self.stored_response_id = Some(id);
            }
            actual
                .as_object_mut()
                .expect("response object")
                .remove("stored_response_id");
        }
        actual
    }

    async fn execute_chat_idempotent(&self, request: &FixtureRequest) -> Value {
        self.provider.hold();
        let first_client = self.client.clone();
        let first_request = request.clone();
        let port = self.server.port;
        let first =
            tokio::spawn(
                async move { send_request(&first_client, port, &first_request, None).await },
            );
        self.provider.wait_arrived().await;
        let second_client = self.client.clone();
        let second_request = request.clone();
        let joined =
            tokio::spawn(
                async move { send_request(&second_client, port, &second_request, None).await },
            );
        tokio::task::yield_now().await;
        self.provider.release();
        let first = first.await.expect("first idempotent request");
        let joined = joined.await.expect("joined idempotent request");
        let mut replay = request.clone();
        replay.headers.remove("idempotency-key");
        replay
            .headers
            .insert("x-idempotency-key".into(), "<IDEMPOTENCY_KEY>".into());
        let settled = self.send(&replay).await;
        assert_eq!(self.provider.count(), 1, "idempotent provider calls");
        json!({"attempts":[first, joined, settled]})
    }

    async fn send(&self, request: &FixtureRequest) -> Value {
        send_request(
            &self.client,
            self.server.port,
            request,
            self.stored_response_id.as_deref(),
        )
        .await
    }
}

async fn send_request(
    client: &reqwest::Client,
    port: u16,
    request: &FixtureRequest,
    stored_response_id: Option<&str>,
) -> Value {
    let method = reqwest::Method::from_bytes(request.method.as_bytes()).expect("fixture method");
    let mut builder = client
        .request(method, format!("http://127.0.0.1:{port}{}", request.path))
        .header(header::AUTHORIZATION, AUTHORIZATION);
    for (name, value) in &request.headers {
        if name != "authorization" {
            builder = builder.header(name, value);
        }
    }
    if let Some(mut body) = request.body.clone() {
        if body.get("previous_response_id").and_then(Value::as_str)
            == Some("<FROM:responses_stored_json>")
        {
            body["previous_response_id"] = json!(stored_response_id.expect("stored dependency"));
        }
        builder = builder.json(&body);
    }
    let response = tokio::time::timeout(CHILD_TIMEOUT, builder.send())
        .await
        .expect("HTTP request timeout")
        .expect("HTTP request");
    normalize_response(response, request.mode == "sse").await
}

async fn normalize_response(response: reqwest::Response, sse: bool) -> Value {
    let status = response.status().as_u16();
    let headers = normalized_headers(response.headers());
    if sse {
        let events = parse_sse(response).await;
        json!({"status":status,"headers":headers,"events":events})
    } else {
        let body: Value = tokio::time::timeout(CHILD_TIMEOUT, response.json())
            .await
            .expect("JSON response timeout")
            .expect("JSON response");
        let stored = body["id"]
            .as_str()
            .filter(|id| id.starts_with("resp_letta_"))
            .map(str::to_owned);
        let mut result = json!({"status":status,"headers":headers,"body":normalize(body)});
        if let Some(id) = stored {
            result["stored_response_id"] = json!(id);
        }
        result
    }
}

fn normalized_headers(headers: &HeaderMap) -> Value {
    let mut values = Map::new();
    for name in [header::CONTENT_TYPE, header::CACHE_CONTROL] {
        if let Some(value) = headers.get(&name) {
            values.insert(
                name.as_str().to_owned(),
                Value::String(value.to_str().expect("header text").to_owned()),
            );
        }
    }
    Value::Object(values)
}

async fn parse_sse(response: reqwest::Response) -> Vec<Value> {
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut events = Vec::new();
    while let Some(chunk) = tokio::time::timeout(CHILD_TIMEOUT, stream.next())
        .await
        .expect("SSE chunk timeout")
    {
        pending.extend_from_slice(&chunk.expect("SSE chunk"));
        assert!(pending.len() <= FIXTURE_BYTES_MAX, "SSE pending byte bound");
        while let Some(end) = pending.windows(2).position(|part| part == b"\n\n") {
            let block = pending.drain(..end + 2).collect::<Vec<_>>();
            events.push(parse_sse_block(&block[..end]));
            assert!(events.len() <= SSE_EVENTS_MAX, "SSE event bound");
        }
    }
    assert!(
        pending.is_empty(),
        "SSE framing has trailing bytes: {pending:?}"
    );
    events
}

fn parse_sse_block(block: &[u8]) -> Value {
    let text = std::str::from_utf8(block).expect("SSE UTF-8");
    let mut event = Value::Null;
    let mut data = None;
    for line in text.split('\n') {
        if let Some(value) = line.strip_prefix("event: ") {
            event = json!(value);
        } else if let Some(value) = line.strip_prefix("data: ") {
            assert!(data.is_none(), "duplicate SSE data line");
            data = Some(value);
        } else {
            panic!("invalid SSE framing line: {line:?}");
        }
    }
    let data = data.expect("SSE data line");
    let value = if data == "[DONE]" {
        json!(data)
    } else {
        normalize(serde_json::from_str(data).expect("SSE data JSON"))
    };
    json!({"event":event,"data":value})
}

fn normalize(mut value: Value) -> Value {
    normalize_at(&mut value, "");
    value
}

fn normalize_at(value: &mut Value, key: &str) {
    match value {
        Value::Number(_) if matches!(key, "created" | "created_at") => {
            *value = json!("<UNIX_TIMESTAMP>");
        }
        Value::String(text) => normalize_string(text),
        Value::Array(values) => {
            for child in values {
                normalize_at(child, key);
            }
        }
        Value::Object(values) => {
            for (child_key, child) in values {
                normalize_at(child, child_key);
            }
        }
        _ => {}
    }
}

fn normalize_string(text: &mut String) {
    let replacement = if text.starts_with("chatcmpl-") {
        Some("<CHAT_COMPLETION_ID>")
    } else if text.starts_with("resp_letta_") {
        Some("<STORED_RESPONSE_ID>")
    } else if text.starts_with("resp_") {
        Some("<RESPONSE_ID>")
    } else if text.starts_with("msg_") {
        Some("<MSG_ID>")
    } else if text.starts_with("fc_") {
        Some("<FC_ID>")
    } else if text.starts_with("fco_") {
        Some("<FCO_ID>")
    } else if text.starts_with("rs_") {
        Some("<RS_ID>")
    } else if uuid::Uuid::parse_str(text).is_ok() {
        Some("<UUID>")
    } else {
        None
    };
    if let Some(replacement) = replacement {
        replacement.clone_into(text);
    }
}

fn assert_events(case: &str, expected: &[Value], actual: &[Value]) {
    let length = expected.len().max(actual.len());
    for index in 0..length {
        let left = expected.get(index);
        let right = actual.get(index);
        assert_eq!(
            left, right,
            "case {case} event index {index}: expected={left:?} actual={right:?}"
        );
    }
}

fn compare(case: &str, mode: &str, expected: &Value, actual: &Value) {
    if mode != "sse" {
        assert_eq!(
            expected, actual,
            "case {case}: expected={expected} actual={actual}"
        );
        return;
    }
    assert_eq!(expected["status"], actual["status"], "case {case} status");
    assert_eq!(
        expected["headers"], actual["headers"],
        "case {case} headers"
    );
    let expected_events = expected["events"].as_array().expect("fixture events");
    let actual_events = actual["events"].as_array().expect("actual events");
    assert_events(case, expected_events, actual_events);
}

async fn run_named_case(name: &str) {
    let index = load_index();
    let selected = index
        .cases
        .iter()
        .find(|case| case.name == name)
        .expect("indexed case");
    let order = dependency_order(&index, selected);
    let mut harness = Harness::start(name).await;
    let before = harness.provider.count();
    for entry in order {
        let fixture = load_fixture(entry);
        let actual = harness.execute(&fixture).await;
        compare(&fixture.name, &fixture.mode, &fixture.expected, &actual);
        assert_fixture_observables(&harness, &fixture, before);
    }
}

fn assert_fixture_observables(harness: &Harness, fixture: &Fixture, before: usize) {
    let expected_calls = fixture.observable["provider_calls_delta"].as_u64();
    if fixture.dependencies.is_empty()
        && fixture.name != "models_json"
        && let Some(expected) = expected_calls
    {
        assert_eq!(harness.provider.count() - before, expected as usize);
    }
    if fixture.name == "chat_stateful_newest" {
        let requests = harness.provider.requests();
        let newest = requests
            .last()
            .expect("stateful provider request")
            .to_string();
        assert!(newest.contains("<NEWEST_USER_TEXT>"));
        assert!(!newest.contains("<OLD_USER_TEXT>"));
        assert!(!newest.contains("<OLD_ASSISTANT_TEXT>"));
    }
    if fixture.name == "responses_previous_json" {
        let records = conversation_records(&harness.root.0.join("store/conversations"));
        assert!(
            records.iter().any(|record| record["hidden"] == true),
            "hidden fork records: {records:?}"
        );
    }
    if matches!(
        fixture.name.as_str(),
        "chat_headerless_json" | "chat_stream_sse" | "responses_nonstored_json"
    ) {
        assert_eq!(
            conversation_records(&harness.root.0.join("store/conversations")).len(),
            0
        );
    }
}

fn conversation_records(root: &Path) -> Vec<Value> {
    if !root.exists() {
        return Vec::new();
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(root).expect("conversation directory") {
        let path = entry.expect("conversation entry").path();
        if path.is_dir() {
            for child in fs::read_dir(path).expect("conversation record directory") {
                let child = child.expect("conversation record entry").path();
                if child.extension().and_then(|value| value.to_str()) == Some("json")
                    && let Ok(value) = serde_json::from_slice(&fs::read(child).expect("record"))
                {
                    records.push(value);
                }
            }
        }
    }
    records
}

fn dependency_order<'a>(index: &'a FixtureIndex, selected: &'a IndexCase) -> Vec<&'a IndexCase> {
    let mut order = Vec::new();
    for dependency in &selected.dependencies {
        let entry = index
            .cases
            .iter()
            .find(|case| &case.name == dependency)
            .expect("dependency indexed");
        assert!(entry.dependencies.is_empty(), "dependency depth bound");
        order.push(entry);
    }
    order.push(selected);
    order
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/openai")
}

fn load_index() -> FixtureIndex {
    let bytes = fs::read(fixtures_root().join("index.json")).expect("fixture index");
    assert!(bytes.len() <= FIXTURE_BYTES_MAX, "index byte bound");
    serde_json::from_slice(&bytes).expect("fixture index JSON")
}

fn load_fixture(entry: &IndexCase) -> Fixture {
    let bytes = fs::read(fixtures_root().join(&entry.path)).expect("fixture case");
    assert!(bytes.len() <= FIXTURE_BYTES_MAX, "fixture byte bound");
    let hash = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(hash, entry.sha256, "fixture hash: {}", entry.name);
    serde_json::from_slice(&bytes).expect("fixture case JSON")
}

async fn seed(root: &Path, provider_port: u16) {
    let store_root = root.join("store");
    fs::create_dir_all(store_root.join("providers")).expect("provider directory");
    let provider = json!({
        "version": 1,
        "providers": {"fixture-provider": {
            "id": "local-provider-fixture-provider",
            "name": "fixture-provider",
            "provider_type": "openai-compatible",
            "provider_category": "byok",
            "auth": {"type": "api", "key": "fixture-provider-credential"},
            "base_url": format!("http://127.0.0.1:{provider_port}/v1/"),
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }}
    });
    fs::write(
        store_root.join("providers/auth.json"),
        serde_json::to_vec_pretty(&provider).expect("provider JSON"),
    )
    .expect("provider auth");
    let paths = StorePaths::new(&store_root).expect("store paths");
    let store = LocalStore::new(paths);
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
    json!({
        "id": id,
        "name": name,
        "description": null,
        "system": "",
        "tags": [],
        "model": "local-provider-fixture-provider/default",
        "model_settings": {},
        "hidden": hidden,
        "compaction_settings": null,
        "created_at": created_at
    })
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
    let started = Instant::now();
    for _ in 0..READINESS_ATTEMPTS_MAX {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{}/v1/models", server.port))
            .header(header::AUTHORIZATION, AUTHORIZATION)
            .send()
            .await
            && response.status().is_success()
        {
            assert!(started.elapsed() <= CHILD_TIMEOUT);
            return;
        }
        tokio::time::sleep(READINESS_DELAY).await;
    }
    panic!("production lotta readiness bound exceeded");
}

fn assert_cursor(id: &str, expected: &Value) {
    let payload = id
        .strip_prefix("resp_letta_")
        .expect("stored cursor prefix");
    assert!(!payload.contains('='), "cursor is unpadded");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("cursor base64");
    let mut value: Value = serde_json::from_slice(&bytes).expect("cursor JSON");
    assert_eq!(value.as_object().expect("cursor object").len(), 4);
    assert_eq!(value["version"], 1);
    assert!(uuid::Uuid::parse_str(value["nonce"].as_str().expect("cursor nonce")).is_ok());
    value["nonce"] = json!("<UUID>");
    value["conversation_id"] = json!("<CONVERSATION_ID>");
    assert_eq!(&value, expected);
}

#[test]
fn index_is_complete() {
    let index = load_index();
    assert_eq!(index.schema_version, 1);
    assert_eq!(index.baseline_commit, BASELINE_COMMIT);
    assert_eq!(index.bounds.cases_max, FIXTURE_CASES_MAX);
    assert_eq!(index.bounds.fixture_bytes_max, FIXTURE_BYTES_MAX);
    assert_eq!(index.bounds.events_per_case_max, SSE_EVENTS_MAX);
    assert_eq!(index.bounds.json_depth_max, JSON_DEPTH_MAX);
    assert_eq!(index.cases.len(), 11);
    let names = index
        .cases
        .iter()
        .map(|case| &case.name)
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), index.cases.len());
    for case in &index.cases {
        let fixture = load_fixture(case);
        assert_eq!(fixture.schema_version, 1);
        assert_eq!(fixture.name, case.name);
        assert_eq!(fixture.route, case.route);
        assert_eq!(fixture.mode, case.mode);
        assert_eq!(fixture.dependencies, case.dependencies);
        assert_eq!(fixture.request.mode, fixture.mode);
        assert!(
            json_depth(&serde_json::to_value(&fixture).expect("fixture value")) <= JSON_DEPTH_MAX
        );
    }
}

#[test]
fn case_coverage() {
    let names = load_index()
        .cases
        .into_iter()
        .map(|case| case.name)
        .collect::<BTreeSet<_>>();
    for required in [
        "chat_stateful_newest",
        "chat_headerless_json",
        "chat_idempotent_retry",
        "responses_stored_json",
        "responses_nonstored_json",
        "responses_previous_json",
        "responses_no_idempotency",
    ] {
        assert!(names.contains(required), "missing required case {required}");
    }
}

#[test]
fn is_sanitized() {
    let index = load_index();
    scan_value(&serde_json::to_value(&index).expect("index value"), "index");
    for entry in &index.cases {
        let fixture = load_fixture(entry);
        scan_value(
            &serde_json::to_value(fixture).expect("fixture value"),
            &entry.name,
        );
    }
}

#[test]
fn sanitizer_mutations_are_rejected() {
    for planted in [
        "sk-fixture-secret",
        "Bearer planted",
        "/Users/example/private",
        "2026-08-31T12:00:00Z",
        "123e4567-e89b-12d3-a456-426614174000",
        "unapproved transcript sentence",
    ] {
        assert!(
            scan_string(planted, "content").is_err(),
            "accepted mutation {planted}"
        );
    }
    assert!(scan_secret_field("api_key", "not-a-placeholder").is_err());
    assert!(scan_secret_field("cookie", "session=value").is_err());
}

#[test]
fn comparator_mutation_reports_event_index() {
    let expected = vec![json!({"data":1}), json!({"data":2})];
    let actual = vec![json!({"data":1}), json!({"data":3})];
    let panic = std::panic::catch_unwind(|| assert_events("mutation", &expected, &actual))
        .expect_err("mutation must fail");
    let message = panic_message(panic);
    assert!(message.contains("case mutation event index 1"));
    assert!(message.contains("expected=") && message.contains("actual="));
}

#[test]
fn comparator_rejects_missing_extra_reordered_and_duplicate_events() {
    let expected = vec![json!("a"), json!("b"), json!("c")];
    for actual in [
        vec![json!("a"), json!("b")],
        vec![json!("a"), json!("b"), json!("c"), json!("d")],
        vec![json!("b"), json!("a"), json!("c")],
        vec![json!("a"), json!("b"), json!("b")],
    ] {
        assert!(
            std::panic::catch_unwind(|| assert_events("mutation", &expected, &actual)).is_err()
        );
    }
}

fn panic_message(value: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = value.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = value.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "non-string panic".to_owned()
    }
}

fn json_depth(value: &Value) -> usize {
    let mut depth_max = 1;
    let mut stack = vec![(value, 1)];
    while let Some((current, depth)) = stack.pop() {
        depth_max = depth_max.max(depth);
        match current {
            Value::Array(values) => stack.extend(values.iter().map(|child| (child, depth + 1))),
            Value::Object(values) => stack.extend(values.values().map(|child| (child, depth + 1))),
            _ => {}
        }
    }
    depth_max
}

fn scan_value(value: &Value, path: &str) {
    let mut stack = vec![(value, path.to_owned())];
    while let Some((current, current_path)) = stack.pop() {
        match current {
            Value::String(text) => {
                scan_string(text, &current_path)
                    .unwrap_or_else(|error| panic!("{current_path}: {error}"));
            }
            Value::Array(values) => {
                for (index, child) in values.iter().enumerate() {
                    stack.push((child, format!("{current_path}/{index}")));
                }
            }
            Value::Object(values) => {
                for (key, child) in values {
                    if let Value::String(text) = child {
                        scan_secret_field(key, text)
                            .unwrap_or_else(|error| panic!("{current_path}/{key}: {error}"));
                    }
                    stack.push((child, format!("{current_path}/{key}")));
                }
            }
            _ => {}
        }
    }
}

fn scan_secret_field(key: &str, value: &str) -> Result<(), &'static str> {
    let lowered = key.to_ascii_lowercase();
    if ["authorization", "key", "token", "cookie", "set-cookie"]
        .iter()
        .any(|secret| lowered.contains(secret))
        && !approved_placeholder(value)
    {
        return Err("secret field is not an approved placeholder");
    }
    Ok(())
}

fn scan_string(value: &str, path: &str) -> Result<(), &'static str> {
    let lowered = value.to_ascii_lowercase();
    if lowered.contains("sk-")
        || lowered.contains("bearer ")
        || lowered.contains("api_key")
        || lowered.contains("apikey")
        || lowered.contains("set-cookie")
    {
        return Err("secret-shaped text");
    }
    if (value.starts_with('/') && !value.starts_with("/v1/"))
        || value.contains("/Users/")
        || value.contains("/home/")
    {
        return Err("absolute or home path");
    }
    if uuid::Uuid::parse_str(value).is_ok() {
        return Err("unsanitized UUID");
    }
    if chrono::DateTime::parse_from_rfc3339(value).is_ok() {
        return Err("unsanitized date");
    }
    if is_content_path(path)
        && !value.is_empty()
        && !approved_placeholder(value)
        && !approved_protocol_text(value)
    {
        return Err("unapproved free text");
    }
    Ok(())
}

fn is_content_path(path: &str) -> bool {
    path == "content"
        || [
            "/content",
            "/text",
            "/delta",
            "/input",
            "/instructions",
            "/message",
        ]
        .iter()
        .any(|part| path.ends_with(part))
}

fn approved_placeholder(value: &str) -> bool {
    value.starts_with('<')
        && value.ends_with('>')
        && value.len() <= 96
        && value[1..value.len() - 1].bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b':')
        })
}

fn approved_protocol_text(value: &str) -> bool {
    matches!(value, "[DONE]" | "" | "assistant" | "user" | "stop")
}

macro_rules! golden_case {
    ($name:ident, $case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() {
            run_named_case($case).await;
        }
    };
}

golden_case!(fixture_models_json, "models_json");
golden_case!(fixture_chat_headerless_json, "chat_headerless_json");
golden_case!(fixture_chat_stream_sse, "chat_stream_sse");
golden_case!(fixture_chat_stateful_first, "chat_stateful_first");
golden_case!(fixture_chat_stateful_newest, "chat_stateful_newest");
golden_case!(fixture_chat_idempotent_retry, "chat_idempotent_retry");
golden_case!(fixture_responses_nonstored_json, "responses_nonstored_json");
golden_case!(fixture_responses_stream_sse, "responses_stream_sse");
golden_case!(fixture_responses_stored_json, "responses_stored_json");
golden_case!(fixture_responses_previous_json, "responses_previous_json");
golden_case!(fixture_responses_no_idempotency, "responses_no_idempotency");
