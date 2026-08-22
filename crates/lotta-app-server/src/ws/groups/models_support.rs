//! Shared fixtures for the models/providers command-group certificate selectors.

use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use lotta_domain::{AgentId, ConversationId, RunId, RuntimeScope, StopReason, TurnLease};
use lotta_domain::{Clock, Timestamp};
use lotta_providers::connections::{
    ActiveTurnRegistry, AuthMethod, ConnectProviderInput, ConnectionAdapter, ConnectionError,
    ConnectionManager, ProviderAuth, ProviderAuthStore, ProviderSecret, TurnCancellation,
};
use lotta_providers::model::{
    ConnectionReadiness, ListedModel, ModelCatalog, ModelHandle, ModelSettings,
};
use lotta_runtime::{ListenerRuntime, RuntimeHandle};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    ChatGptUsageReader, ModelsBridge, ModelsForwarder, ModelsMessage, UsageError, UsageSnapshot,
    UsageWindow,
};
use crate::{framing, ws::ConnectionId};

struct FixedSupportClock;

impl Clock for FixedSupportClock {
    fn now(&self) -> Timestamp {
        Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56+00:00").expect("support timestamp")
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, lotta_domain::DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

pub(super) const CONNECTION_A: ConnectionId = 31;
/// Fixture agent identifier for scoped update commands.
pub(super) const AGENT_ID: &str = "agent-1";
/// Fixture conversation identifier for agent-level scope.
pub(super) const CONVERSATION_ID: &str = "default";
/// Planted credential marker asserted absent from every recorded response.
pub(super) const SECRET_MARKER: &str = "SECRET_MARKER_KEY";

/// Recorded forwarded messages as `(connection, message)` pairs.
pub(super) type RecordedMessages = Arc<Mutex<Vec<(ConnectionId, ModelsMessage)>>>;

static ROOT_ORDINAL: AtomicUsize = AtomicUsize::new(0);

/// Stable local provider record identifier for one declarative row.
pub(super) fn record_id(row: &str) -> String {
    let prefix = super::RECORD_ID_PREFIX;
    format!("{prefix}{row}")
}

/// Bridge scope key for the fixture agent and `conversation`.
pub(super) fn scope_key(conversation: &str) -> String {
    format!("{AGENT_ID}/{conversation}")
}

/// Runtime scope JSON for the fixture agent and `conversation`.
pub(super) fn scope_json(conversation: &str) -> Value {
    json!({ "agent_id": AGENT_ID, "conversation_id": conversation })
}

/// Adapter recording structural validation calls without touching any host.
pub(super) struct RecordingAdapter {
    /// Provider types handed to [`ConnectionAdapter::validate`], in order.
    pub(super) validations: Mutex<Vec<String>>,
}

impl ConnectionAdapter for RecordingAdapter {
    fn validate<'a>(
        &'a self,
        provider_type: &'a str,
        _auth: &'a ProviderAuth,
        _fields: &'a BTreeMap<String, String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConnectionError>> + Send + 'a>> {
        let provider_type = provider_type.to_owned();
        Box::pin(async move {
            self.validations
                .lock()
                .expect("validation lock")
                .push(provider_type);
            Ok(())
        })
    }
}

/// Cancellation fake that settles the affected turn and records call order.
pub(super) struct SettlingCancellation {
    /// Recorded call order: `cancel` then `ack` per forced attempt.
    pub(super) order: Arc<Mutex<Vec<&'static str>>>,
    lease: Mutex<Option<TurnLease>>,
}

impl SettlingCancellation {
    /// Arms the fake with the lease to settle on the next cancel request.
    pub(super) fn arm(&self, lease: TurnLease) {
        self.lease.lock().expect("lease lock").replace(lease);
    }
}

impl TurnCancellation for SettlingCancellation {
    fn cancel_and_wait<'a>(
        &'a self,
        runtime: &'a mut ListenerRuntime,
        handle: &'a RuntimeHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConnectionError>> + Send + 'a>> {
        Box::pin(async move {
            self.order.lock().expect("order lock").push("cancel");
            let Some(lease) = self.lease.lock().expect("lease lock").take() else {
                return Err(ConnectionError::Cancellation);
            };
            let owner = runtime.lifecycle_mut(handle)?;
            owner
                .request_cancellation(&lease)
                .map_err(|_| ConnectionError::Cancellation)?;
            let stopped =
                StopReason::new("cancelled").map_err(|_| ConnectionError::Cancellation)?;
            owner
                .finish_turn(&lease, stopped)
                .map_err(|_| ConnectionError::Cancellation)?;
            self.order.lock().expect("order lock").push("ack");
            Ok(())
        })
    }
}

/// Usage-reader fake returning one canned outcome and recording calls.
pub(super) struct StubUsage {
    /// Outcome served for every read until overridden.
    pub(super) outcome: Mutex<Result<UsageSnapshot, UsageError>>,
    /// Recorded `(provider_name, force_refresh)` call pairs, in order.
    pub(super) calls: Mutex<Vec<(String, bool)>>,
}

impl StubUsage {
    /// Switches the fake to the pinned network failure for later reads.
    pub(super) fn fail_next_reads(&self) {
        let failure = Err(UsageError {
            code: super::USAGE_ERROR_NETWORK.to_owned(),
            message: super::USAGE_BACKEND_UNAVAILABLE.to_owned(),
            retry_after_ms: None,
        });
        *self.outcome.lock().expect("usage outcome lock") = failure;
    }
}

impl ChatGptUsageReader for StubUsage {
    fn read(&self, provider_name: &str, force_refresh: bool) -> Result<UsageSnapshot, UsageError> {
        self.calls
            .lock()
            .expect("usage call lock")
            .push((provider_name.to_owned(), force_refresh));
        self.outcome.lock().expect("usage outcome lock").clone()
    }
}

/// A bridge over one unique temporary provider store plus its fakes and recorder.
pub(super) struct TestModels {
    /// Bridge under test; every command applies inline via [`Self::send`].
    pub(super) bridge: Arc<ModelsBridge>,
    /// Recording connection adapter shared with the bridge.
    pub(super) adapter: Arc<RecordingAdapter>,
    /// Forced-cancellation fake shared with the bridge.
    pub(super) cancellation: Arc<SettlingCancellation>,
    /// Usage-reader fake shared with the bridge.
    pub(super) usage: Arc<StubUsage>,
    messages: RecordedMessages,
}

impl TestModels {
    /// Decodes a raw JSON command through framing and applies it inline.
    pub(super) async fn send(&self, connection: ConnectionId, command: &Value) {
        let frame = framing::decode_text(&command.to_string()).expect("bounded models frame");
        let decoded = super::decode(&frame)
            .expect("wellformed models command")
            .expect("models command routed");
        self.bridge.apply(connection, &decoded).await;
    }

    /// Snapshot of every forwarded message for one connection.
    pub(super) fn messages_for(&self, connection: ConnectionId) -> Vec<ModelsMessage> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == connection)
            .map(|(_, message)| message.clone())
            .collect()
    }

