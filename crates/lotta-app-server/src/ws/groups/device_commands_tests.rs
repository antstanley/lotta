//! `ws::device::commands` — one case per §WebSocket command groups Device-row
//! command: slash/mod execution routes through the Task 45 registry, queue
//! removal answers and broadcasts, branches search and switch under confined
//! git, and secrets round-trip through the Task 52 store surface by name only.

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

use super::{DeviceBridge, DeviceForwarder, DeviceMessage};
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
        DeviceMessage::QueueUpdate(_) => "update_queue",
        DeviceMessage::StatusUpdate(_) => "update_device_status",
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
    Harness {
        bridge: Arc::new(DeviceBridge::new(forward, &workspace, &storage).expect("bridge")),
        workspace: workspace.clone(),
        scope: RuntimeScope::new(
            AgentId::accept("agent-device-cmd").expect("agent"),
            ConversationId::accept("conversation-device-cmd").expect("conversation"),
            None,
        ),
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

/// A mod host answering every command call with the fixed `cleared` output.
struct EchoHost;

impl ModHost for EchoHost {
    fn call(
        &self,
        _owner: &ModOwner,
        method: RpcMethod,
        _params: RpcParams,
        _cancellation: CancellationToken,
    ) -> HostFuture<'_> {
        assert!(matches!(method, RpcMethod::CommandCall));
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

fn published_echo_registry() -> Arc<ModRegistries> {
    let tools = Arc::new(ToolRegistry::new(Vec::new()).expect("empty tool registry"));
    let registries = ModRegistries::new(tools);
    let owner = ModOwner {
        id: ModId::new("echo-mod".to_owned()).expect("mod identity"),
        generation: Generation(1),
    };
    let batch = RegistrationBatch {
        commands: vec![CommandRegistration {
            id: RegistrationName::new("clear".to_owned()).expect("command name"),
            description: "clears the conversation view".to_owned(),
            args: None,
            owner: owner.clone(),
        }],
        ..Default::default()
    };
    let snapshot =
        ModRegistrationSnapshot::from_batch(&owner, batch).expect("valid registration batch");
    let publication = ModPublication {
        owner: owner.clone(),
        registrations: Arc::new(snapshot),
        host: Arc::new(EchoHost),
    };
    registries
        .commit(&[publication])
        .expect("publication commits");
    Arc::new(registries)
}

#[tokio::test]
async fn execute_command_runs_through_mod_registry() {
    let fixture = harness("exec");
    fixture
        .bridge
        .register_mod_commands(published_echo_registry());
    fixture
        .send(&json!({
            "type": "execute_command",
            "command_id": "clear",
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
    assert_eq!(fixture.encoded()[1]["output"], "unknown command");
}

#[tokio::test]
async fn queued_item_removal_answers_and_broadcasts() {
    let fixture = harness("queue");
    fixture.bridge.enqueue_for_test(&fixture.scope, "item-7");
    fixture
        .send(&json!({
            "type": "remove_queue_item",
            "request_id": "rq-7",
            "runtime": fixture.scope_json(),
            "item_id": "item-7",
        }))
        .await;
    assert_eq!(
        fixture.kinds(),
        vec!["remove_queue_item_response", "update_queue"],
        "answer first, authoritative state broadcast second"
    );
    let encoded = fixture.encoded();
    assert_eq!(encoded[0]["success"], true);
    assert_eq!(encoded[0]["item_id"], "item-7");
    let removed = encoded[1]["removed"].as_array().expect("transitions");
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0]["disposition"], json!("cancelled"));
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
async fn checkout_branch_switches_head() {
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
        vec!["checkout_branch_response", "update_device_status"],
        "answer first, then the pinned post-checkout device-status refresh"
    );
    assert_eq!(encoded[0]["success"], true);
    assert_eq!(encoded[0]["branch"], "topic/next");

    // The new branch is now the checked-out HEAD visible to searches. Captured
    // sequence so far: checkout answer, status refresh, then this answer.
    fixture
        .send(&json!({
            "type": "search_branches",
            "request_id": "cb-2",
            "query": "topic/next",
            "cwd": cwd,
        }))
        .await;
    let found = &fixture.encoded()[2];
    assert_eq!(found["branches"][0]["is_current"], true);

    // Checking out a nonexistent branch without create fails scrubbed. The
    // captured sequence so far: checkout answer, status refresh, search answer.
    fixture
        .send(&json!({
            "type": "checkout_branch",
            "request_id": "cb-3",
            "branch": "ghost",
            "cwd": cwd,
        }))
        .await;
    let failed = &fixture.encoded()[3];
    assert_eq!(failed["type"], "checkout_branch_response");
    assert_eq!(failed["success"], false);
    assert!(failed["error"].is_string());
}

#[tokio::test]
async fn secret_list_and_apply_round_trip_names_only() {
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
    assert_eq!(listed["secrets"], json!([{"key": "API_KEY"}]));
    let wire = listed.to_string();
    assert!(
        !wire.contains("s3cret-value"),
        "plaintext never crosses the wire: {wire}"
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
