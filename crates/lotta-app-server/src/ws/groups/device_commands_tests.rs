//! `ws::device::commands` — one case per §WebSocket command groups Device-row
//! command: slash/mod execution routes through the Task 45 registry behind the
//! runtime-scope gate, queue removal answers and broadcasts through the
//! authoritative port, branches search and switch under confined git, and
//! secrets round-trip through the Task 52 store surface with pinned `{key,
//! value}` list entries.

use std::{
    path::{Path, PathBuf},
    process::Command as StdCommand,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use lotta_domain::{AgentId, ConversationId, RuntimeScope};
use lotta_extensions::mods::{
    host::{HostFuture, ModHost},
    protocol::{RpcMethod, RpcParams, RpcResult},
    registrations::{CommandRegistration, ModRegistrationSnapshot, RegistrationBatch},
    registry::{ModPublication, ModRegistries},
    types::{Generation, ModId, ModOwner, RegistrationName},
};
use lotta_tools::registry::ToolRegistry;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{DeviceBridge, DeviceForwarder, DeviceMessage, ScopeGate};
use crate::{framing, ws::ConnectionId};

const CONNECTION: ConnectionId = 81;

static ROOT_ORDINAL: AtomicUsize = AtomicUsize::new(0);

struct Harness {
    bridge: Arc<DeviceBridge>,
    workspace: PathBuf,
    scope: RuntimeScope,
    messages: Arc<Mutex<Vec<(ConnectionId, DeviceMessage)>>>,
}

impl Harness {
    async fn send(&self, command: &Value) {
        let frame = framing::decode_text(&command.to_string()).expect("bounded frame");
        let decoded = super::decode(&frame)
            .expect("wellformed device command")
            .expect("device command routed");
        self.bridge.apply(CONNECTION, &decoded).await;
    }

    fn kinds(&self) -> Vec<&'static str> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == CONNECTION)
            .map(|(_, message)| discriminant(message))
            .collect()
    }

    fn encoded(&self) -> Vec<Value> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == CONNECTION)
            .map(|(_, message)| serde_json::to_value(message.clone()).expect("encodes"))
            .collect()
    }

    /// Runtime-scope JSON matching this fixture's scope.
    fn scope_json(&self) -> Value {
        json!({
            "agent_id": self.scope.agent_id.as_str(),
            "conversation_id": self.scope.conversation_id.as_str(),
        })
    }
}

fn discriminant(message: &DeviceMessage) -> &'static str {
    match message {
        DeviceMessage::ExecuteCommand(_) => "execute_command_response",
        DeviceMessage::RemoveQueueItem(_) => "remove_queue_item_response",
        DeviceMessage::SearchBranches(_) => "search_branches_response",
        DeviceMessage::CheckoutBranch(_) => "checkout_branch_response",
        DeviceMessage::SecretList(_) => "secret_list_response",
        DeviceMessage::SecretApply(_) => "secret_apply_response",
    }
}