    /// Begins one active provider turn attached to the `openai` record.
    ///
    /// Arms the cancellation fake with the minted lease so a forced wire
    /// disconnect can settle the turn exactly like the Task 47 lifecycle.
    pub(super) async fn attach_active_openai_turn(&self) {
        let scope = RuntimeScope {
            agent_id: AgentId::accept(AGENT_ID).expect("agent id"),
            conversation_id: ConversationId::accept("conversation-1").expect("conversation id"),
            acting_user_id: None,
        };
        let (handle, lease) = {
            let mut runtime = self.bridge.runtime().lock().await;
            let handle = runtime
                .get_or_create(&scope, Uuid::from_u128(0x67))
                .expect("runtime handle");
            let lease = runtime
                .lifecycle_mut(&handle)
                .expect("runtime lifecycle")
                .begin_turn(
                    "turn-models".to_owned(),
                    RunId::accept("run").expect("run id"),
                )
                .expect("turn lease");
            (handle, lease)
        };
        {
            let mut turns = self.bridge.turns().lock().await;
            turns.attach(record_id("openai"), handle);
        }
        self.cancellation.arm(lease);
    }
}

/// Creates a bridge over a bare temporary provider store.
pub(super) async fn bridge() -> TestModels {
    fixture_bridge(None, false).await
}

