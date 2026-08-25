//! `ws::device::background_snapshot_is_state` — background-process snapshots
//! travel as `RuntimeEvent::UpdateDeviceStatus` listener state with the pinned
//! `device_status` envelope body, and never appear in the canonical
//! model-facing tool inventory.

use std::{
    path::PathBuf,
    process::Command as StdCommand,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use lotta_tools::names;
use serde_json::{Value, json};

use super::{
    BackgroundProcessSource, DeviceBridge, DeviceForwarder, DeviceMessage, NoRunningProcesses,
};
use crate::{ws::ConnectionId, ws::event::RuntimeEvent, ws::service::RuntimeEventSink};

const CONNECTION: ConnectionId = 91;

static ROOT_ORDINAL: AtomicUsize = AtomicUsize::new(0);

fn unique_root() -> PathBuf {
    let ordinal = ROOT_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let parent =
        std::env::temp_dir().join(format!("lotta-device-bg-{}-{ordinal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    parent.canonicalize().expect("canonical root")
}

fn git(workspace: &std::path::Path, args: &[&str]) {
    let output = StdCommand::new("git")
        .current_dir(workspace)
        .args(args)
        .env("GIT_AUTHOR_NAME", "lotta-test")
        .env("GIT_AUTHOR_EMAIL", "test@lotta.dev")
        .env("GIT_COMMITTER_NAME", "lotta-test")
        .env("GIT_COMMITTER_EMAIL", "test@lotta.dev")
        .output()
        .expect("git runs in tests");
    assert!(output.status.success(), "git {args:?} failed");
}

/// Records every runtime event routed to scope subscribers.
#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<RuntimeEvent>>,
}

impl RuntimeEventSink for RecordingSink {
    fn emit(
        &self,
        _scope: &lotta_domain::RuntimeScope,
        event: RuntimeEvent,
    ) -> Result<(), crate::error::AppServerError> {
        self.events.lock().expect("sink lock").push(event);
        Ok(())
    }
}

#[tokio::test]
async fn emits_update_device_status_and_no_tool_entry() {
    // One real repository so a successful checkout triggers the pinned
    // post-checkout device-status refresh.
    let root = unique_root();
    let workspace = root.join("workspace");
    let storage = root.join("storage");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&storage).expect("storage");
    git(&workspace, &["init", "-b", "main"]);
    git(&workspace, &["commit", "--allow-empty", "-m", "init"]);

    let messages: Arc<Mutex<Vec<(ConnectionId, DeviceMessage)>>> = Arc::default();
    let sink_messages = Arc::clone(&messages);
    let forward: DeviceForwarder = Arc::new(move |connection, message| {
        sink_messages
            .lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let bridge = Arc::new(DeviceBridge::new(forward, &workspace, &storage).expect("bridge"));
    let sink = Arc::new(RecordingSink::default());
    bridge.register_event_sink(Arc::clone(&sink) as Arc<dyn RuntimeEventSink>);
    bridge.register_scope_gate(Arc::new(|_| {
        vec![lotta_domain::RuntimeScope::new(
            lotta_domain::AgentId::accept("agent-bg").expect("agent"),
            lotta_domain::ConversationId::accept("conversation-bg").expect("conversation"),
            None,
        )]
    }));

    let command: Value = json!({
        "type": "checkout_branch",
        "request_id": "bg-1",
        "branch": "topic/bg",
        "create": true,
        "cwd": workspace.to_str().expect("utf-8 workspace"),
    });
    let frame = crate::framing::decode_text(&command.to_string()).expect("bounded frame");
    let decoded = super::decode(&frame)
        .expect("wellformed checkout")
        .expect("checkout routed");
    bridge.apply(CONNECTION, &decoded).await;

    let captured = messages.lock().expect("message lock").clone();
    assert_eq!(captured.len(), 1, "the direct answer only");
    assert_eq!(captured[0].1.discriminant_of(), "checkout_branch_response");

    // The state refresh rides RuntimeEvent::UpdateDeviceStatus with the
    // complete scoped device_status body including background processes.
    let events = sink.events.lock().expect("sink lock");
    assert_eq!(events.len(), 1, "one listener state event");
    let status = match &events[0] {
        RuntimeEvent::UpdateDeviceStatus { device_status } => {
            serde_json::to_value(device_status.as_value()).expect("bounded encodes")
        }
        other => panic!(
            "expected update_device_status, got {}",
            other.discriminant()
        ),
    };
    assert_eq!(
        status["background_processes"],
        json!([]),
        "no host-registered processes means an empty summary section"
    );
    assert_eq!(status["is_online"], json!(true));
    assert_eq!(
        status["current_working_directory"],
        json!(workspace.to_string_lossy())
    );
    assert_eq!(
        status["boot_working_directory"],
        status["current_working_directory"]
    );
    assert!(status["letta_code_version"].is_string());
    assert!(status["supported_commands"].as_array().is_some());

    // The default source stays inert and typed.
    let source: Arc<dyn BackgroundProcessSource> = Arc::new(NoRunningProcesses);
    assert!(source.snapshot().is_empty());

    // Even the fully canonical model-facing builtin inventory has no entry
    // under the background-snapshot names: they remain protocol services.
    for row in names::rows() {
        assert_ne!(row.internal, "process_manager", "no model-facing tool");
        assert_ne!(row.model, "process_manager");
        assert_ne!(row.internal, "background_process_snapshot");
        assert_ne!(row.model, "background_process_snapshot");
    }
}

/// Small local mirror of the wire discriminant for direct answers.
trait DiscriminantOf {
    fn discriminant_of(&self) -> &'static str;
}

impl DiscriminantOf for DeviceMessage {
    fn discriminant_of(&self) -> &'static str {
        match self {
            DeviceMessage::ExecuteCommand(_) => "execute_command_response",
            DeviceMessage::RemoveQueueItem(_) => "remove_queue_item_response",
            DeviceMessage::SearchBranches(_) => "search_branches_response",
            DeviceMessage::CheckoutBranch(_) => "checkout_branch_response",
            DeviceMessage::SecretList(_) => "secret_list_response",
            DeviceMessage::SecretApply(_) => "secret_apply_response",
        }
    }
}