fn unique_root(tag: &str) -> PathBuf {
    let ordinal = ROOT_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let parent = std::env::temp_dir().join(format!(
        "lotta-device-{tag}-{}-{ordinal}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    parent.canonicalize().expect("canonical root")
}

fn harness(tag: &str) -> Harness {
    let root = unique_root(tag);
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let storage = root.join("storage");
    std::fs::create_dir_all(&storage).expect("storage");
    let messages: Arc<Mutex<Vec<(ConnectionId, DeviceMessage)>>> = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: DeviceForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let bridge = Arc::new(DeviceBridge::new(forward, &workspace, &storage).expect("bridge"));
    let scope = RuntimeScope::new(
        AgentId::accept("agent-device-cmd").expect("agent"),
        ConversationId::accept("conversation-device-cmd").expect("conversation"),
        None,
    );
    // Every connection subscribes to this fixture's scope by default.
    let subscribed = scope.clone();
    let gate: ScopeGate = Arc::new(move |_| vec![subscribed.clone()]);
    bridge.register_scope_gate(gate);
    Harness {
        bridge,
        workspace,
        scope,
        messages,
    }
}

/// Creates one real repository with a single empty-root commit on `main`.
fn git_repo(workspace: &Path) -> PathBuf {
    let repo = workspace.join("repo");
    std::fs::create_dir_all(&repo).expect("repo dir");
    let mut environment = vec![
        ("GIT_AUTHOR_NAME", "lotta-test"),
        ("GIT_AUTHOR_EMAIL", "test@lotta.dev"),
        ("GIT_COMMITTER_NAME", "lotta-test"),
        ("GIT_COMMITTER_EMAIL", "test@lotta.dev"),
    ];
    git(&repo, &["init", "-b", "main"], &mut environment);
    git(
        &repo,
        &["config", "user.email", "test@lotta.dev"],
        &mut Vec::new(),
    );
    git(
        &repo,
        &["config", "user.name", "lotta-test"],
        &mut Vec::new(),
    );
    git(
        &repo,
        &["commit", "--allow-empty", "-m", "init"],
        &mut environment,
    );
    repo
}

fn git(repo: &Path, args: &[&str], environment: &mut [(&str, &str)]) {
    let mut command = StdCommand::new("git");
    command.current_dir(repo).args(args);
    for (key, value) in environment.iter() {
        command.env(key, value);
    }
    let output = command.output().expect("git runs in tests");
    assert!(output.status.success(), "git {args:?} failed");
}

/// A mod host answering every command call with the fixed `cleared` output and
/// recording the argument bodies it received for scoped-context assertions.
struct EchoHost {
    received: Mutex<Vec<Value>>,
}

impl EchoHost {
    fn new() -> Self {
        Self {
            received: Mutex::new(Vec::new()),
        }
    }

    fn last_received(&self) -> Value {
        self.received
            .lock()
            .expect("received lock")
            .last()
            .cloned()
            .expect("one recorded call")
    }
}

impl ModHost for EchoHost {
    fn call(
        &self,
        _owner: &ModOwner,
        method: RpcMethod,
        params: RpcParams,
        _cancellation: CancellationToken,
    ) -> HostFuture<'_> {
        assert!(matches!(method, RpcMethod::CommandCall));
        if let RpcParams::CommandCall { arguments, .. } = params {
            self.received.lock().expect("received lock").push(arguments);
        }
        Box::pin(async {
            Ok(RpcResult::Value {
                value: json!("cleared"),
            })
        })
    }

    fn dispose(&self) -> HostFuture<'_> {
        Box::pin(async { Ok(RpcResult::Disposed) })
    }

    fn abort(&self) -> HostFuture<'_> {
        Box::pin(async { Ok(RpcResult::Disposed) })
    }
}

fn published_echo_registry() -> (Arc<ModRegistries>, Arc<EchoHost>) {
    let tools = Arc::new(ToolRegistry::new(Vec::new()).expect("empty tool registry"));
    let registries = ModRegistries::new(tools);
    let owner = ModOwner {
        id: ModId::new("echo-mod".to_owned()).expect("mod identity"),
        generation: Generation(1),
    };
    let batch = RegistrationBatch {
        commands: vec![CommandRegistration {
            id: RegistrationName::new("echo-command".to_owned()).expect("command name"),
            description: "echoes its scoped context".to_owned(),
            args: None,
            owner: owner.clone(),
        }],
        ..Default::default()
    };
    let snapshot =
        ModRegistrationSnapshot::from_batch(&owner, batch).expect("valid registration batch");
    let host = Arc::new(EchoHost::new());
    let publication = ModPublication {
        owner: owner.clone(),
        registrations: Arc::new(snapshot),
        host: Arc::clone(&host) as Arc<dyn ModHost>,
    };
    registries
        .commit(&[publication])
        .expect("publication commits");
    (Arc::new(registries), host)
}

