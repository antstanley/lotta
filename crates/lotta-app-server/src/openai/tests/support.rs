use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lotta_domain::{
    Agent, AgentId, BoundedMap, BoundedVec, Clock, DomainError, EntityExtras, NonEmptyString,
    Timestamp,
};
use lotta_runtime::ports::AgentStore;
use lotta_store::{LocalStore, StorePaths};
use sha2::{Digest, Sha256};

use crate::{
    config::ServerArgs,
    listener::{ListenerHandle, start_listener},
};

pub(super) struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56Z")
            .unwrap_or_else(|error| panic!("timestamp: {error}"))
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

pub(super) struct Roots {
    pub storage: PathBuf,
    pub workspace: PathBuf,
}

pub(super) fn roots(label: &str) -> Roots {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "lotta-openai-{label}-{}-{nonce}",
        std::process::id()
    ));
    let storage = root.join("storage");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&storage).unwrap_or_else(|error| panic!("storage: {error}"));
    std::fs::create_dir_all(&workspace).unwrap_or_else(|error| panic!("workspace: {error}"));
    Roots {
        storage: storage.canonicalize().unwrap_or(storage),
        workspace: workspace.canonicalize().unwrap_or(workspace),
    }
}

pub(super) fn agent(id: &str, name: &str, hidden: bool) -> Agent {
    Agent {
        id: AgentId::accept(id).unwrap_or_else(|error| panic!("agent id: {error}")),
        name: NonEmptyString::new(name).unwrap_or_else(|error| panic!("name: {error}")),
        description: None,
        system: String::new(),
        tags: BoundedVec::new(Vec::new()).unwrap_or_else(|error| panic!("tags: {error}")),
        model: NonEmptyString::new("openai/test").unwrap_or_else(|error| panic!("model: {error}")),
        model_settings: BoundedMap::new(BTreeMap::new())
            .unwrap_or_else(|error| panic!("settings: {error}")),
        hidden: hidden.then_some(Some(true)),
        compaction_settings: None,
        extras: EntityExtras::default(),
    }
}

pub(super) async fn seed(storage: &Path, agents: &[Agent]) {
    let paths = StorePaths::new(storage.to_path_buf())
        .unwrap_or_else(|error| panic!("store paths: {error}"));
    let store = LocalStore::new(paths);
    for agent in agents {
        store
            .save(agent)
            .await
            .unwrap_or_else(|error| panic!("save agent: {error}"));
    }
}

pub(super) async fn launch(roots: &Roots, openai_api: bool, token: Option<&str>) -> ListenerHandle {
    let mut args = ServerArgs {
        listen: Some("ws://127.0.0.1:0".to_owned()),
        listen_enabled: true,
        openai_api,
        storage_dir: Some(roots.storage.clone()),
        workspace_dir: Some(roots.workspace.clone()),
        ..ServerArgs::default()
    };
    if let Some(token) = token {
        args.ws_auth = Some("capability-token".to_owned());
        args.ws_token_sha256 = Some(format!("{:x}", Sha256::digest(token.as_bytes())));
    }
    let prepared = args
        .prepare()
        .unwrap_or_else(|error| panic!("prepare listener: {error}"));
    start_listener(prepared, Arc::new(TestClock))
        .await
        .unwrap_or_else(|error| panic!("start listener: {error}"))
}

pub(super) struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub(super) async fn get(handle: &ListenerHandle, target: &str, headers: &str) -> HttpResponse {
    let address = handle.address();
    let target = target.to_owned();
    let headers = headers.to_owned();
    tokio::task::spawn_blocking(move || request(address, &target, &headers))
        .await
        .unwrap_or_else(|error| panic!("request task: {error}"))
}

fn request(address: std::net::SocketAddr, target: &str, headers: &str) -> HttpResponse {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .unwrap_or_else(|error| panic!("connect: {error}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap_or_else(|error| panic!("read timeout: {error}"));
    let request =
        format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n");
    stream
        .write_all(request.as_bytes())
        .unwrap_or_else(|error| panic!("write request: {error}"));
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .unwrap_or_else(|error| panic!("read response: {error}"));
    parse_response(&bytes)
}

fn parse_response(bytes: &[u8]) -> HttpResponse {
    let text =
        String::from_utf8(bytes.to_vec()).unwrap_or_else(|error| panic!("response UTF-8: {error}"));
    let (head, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("HTTP response separator"));
    let status = head
        .split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("HTTP status"))
        .parse()
        .unwrap_or_else(|error| panic!("status number: {error}"));
    HttpResponse {
        status,
        body: body.to_owned(),
    }
}
