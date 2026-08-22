// Wire payload models for the models/providers group, included verbatim into
// `models.rs` (`include!`). Shapes mirror the pinned `protocol_v2.ts`
// declarations for this row; the one deliberate extension is
// [`DisconnectProviderCommand::force`], which surfaces the Task 52 forced
// disconnect over the wire so the active-turn guard stays reachable.

/// Pinned `list_models` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ListModelsCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Caller-requested freshness bypass; this bridge reads live connection
    /// state on every call, so there is no cache to bypass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

/// Pinned `list_connect_providers` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ListConnectProvidersCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Provider store to inspect; only the local store exists.
    pub target: StorageTarget,
}

/// Pinned `connect_provider` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConnectProviderCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Provider store to write; only the local store exists.
    pub target: StorageTarget,
    /// Provider row identifier from `list_connect_providers`.
    pub provider_id: String,
    /// Auth method selector for multi-method rows such as Bedrock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_method_id: Option<String>,
    /// User-supplied connection fields keyed by field key.
    pub fields: BTreeMap<String, String>,
}

/// Pinned `disconnect_provider` payload plus the Task 52 `force` extension.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DisconnectProviderCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Provider store to write; only the local store exists.
    pub target: StorageTarget,
    /// Provider row identifier from `list_connect_providers`.
    pub provider_id: String,
    /// Exact record name selector when one row holds several aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    /// Cancel affected active turns instead of refusing the disconnect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

/// Pinned `chatgpt_usage_read` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChatgptUsageReadCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Usage source; this bridge serves the local provider store only.
    pub target: UsageTarget,
    /// Connected `ChatGPT` alias override; defaults to the built-in alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    /// Skip the reader-side cache when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_refresh: Option<bool>,
}

/// Pinned `update_model` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateModelCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Agent and conversation whose model selection changes.
    pub runtime: RuntimeScopeRef,
    /// Requested model identity and effort overlay.
    pub payload: UpdateModelPayload,
}

/// Pinned `update_toolset` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateToolsetCommand {
    /// Response correlation identifier.
    pub request_id: String,
    /// Agent and conversation whose toolset preference changes.
    pub runtime: RuntimeScopeRef,
    /// Preference applied through the Task 32 registry.
    pub toolset_preference: ToolsetPreference,
}

/// The seven concrete models/providers group commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum ModelsCommand {
    /// Credential-free model listing with readiness.
    #[serde(rename = "list_models")]
    ListModels(ListModelsCommand),
    /// Declarative provider rows with live connection states.
    #[serde(rename = "list_connect_providers")]
    ListConnectProviders(ListConnectProvidersCommand),
    /// Persist one provider connection through Task 52.
    #[serde(rename = "connect_provider")]
    ConnectProvider(ConnectProviderCommand),
    /// Remove one provider connection through Task 52.
    #[serde(rename = "disconnect_provider")]
    DisconnectProvider(DisconnectProviderCommand),
    /// Read `ChatGPT` plan usage behind a connected provider.
    #[serde(rename = "chatgpt_usage_read")]
    ChatgptUsageRead(ChatgptUsageReadCommand),
    /// Validate then persist one scoped model selection.
    #[serde(rename = "update_model")]
    UpdateModel(UpdateModelCommand),
    /// Resolve then persist one scoped toolset preference.
    #[serde(rename = "update_toolset")]
    UpdateToolset(UpdateToolsetCommand),
}

/// Provider store selector; the baseline serves the local store only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageTarget {
    /// Canonical local provider record store.
    Local,
}

/// Usage source selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageTarget {
    /// Local provider record store.
    Local,
    /// Remote API source; unsupported by this bridge.
    Api,
}

/// Pinned runtime scope echoed by scoped responses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeScopeRef {
    /// Agent whose runtime scope is addressed.
    pub agent_id: String,
    /// Conversation inside the agent; `default` selects agent-level scope.
    pub conversation_id: String,
    /// Acting user attribution echoed back untouched when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_user_id: Option<String>,
}