#[tokio::test]
async fn execute_command_runs_through_mod_registry() {
    let fixture = harness("exec");
    let (registries, host) = published_echo_registry();
    fixture.bridge.register_mod_commands(registries);
    fixture
        .send(&json!({
            "type": "execute_command",
            "command_id": "echo-command",
            "request_id": "ec-1",
            "runtime": fixture.scope_json(),
            "args": "--all",
        }))
        .await;
    let encoded = fixture.encoded();
    assert_eq!(
        fixture.kinds(),
        vec!["execute_command_response"],
        "one answer per dispatch"
    );
    assert_eq!(encoded[0]["success"], true, "the mod command resolved");
    assert_eq!(encoded[0]["output"], "cleared", "host output is relayed");

    // The call carries scoped context: parsed args plus cwd/conversation.
    let context = host.last_received();
    assert_eq!(context["args"], json!("--all"));
    assert_eq!(context["command"], json!("echo-command"));
    assert_eq!(
        context["runtime"]["conversation_id"],
        json!(fixture.scope.conversation_id.as_str())
    );

    // An identifier no mod published answers the failure shape without effects.
    fixture
        .send(&json!({
            "type": "execute_command",
            "command_id": "absent_command",
            "request_id": "ec-2",
            "runtime": fixture.scope_json(),
        }))
        .await;
    assert_eq!(fixture.kinds(), vec!["execute_command_response"; 2]);
    assert_eq!(fixture.encoded()[1]["success"], false);
    assert_eq!(
        fixture.encoded()[1]["output"],
        "Unknown command: absent_command"
    );
}

/// A connection subscribed to a different scope gets no dispatch at all.
#[tokio::test]
async fn execute_command_rejects_unsubscribed_scopes() {
    let fixture = harness("scope-gate");
    let (registries, _host) = published_echo_registry();
    fixture.bridge.register_mod_commands(registries);
    let other_scope = RuntimeScope::new(
        AgentId::accept("other-agent").expect("other agent"),
        ConversationId::accept("other-conversation").expect("other conversation"),
        None,
    );
    fixture
        .send(&json!({
            "type": "execute_command",
            "command_id": "echo-command",
            "request_id": "ec-gate",
            "runtime": {
                "agent_id": other_scope.agent_id.as_str(),
                "conversation_id": other_scope.conversation_id.as_str(),
            },
        }))
        .await;
    let encoded = fixture.encoded();
    assert_eq!(fixture.kinds(), vec!["execute_command_response"]);
    assert_eq!(encoded[0]["success"], false, "unsubscribed scope rejected");
    assert_eq!(encoded[0]["output"], "Unknown command: echo-command");
}

#[tokio::test]
async fn branch_search_filters_by_substring() {
    let fixture = harness("search");
    let repo = git_repo(&fixture.workspace);
    let cwd = repo.to_str().expect("utf-8 repo path").to_owned();

    fixture
        .send(&json!({
            "type": "search_branches",
            "request_id": "sb-1",
            "query": "",
            "cwd": cwd,
        }))
        .await;
    let encoded = fixture.encoded();
    assert_eq!(
        fixture.kinds(),
        vec!["search_branches_response"],
        "one listing per search"
    );
    assert_eq!(encoded[0]["success"], true);

    fixture
        .send(&json!({
            "type": "search_branches",
            "request_id": "sb-2",
            "query": "absent-query",
            "cwd": cwd,
        }))
        .await;
    assert_eq!(fixture.encoded()[1]["branches"], json!([]));

    // The current branch always matches its own lowercase substring.
    fixture
        .send(&json!({
            "type": "search_branches",
            "request_id": "sb-3",
            "query": "main",
            "max_results": 20,
            "cwd": cwd,
        }))
        .await;
    let third = &fixture.encoded()[2];
    let branches = third["branches"].as_array().expect("branch list");
    assert_eq!(branches.len(), 1, "only main matches the query");
    assert_eq!(branches[0]["name"], "main");
    assert_eq!(branches[0]["is_current"], true);
    assert_eq!(branches[0]["is_remote"], false);
}