/// Creates a bridge with one connected `OpenAI` record carrying `key`.
pub(super) async fn bridge_with_openai(key: &str) -> TestModels {
    fixture_bridge(Some(key), false).await
}

/// Creates a bridge with one connected `ChatGPT` plan record.
pub(super) async fn bridge_with_chatgpt() -> TestModels {
    fixture_bridge(None, true).await
}

/// Composes the bridge over a fresh store, seeding requested connections.
async fn fixture_bridge(openai_key: Option<&str>, chatgpt: bool) -> TestModels {
    let ordinal = ROOT_ORDINAL.fetch_add(1, Ordering::SeqCst);
    let root =
        std::env::temp_dir().join(format!("lotta-models-ws-{}-{ordinal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("fixture root");
    let storage_root = root.canonicalize().expect("canonical root");
    let store =
        ProviderAuthStore::new(lotta_store::StorePaths::new(&storage_root).expect("store paths"));
    let mut manager = ConnectionManager::load(store).expect("connection manager");
    let adapter = Arc::new(RecordingAdapter {
        validations: Mutex::new(Vec::new()),
    });
    if let Some(key) = openai_key {
        manager
            .connect(api_seed("openai", "openai", key, 0), adapter.as_ref())
            .await
            .expect("openai seed connection");
    }
    if chatgpt {
        manager
            .connect(
                oauth_seed("chatgpt-plus-pro", "chatgpt_oauth"),
                adapter.as_ref(),
            )
            .await
            .expect("chatgpt seed connection");
    }
    let cancellation = Arc::new(SettlingCancellation {
        order: Arc::new(Mutex::new(Vec::new())),
        lease: Mutex::new(None),
    });
    let usage = Arc::new(StubUsage {
        outcome: Mutex::new(Ok(canned_usage())),
        calls: Mutex::new(Vec::new()),
    });
    let messages: RecordedMessages = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: ModelsForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let bridge = Arc::new(ModelsBridge::compose(
        forward,
        manager,
        ActiveTurnRegistry::default(),
        ListenerRuntime::new(),
        Arc::clone(&adapter) as Arc<dyn ConnectionAdapter>,
        Arc::clone(&cancellation) as Arc<dyn TurnCancellation>,
        fixture_catalog(),
        Arc::clone(&usage) as Arc<dyn ChatGptUsageReader>,
        Arc::new(FixedSupportClock),
    ));
    TestModels {
        bridge,
        adapter,
        cancellation,
        usage,
        messages,
    }
}

/// Manager-level seed connect input for one API-key provider record.
fn api_seed(name: &str, provider_type: &str, key: &str, revision: u64) -> ConnectProviderInput {
    ConnectProviderInput {
        provider_id: record_id(name),
        provider_name: name.to_owned(),
        provider_type: provider_type.to_owned(),
        auth_method: AuthMethod::Api,
        auth: ProviderAuth::Api {
            key: ProviderSecret::new(key.to_owned()).expect("seed key"),
            extras: serde_json::Map::new(),
        },
        fields: BTreeMap::new(),
        expected_revision: revision,
        now: "2026-01-02T03:04:05Z".to_owned(),
    }
}

/// Manager-level seed connect input for the `ChatGPT` OAuth plan record.
fn oauth_seed(name: &str, provider_type: &str) -> ConnectProviderInput {
    ConnectProviderInput {
        provider_id: record_id(name),
        provider_name: name.to_owned(),
        provider_type: provider_type.to_owned(),
        auth_method: AuthMethod::OAuth,
        auth: ProviderAuth::OAuth {
            access: ProviderSecret::new("oauth-access-seed".to_owned()).expect("seed access"),
            refresh: None,
            id_token: None,
            expires: 9_999_999_999,
            account_id: None,
            extras: serde_json::Map::new(),
        },
        fields: BTreeMap::new(),
        expected_revision: 0,
        now: "2026-01-02T03:04:05Z".to_owned(),
    }
}

/// Catalog with one `OpenAI` and one `Anthropic` entry, both declared ready.
///
/// Readiness still derives from live connections: only providers holding a
/// connected record project their entries as available.
pub(super) fn fixture_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![
        (
            "openai".to_owned(),
            vec![listed_model("openai", "gpt-test")],
        ),
        (
            "anthropic".to_owned(),
            vec![listed_model("anthropic", "claude-test")],
        ),
    ])
    .expect("fixture catalog")
}