/// Pinned `update_model` request body.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateModelPayload {
    /// Curated model identifier such as `sonnet`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Direct `provider/model` handle override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_handle: Option<String>,
    /// Explicit reasoning effort; absent preserves the catalog default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// One listed model entry carrying Task 47 readiness.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelEntry {
    /// Stable curated identifier, or the handle for native rows.
    pub id: String,
    /// Stable `provider/model` handle.
    pub handle: String,
    /// Human-readable label.
    pub label: String,
    /// Short description; native rows report an empty string.
    pub description: String,
    /// Connection-derived readiness reported per the plan step.
    pub readiness: ConnectionReadiness,
    /// Canonical settings defaults for the entry.
    pub model_settings: ModelSettings,
}

/// Pinned `list_models_response` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ListModelsResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Credential-free catalog entries with readiness.
    pub entries: Vec<ModelEntry>,
    /// Handles currently usable from live connections.
    pub available_handles: Vec<String>,
    /// Connected alias to canonical provider-type mapping.
    pub byok_provider_aliases: BTreeMap<String, String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Redacted connection state for one stored provider record.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConnectionState {
    /// Baseline connected-state discriminator.
    pub is_connected: bool,
    /// Stable connection identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Provider record name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    /// Registered provider type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    /// Authentication method without credential material.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<AuthMethod>,
    /// Optional endpoint URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Optional baseline timeout object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Value>,
    /// Optional region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl ConnectionState {
    /// Disconnected placeholder preserving the pinned older-client shape.
    #[must_use]
    fn disconnected() -> Self {
        Self {
            is_connected: false,
            id: None,
            provider_name: None,
            provider_type: None,
            auth_type: None,
            base_url: None,
            timeout: None,
            region: None,
        }
    }
}

/// One declarative connectable provider row with live connection states.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderEntry {
    /// Stable row identifier used by connect and disconnect commands.
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Human-readable row description.
    pub description: String,
    /// Registered provider type served by the row.
    pub provider_type: String,
    /// Primary provider record name.
    pub provider_name: String,
    /// Every provider record name represented by the row.
    pub provider_names: Vec<String>,
    /// Whether the row demands an API key.
    pub requires_api_key: bool,
    /// Declared connection fields when the row accepts key connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<ConnectField>>,
    /// Declared auth methods for multi-method rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_methods: Option<Vec<ConnectProviderAuthMethod>>,
    /// First connected state, preserved for older clients.
    pub connected: ConnectionState,
    /// Every connected state represented by the row.
    pub connected_providers: Vec<ConnectionState>,
}

/// Pinned `list_connect_providers_response` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ListConnectProvidersResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Store the listing describes.
    pub target: StorageTarget,
    /// Declarative rows merged with live states.
    pub providers: Vec<ProviderEntry>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `connect_provider_response` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConnectProviderResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Store the mutation touched.
    pub target: StorageTarget,
    /// Refreshed provider snapshot after the mutation.
    pub providers: Vec<ProviderEntry>,
    /// Whether model readiness should be refetched.
    pub models_may_have_changed: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `disconnect_provider_response` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DisconnectProviderResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Store the mutation touched.
    pub target: StorageTarget,
    /// Refreshed provider snapshot after the mutation.
    pub providers: Vec<ProviderEntry>,
    /// Whether model readiness should be refetched.
    pub models_may_have_changed: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One usage window of the pinned `ChatGPT` usage snapshot.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageWindow {
    /// Window label such as `five_hours`.
    pub label: String,
    /// Consumed share of the window when reported.
    #[serde(rename = "usedPercent")]
    pub used_percent: Option<f64>,
    /// Window length in minutes when bounded.
    #[serde(rename = "windowDurationMins")]
    pub window_duration_mins: Option<u64>,
    /// Epoch-millisecond reset instant when bounded.
    #[serde(rename = "resetsAt")]
    pub resets_at: Option<u64>,
}

/// Credit balance block of the pinned `ChatGPT` usage snapshot.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageCredits {
    /// Display balance when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
    /// Remaining credit count when reported.
    #[serde(rename = "availableCount", default, skip_serializing_if = "Option::is_none")]
    pub available_count: Option<u64>,
    /// Whether credits remain when reported.
    #[serde(rename = "hasCredits", default, skip_serializing_if = "Option::is_none")]
    pub has_credits: Option<bool>,
    /// Whether the plan is unlimited when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlimited: Option<bool>,
}