#[tokio::test]
async fn checkout_branch_reports_the_checked_out_head() {
    let fixture = harness("checkout");
    let repo = git_repo(&fixture.workspace);
    let cwd = repo.to_str().expect("utf-8 repo path").to_owned();

    fixture
        .send(&json!({
            "type": "checkout_branch",
            "request_id": "cb-1",
            "branch": "topic/next",
            "create": true,
            "cwd": cwd,
        }))
        .await;
    let encoded = fixture.encoded();
    assert_eq!(
        fixture.kinds(),
        vec!["checkout_branch_response"],
        "answer only; the status refresh travels as a runtime event"
    );
    assert_eq!(encoded[0]["success"], true);
    assert_eq!(encoded[0]["branch"], "topic/next");

    // The reported branch matches the actual checked-out HEAD on disk.
    let head = StdCommand::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("head query");
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        encoded[0]["branch"].as_str().expect("branch text")
    );

    // Checking out a nonexistent branch without create fails scrubbed.
    fixture
        .send(&json!({
            "type": "checkout_branch",
            "request_id": "cb-3",
            "branch": "ghost",
            "cwd": cwd,
        }))
        .await;
    let failed = &fixture.encoded()[1];
    assert_eq!(failed["type"], "checkout_branch_response");
    assert_eq!(failed["success"], false);
    assert!(failed["error"].is_string());
}

/// Option-looking names and invalid ref characters are rejected fail-closed
/// before any git invocation runs.
#[tokio::test]
async fn checkout_branch_validates_names() {
    let fixture = harness("validate");
    let repo = git_repo(&fixture.workspace);
    let cwd = repo.to_str().expect("utf-8 repo path").to_owned();

    for (request_id, branch) in [("cb-bad-1", "--amend"), ("cb-bad-2", "bad name")] {
        fixture
            .send(&json!({
                "type": "checkout_branch",
                "request_id": request_id,
                "branch": branch,
                "create": true,
                "cwd": cwd,
            }))
            .await;
    }
    let encoded = fixture.encoded();
    for answer in &encoded {
        assert_eq!(answer["success"], false, "{answer} must be rejected");
        assert_eq!(answer["error"], json!("invalid branch name"));
    }
    // HEAD never moved off main despite the injected-looking arguments.
    let head = StdCommand::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("head query");
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main");
}

/// An unresolvable cwd is rejected instead of accepted as-is.
#[tokio::test]
async fn search_branches_fail_closed_on_unresolvable_cwd() {
    let fixture = harness("fail-closed");
    git_repo(&fixture.workspace);
    let missing = fixture
        .workspace
        .join("missing-dir")
        .join("deeper")
        .to_str()
        .expect("utf-8 path")
        .to_owned();

    fixture
        .send(&json!({
            "type": "search_branches",
            "request_id": "fc-1",
            "query": "",
            "cwd": missing,
        }))
        .await;
    let encoded = fixture.encoded();
    assert_eq!(encoded[0]["success"], false, "unresolvable cwd rejected");
    assert!(
        encoded[0]["error"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()),
        "scrubbed failure detail present"
    );
}

#[tokio::test]
async fn secret_list_and_apply_round_trip_with_pinned_entries() {
    let fixture = harness("secrets");
    fixture
        .send(&json!({
            "type": "secret_apply",
            "request_id": "sa-1",
            "agent_id": "agent-a",
            "set": {"Api_Key": "s3cret-value"},
            "unset": ["STALE_KEY"],
        }))
        .await;
    let encoded = fixture.encoded();
    assert_eq!(
        fixture.kinds(),
        vec!["secret_apply_response"],
        "one answer per apply"
    );
    assert_eq!(encoded[0]["success"], true);
    assert_eq!(encoded[0]["names"], json!(["API_KEY"]));

    fixture
        .send(&json!({
            "type": "secret_list",
            "request_id": "sl-1",
            "agent_id": "agent-a",
        }))
        .await;
    let listed = &fixture.encoded()[1];
    assert_eq!(listed["success"], true);
    // Pinned contract: sorted {key, value} entries; the authenticated modal
    // reads plaintext values back.
    assert_eq!(
        listed["secrets"],
        json!([{"key": "API_KEY", "value": "s3cret-value"}])
    );

    fixture
        .send(&json!({
            "type": "secret_apply",
            "request_id": "sa-2",
            "agent_id": "agent-a",
            "set": {"9bad": "x"},
            "unset": [],
        }))
        .await;
    let rejected = &fixture.encoded()[2];
    assert_eq!(rejected["success"], false);
    assert!(
        rejected["error"]
            .as_str()
            .expect("rejection detail")
            .starts_with("Invalid secret name '9bad'."),
        "pinned rejection text"
    );
}