fn listed_model(provider: &str, model: &str) -> ListedModel {
    ListedModel {
        handle: ModelHandle::new(provider, model).expect("model handle"),
        readiness: ConnectionReadiness::Ready,
        model_settings: ModelSettings::default(),
    }
}

/// Canned credential-free usage snapshot served by the fake reader.
pub(super) fn canned_usage() -> UsageSnapshot {
    UsageSnapshot {
        provider_name: "chatgpt-plus-pro".to_owned(),
        fetched_at: "2026-01-02T03:04:05Z".to_owned(),
        summary: "5h 12% used".to_owned(),
        plan_type: Some("pro".to_owned()),
        limit_reached: Some(false),
        rate_limit_reached_type: None,
        primary: Some(usage_window("five_hours", 12.5, 300, 1_000)),
        secondary: Some(usage_window("weekly_all", 40.0, 10_080, 2_000)),
        additional: Vec::new(),
        credits: None,
        individual_limit: None,
    }
}

fn usage_window(label: &str, used_percent: f64, minutes: u64, resets_at: u64) -> UsageWindow {
    UsageWindow {
        label: label.to_owned(),
        used_percent: Some(used_percent),
        window_duration_mins: Some(minutes),
        resets_at: Some(resets_at),
    }
}

/// Encodes one outbound message for field-level assertions.
#[must_use]
pub(super) fn encoded(message: &ModelsMessage) -> Value {
    serde_json::to_value(message).expect("response encodes")
}

/// Outbound discriminant of one models group message.
#[must_use]
pub(super) fn discriminant_of(message: &ModelsMessage) -> &'static str {
    match message {
        ModelsMessage::ListModelsResponse(_) => "list_models_response",
        ModelsMessage::ListConnectProvidersResponse(_) => "list_connect_providers_response",
        ModelsMessage::ConnectProviderResponse(_) => "connect_provider_response",
        ModelsMessage::DisconnectProviderResponse(_) => "disconnect_provider_response",
        ModelsMessage::ChatgptUsageReadResponse(_) => "chatgpt_usage_read_response",
        ModelsMessage::UpdateModelResponse(_) => "update_model_response",
        ModelsMessage::UpdateToolsetResponse(_) => "update_toolset_response",
    }
}

pub(super) fn fixture_discriminants(section: &str) -> Vec<String> {
    let raw = include_str!("../../../../../fixtures/protocol/discriminants.json");
    let fixture: Value = serde_json::from_str(raw).expect("bounded fixture");
    fixture[section]["discriminants"]
        .as_array()
        .expect("discriminants")
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn assert_in_fixture(section: &str, tag: &str) {
    assert!(
        fixture_discriminants(section)
            .iter()
            .any(|entry| entry == tag),
        "{tag} missing from the {section} fixture group"
    );
}

/// Asserts no recorded response body carries the planted `marker`.
pub(super) fn assert_marker_absent(messages: &[ModelsMessage], marker: &str) {
    for message in messages {
        let body = serde_json::to_string(message).expect("response serializes");
        assert!(
            !body.contains(marker),
            "response leaked the planted credential marker"
        );
    }
}

/// Finds the declarative provider row with `id` for field-level assertions.
pub(super) fn find_row<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["id"] == id)
        .expect("declarative row present")
}

/// Finds the catalog entry with `handle` for readiness assertions.
pub(super) fn entry_with_handle<'a>(entries: &'a [Value], handle: &str) -> &'a Value {
    entries
        .iter()
        .find(|entry| entry["handle"] == handle)
        .expect("catalog entry present")
}