/// Individual rate-limit block of the pinned `ChatGPT` usage snapshot.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageIndividualLimit {
    /// Human-readable limit bound.
    pub limit: String,
    /// Human-readable consumed amount.
    pub used: String,
    /// Remaining share of the limit.
    #[serde(rename = "remainingPercent")]
    pub remaining_percent: f64,
    /// Epoch-millisecond reset instant.
    #[serde(rename = "resetsAt")]
    pub resets_at: u64,
}

/// Credential-free `ChatGPT` usage snapshot delivered by the injected reader.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageSnapshot {
    /// Connected provider alias the snapshot describes.
    #[serde(rename = "providerName")]
    pub provider_name: String,
    /// Reader-side fetch timestamp.
    #[serde(rename = "fetchedAt")]
    pub fetched_at: String,
    /// Human-readable summary line.
    pub summary: String,
    /// Plan tier when reported.
    #[serde(rename = "planType", default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    /// Whether the plan limit is reached when reported.
    #[serde(rename = "limitReached", default, skip_serializing_if = "Option::is_none")]
    pub limit_reached: Option<bool>,
    /// Which rate limit is reached when reported.
    #[serde(
        rename = "rateLimitReachedType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub rate_limit_reached_type: Option<String>,
    /// Primary rolling window when bounded.
    pub primary: Option<UsageWindow>,
    /// Secondary rolling window when bounded.
    pub secondary: Option<UsageWindow>,
    /// Additional tracked windows.
    pub additional: Vec<UsageWindow>,
    /// Credit balance block when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<UsageCredits>,
    /// Individual rate-limit block when reported.
    #[serde(rename = "individualLimit", default, skip_serializing_if = "Option::is_none")]
    pub individual_limit: Option<UsageIndividualLimit>,
}

/// Typed credential-free usage failure payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UsageError {
    /// Stable failure class from the pinned code set.
    pub code: String,
    /// Human-readable scrubbed detail.
    pub message: String,
    /// Suggested retry delay when the failure is transient.
    #[serde(rename = "retryAfterMs", default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// Pinned `chatgpt_usage_read_response` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChatgptUsageReadResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Source the read attempted.
    pub target: UsageTarget,
    /// Snapshot on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageSnapshot>,
    /// Typed failure on rejection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<UsageError>,
}

/// Pinned `update_model_response` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateModelResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Scope the update addressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeScopeRef>,
    /// Persistence level reached on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_to: Option<String>,
    /// Resolved model identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Resolved `provider/model` handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_handle: Option<String>,
    /// Applied canonical settings on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<ModelSettings>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `update_toolset_response` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateToolsetResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Scope the update addressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeScopeRef>,
    /// Concrete toolset the preference resolved to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_toolset: Option<String>,
    /// Preference persisted for the scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_toolset_preference: Option<ToolsetPreference>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The seven outbound models/providers group messages.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum ModelsMessage {
    /// Model listing result.
    #[serde(rename = "list_models_response")]
    ListModelsResponse(ListModelsResponseMessage),
    /// Provider listing result.
    #[serde(rename = "list_connect_providers_response")]
    ListConnectProvidersResponse(ListConnectProvidersResponseMessage),
    /// Connect result with refreshed snapshot.
    #[serde(rename = "connect_provider_response")]
    ConnectProviderResponse(ConnectProviderResponseMessage),
    /// Disconnect result with refreshed snapshot.
    #[serde(rename = "disconnect_provider_response")]
    DisconnectProviderResponse(DisconnectProviderResponseMessage),
    /// Usage read result.
    #[serde(rename = "chatgpt_usage_read_response")]
    ChatgptUsageReadResponse(Box<ChatgptUsageReadResponseMessage>),
    /// Model update result.
    #[serde(rename = "update_model_response")]
    UpdateModelResponse(UpdateModelResponseMessage),
    /// Toolset update result.
    #[serde(rename = "update_toolset_response")]
    UpdateToolsetResponse(UpdateToolsetResponseMessage),
}

/// Pinned usage failure class: the backend was unreachable.
const USAGE_ERROR_NETWORK: &str = "network_error";
/// Pinned usage failure class: the requested source is unsupported.
const USAGE_ERROR_UNSUPPORTED_TARGET: &str = "unsupported_target";
/// Pinned usage failure class: no `ChatGPT` provider is connected.
const USAGE_ERROR_NOT_CONNECTED: &str = "not_connected";
