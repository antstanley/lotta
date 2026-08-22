//! WebSocket models/providers command group.
//!
//! Decodes and routes the seven §WebSocket command groups Models/providers
//! discriminants — `list_models`, `list_connect_providers`,
//! `connect_provider`, `disconnect_provider`, `chatgpt_usage_read`,
//! `update_model`, and `update_toolset` (the pinned fixture lists seven tags
//! for this row even though the plan prose says "six") — over the existing
//! Task 52 connection unit, Task 47 model catalog/update service, and Task 32
//! toolset registry. Nothing here re-implements those units: connect and
//! disconnect call
//! [`ConnectionManager`](lotta_providers::connections::ConnectionManager),
//! whose disconnect inherits the active-turn guard and the forced
//! cancel-then-remove ordering, update model calls
//! [`ModelUpdateService`](lotta_providers::model::ModelUpdateService) so
//! availability is checked before any persistence, and update toolset resolves
//! through [`ToolsetPreference`](lotta_tools::ToolsetPreference) over the six
//! canonical toolset identifiers.
//!
//! No response of this group carries credential material: provider state is
//! reported through
//! [`ConnectionSnapshot`](lotta_providers::connections::ConnectionSnapshot)-shaped
//! connection states that expose the authentication method only, and model
//! listings are Task 47 [`ListedModel`](lotta_providers::model::ListedModel)
//! projections whose readiness derives from connection state.
//!
//! Every mutating response carries the refreshed provider snapshot per the
//! pinned semantics (connect/disconnect embed the full provider list plus
//! `models_may_have_changed`; update responses embed the applied runtime
//! state). OAuth connection establishment stays in the provider host (Task
//! 51); this group initiates and reports, so `chatgpt_usage_read` consults
//! the injected usage reader and answers typed failures when no usable
//! source exists.
//!
//! ponytail: the connection adapter validates structurally only — live
//! endpoint validation needs the Task 51 compatibility host in server
//! config. ponytail: the usage reader defaults to typed failures because no
//! usage backend client exists yet; inject one when the host lands.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
};

use lotta_domain::Clock;
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_providers::connections::TurnCancellation;
use lotta_providers::connections::{
    ActiveTurnRegistry, AuthMethod, ConnectField, ConnectProviderAuthMethod, ConnectProviderInput,
    ConnectionAdapter, ConnectionError, ConnectionManager, ConnectionSnapshot,
    DisconnectProviderInput, ProviderAuth, ProviderAuthStore, ProviderSecret,
    baseline_bedrock_auth_methods, default_api_key_fields, google_vertex_fields, llama_cpp_fields,
    lmstudio_fields, ollama_fields, openai_compatible_fields,
};
use lotta_providers::model::{
    ConnectionReadiness, ListedModel, ModelAvailability, ModelCatalog, ModelHandle, ModelSettings,
    ModelStore, ModelUpdateError, ModelUpdateService, StoredModel,
};
use lotta_runtime::ListenerRuntime;
use lotta_tools::ToolsetPreference;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{bounds::WS_FRAME_BYTES_MAX, errors::ProtocolErrorEnvelope, framing::DecodedFrame};

use super::connection::ConnectionId;

/// Stable local provider record identifier prefix (pinned auth.json shape).
const RECORD_ID_PREFIX: &str = "local-provider-";
/// Provider type served by the compatibility host for `ChatGPT` plan usage.
const CHATGPT_USAGE_PROVIDER_TYPE: &str = "chatgpt_oauth";
/// Default connected `ChatGPT` provider alias when a command names none.
const CHATGPT_USAGE_DEFAULT_PROVIDER: &str = "chatgpt-plus-pro";
/// Scrubbed failure detail for rejected or failed provider listings.
/// Scrubbed failure detail for rejected or failed connects.
/// Scrubbed failure detail for rejected or failed disconnects.
/// Pinned unresolvable-model rejection text.
const MODEL_NOT_FOUND: &str =
    "Model not found. Provide a valid model_id from list_models or a model_handle.";
/// Typed unavailable-model rejection (validated before persistence).
const MODEL_UNAVAILABLE: &str = "model unavailable";
/// Scrubbed failure detail for failed model persistence.
const MODEL_UPDATE_FAILURE: &str = "Failed to update model";
/// Pinned usage failure message when the usage backend is unreachable.
const USAGE_BACKEND_UNAVAILABLE: &str = "Failed to read ChatGPT usage.";
/// Pinned usage failure message when no `ChatGPT` provider is connected.
const USAGE_NOT_CONNECTED: &str = "Connect a ChatGPT provider to read usage.";

include!("models_wire.rs");

/// Push callback delivering one outbound message to one connection.
pub type ModelsForwarder = Arc<
    dyn Fn(ConnectionId, ModelsMessage) -> Result<(), crate::error::AppServerError> + Send + Sync,
>;

/// Usage snapshot source for `chatgpt_usage_read` behind a connected provider.
pub trait ChatGptUsageReader: Send + Sync {
    /// Reads one usage snapshot without exposing provider credentials.
    ///
    /// # Errors
    /// Returns the pinned typed failure payload without secret material.
    fn read(&self, provider_name: &str, force_refresh: bool) -> Result<UsageSnapshot, UsageError>;
}

/// Production usage reader: no usage backend client exists yet.
struct UnreadUsageReader;

impl ChatGptUsageReader for UnreadUsageReader {
    fn read(
        &self,
        _provider_name: &str,
        _force_refresh: bool,
    ) -> Result<UsageSnapshot, UsageError> {
        Err(UsageError {
            code: USAGE_ERROR_NETWORK.to_owned(),
            message: USAGE_BACKEND_UNAVAILABLE.to_owned(),
            retry_after_ms: None,
        })
    }
}

/// Structural connection validation until the Task 51 host joins server config.
struct LocalConnectionAdapter;

impl ConnectionAdapter for LocalConnectionAdapter {
    fn validate<'a>(
        &'a self,
        _provider_type: &'a str,
        _auth: &'a ProviderAuth,
        _fields: &'a BTreeMap<String, String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + Send + 'a>>
    {
        Box::pin(async { Ok(()) })
    }
}

/// Safe forced-disconnect fallback: refuses cancellation it cannot settle.
struct UnsettleableTurnCancellation;

impl TurnCancellation for UnsettleableTurnCancellation {
    fn cancel_and_wait<'a>(
        &'a self,
        _runtime: &'a mut ListenerRuntime,
        _handle: &'a lotta_runtime::RuntimeHandle,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + Send + 'a>>
    {
        Box::pin(async { Err(ConnectionError::Cancellation) })
    }
}

/// Newtype so a shared adapter object satisfies the Task 52 generic bound.
struct GroupAdapter(Arc<dyn ConnectionAdapter>);

impl ConnectionAdapter for GroupAdapter {
    fn validate<'a>(
        &'a self,
        provider_type: &'a str,
        auth: &'a ProviderAuth,
        fields: &'a BTreeMap<String, String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + Send + 'a>>
    {
        self.0.validate(provider_type, auth, fields)
    }
}

/// Newtype so a shared cancellation object satisfies the Task 52 generic bound.
struct GroupCancellation(Arc<dyn TurnCancellation>);

impl TurnCancellation for GroupCancellation {
    fn cancel_and_wait<'a>(
        &'a self,
        runtime: &'a mut ListenerRuntime,
        handle: &'a lotta_runtime::RuntimeHandle,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + Send + 'a>>
    {
        self.0.cancel_and_wait(runtime, handle)
    }
}

/// Per-scope stored model cell implementing the Task 47 persistence port.
///
/// The inner value stays unset until one validated update persists; hints and
/// settings then derive from the stored state instead of a fabricated handle.
struct ModelCell(std::sync::Mutex<Option<StoredModel>>);

impl ModelStore for ModelCell {
    fn replace_model(
        &self,
        expected: u64,
        handle: ModelHandle,
        settings: ModelSettings,
    ) -> Result<StoredModel, ModelUpdateError> {
        let mut state = self.0.lock().map_err(|_| ModelUpdateError::Store)?;
        if state
            .as_ref()
            .is_some_and(|stored| stored.revision != expected)
        {
            return Err(ModelUpdateError::RevisionConflict);
        }
        let stored = StoredModel {
            handle,
            settings,
            revision: expected.checked_add(1).ok_or(ModelUpdateError::Store)?,
        };
        *state = Some(stored.clone());
        Ok(stored)
    }
}

/// Availability port projecting the Task 47 catalog over connection state.
struct CatalogAvailability {
    entries: Vec<ListedModel>,
}

impl ModelAvailability for CatalogAvailability {
    fn is_available(
        &self,
        handle: &ModelHandle,
    ) -> Result<bool, lotta_providers::model::AvailabilityError> {
        Ok(self
            .entries
            .iter()
            .any(|model| &model.handle == handle && model.readiness == ConnectionReadiness::Ready))
    }
}

/// Applies wire models/providers commands to Task 47, 52, and 32 units.
pub struct ModelsBridge {
    manager: Arc<Mutex<ConnectionManager>>,
    turns: Arc<Mutex<ActiveTurnRegistry>>,
    runtime: Arc<Mutex<ListenerRuntime>>,
    adapter: Arc<GroupAdapter>,
    cancellation: Arc<GroupCancellation>,
    catalog: ModelCatalog,
    models: Mutex<HashMap<String, Arc<ModelCell>>>,
    toolsets: Mutex<HashMap<String, ToolsetPreference>>,
    usage: Arc<dyn ChatGptUsageReader>,
    clock: Arc<dyn Clock + Send + Sync>,
    forward: ModelsForwarder,
}

impl ModelsBridge {
    /// Creates the production bridge over one canonical local storage root.
    ///
    /// # Errors
    /// Returns [`crate::error::AppServerError::Config`] when the provider
    /// connection store cannot be prepared or loaded.
    pub fn new(
        forward: ModelsForwarder,
        storage_dir: &Path,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, crate::error::AppServerError> {
        let paths = lotta_store::StorePaths::new(storage_dir)
            .map_err(|_| crate::error::AppServerError::Config("provider store root invalid"))?;
        let manager = ConnectionManager::load(ProviderAuthStore::new(paths))
            .map_err(|_| crate::error::AppServerError::Config("provider connections unloadable"))?;
        Ok(Self::compose(
            forward,
            manager,
            ActiveTurnRegistry::default(),
            ListenerRuntime::new(),
            Arc::new(LocalConnectionAdapter),
            Arc::new(UnsettleableTurnCancellation),
            ModelCatalog::default(),
            Arc::new(UnreadUsageReader),
            clock,
        ))
    }

    /// Composes a bridge from explicit Task 47/52/32 parts (test seam).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn compose(
        forward: ModelsForwarder,
        manager: ConnectionManager,
        turns: ActiveTurnRegistry,
        runtime: ListenerRuntime,
        adapter: Arc<dyn ConnectionAdapter>,
        cancellation: Arc<dyn TurnCancellation>,
        catalog: ModelCatalog,
        usage: Arc<dyn ChatGptUsageReader>,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Self {
        Self {
            manager: Arc::new(Mutex::new(manager)),
            turns: Arc::new(Mutex::new(turns)),
            runtime: Arc::new(Mutex::new(runtime)),
            adapter: Arc::new(GroupAdapter(adapter)),
            cancellation: Arc::new(GroupCancellation(cancellation)),
            catalog,
            models: Mutex::new(HashMap::new()),
            toolsets: Mutex::new(HashMap::new()),
            usage,
            clock,
            forward,
        }
    }

    /// Routes one decoded command in a detached task, like the pinned listener.
    pub fn handle(self: &Arc<Self>, connection: ConnectionId, command: &ModelsCommand) {
        let this = Arc::clone(self);
        let command = command.clone();
        tokio::spawn(async move { this.apply(connection, &command).await });
    }

    /// Applies one command inline, emitting responses through the forwarder.
    pub async fn apply(&self, connection: ConnectionId, command: &ModelsCommand) {
        match command {
            ModelsCommand::ListModels(payload) => self.list_models(connection, payload).await,
            ModelsCommand::ListConnectProviders(payload) => {
                self.list_connect_providers(connection, payload).await;
            }
            ModelsCommand::ConnectProvider(payload) => {
                self.connect_provider(connection, payload).await;
            }
            ModelsCommand::DisconnectProvider(payload) => {
                self.disconnect_provider(connection, payload).await;
            }
            ModelsCommand::ChatgptUsageRead(payload) => {
                self.chatgpt_usage_read(connection, payload).await;
            }
            ModelsCommand::UpdateModel(payload) => self.update_model(connection, payload).await,
            ModelsCommand::UpdateToolset(payload) => {
                self.update_toolset(connection, payload).await;
            }
        }
    }

    /// Active-turn registry for callers that attach provider turns.
    #[cfg(test)]
    pub(crate) fn turns(&self) -> &Arc<Mutex<ActiveTurnRegistry>> {
        &self.turns
    }

    /// Bridge-owned runtime registry whose handles back attached turns.
    #[cfg(test)]
    pub(crate) fn runtime(&self) -> &Arc<Mutex<ListenerRuntime>> {
        &self.runtime
    }

    /// Stored model for one scope key, for update-validation assertions.
    #[cfg(test)]
    pub(crate) async fn stored_model(&self, scope_key: &str) -> Option<StoredModel> {
        let models = self.models.lock().await;
        match models.get(scope_key) {
            Some(cell) => cell.snapshot().1,
            None => None,
        }
    }

    /// Stored toolset preference for one scope key, for persistence assertions.
    #[cfg(test)]
    pub(crate) async fn stored_toolset(&self, scope_key: &str) -> Option<ToolsetPreference> {
        self.toolsets.lock().await.get(scope_key).copied()
    }

    async fn list_models(&self, connection: ConnectionId, command: &ListModelsCommand) {
        let (listed, aliases) = self.projected_catalog().await;
        let entries: Vec<ModelEntry> = listed
            .iter()
            .map(|model| ModelEntry {
                id: model.handle.to_string(),
                handle: model.handle.to_string(),
                label: model.handle.to_string(),
                description: String::new(),
                readiness: model.readiness,
                model_settings: model.model_settings.clone(),
            })
            .collect();
        let available: Vec<String> = listed
            .iter()
            .filter(|model| model.readiness == ConnectionReadiness::Ready)
            .map(|model| model.handle.to_string())
            .collect();
        self.emit(
            connection,
            ModelsMessage::ListModelsResponse(ListModelsResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                entries,
                available_handles: available,
                byok_provider_aliases: aliases,
                error: None,
            }),
        );
    }

    /// Catalog entries with readiness plus provider-name aliases, one lock.
    async fn projected_catalog(&self) -> (Vec<ListedModel>, BTreeMap<String, String>) {
        let manager = self.manager.lock().await;
        let snapshots = manager.snapshots();
        let mut readiness = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        for snapshot in &snapshots {
            readiness.insert(snapshot.provider_type.clone(), ConnectionReadiness::Ready);
            readiness.insert(snapshot.provider_name.clone(), ConnectionReadiness::Ready);
            aliases.insert(
                snapshot.provider_name.clone(),
                snapshot.provider_type.clone(),
            );
        }
        (self.catalog.list_models(&readiness), aliases)
    }

    async fn list_connect_providers(
        &self,
        connection: ConnectionId,
        command: &ListConnectProvidersCommand,
    ) {
        let providers = self.provider_entries().await;
        self.emit(
            connection,
            ModelsMessage::ListConnectProvidersResponse(ListConnectProvidersResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                target: command.target,
                providers,
                error: None,
            }),
        );
    }

    /// Declarative rows merged with live connection states, one lock.
    async fn provider_entries(&self) -> Vec<ProviderEntry> {
        let manager = self.manager.lock().await;
        let snapshots = manager.snapshots();
        let mut entries = declarative_rows()
            .into_iter()
            .map(|row| row.entry(&snapshots))
            .collect::<Vec<_>>();
        for snapshot in &snapshots {
            if !declarative_rows()
                .iter()
                .any(|row| row.provider_type == snapshot.provider_type)
            {
                entries.push(orphan_entry(snapshot));
            }
        }
        entries
    }

    async fn connect_provider(&self, connection: ConnectionId, command: &ConnectProviderCommand) {
        let row = declarative_rows()
            .into_iter()
            .find(|row| row.id == command.provider_id);
        let Some(row) = row else {
            self.emit_connect_failure(
                connection,
                &command.request_id,
                &format!("Unknown provider: {}", command.provider_id),
            );
            return;
        };
        let now = self.clock.now().to_string();
        match row.resolve(command, now) {
            Ok((input, provider_type)) => {
                self.connect_resolved(connection, command, input, provider_type)
                    .await;
            }
            Err(error) => {
                tracing::info!(provider_id = %command.provider_id, "provider connect rejected");
                self.emit_connect_failure(connection, &command.request_id, &error);
            }
        }
    }

    /// Persists one structurally valid connect through the Task 52 manager.
    async fn connect_resolved(
        &self,
        connection: ConnectionId,
        command: &ConnectProviderCommand,
        input: ConnectProviderInput,
        provider_type: String,
    ) {
        match self.connect_through_manager(input).await {
            Ok(_) => {
                let providers = self.provider_entries().await;
                self.emit(
                    connection,
                    ModelsMessage::ConnectProviderResponse(ConnectProviderResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        target: command.target,
                        providers,
                        models_may_have_changed: true,
                        error: None,
                    }),
                );
            }
            Err(error) => {
                tracing::info!(provider_id = %command.provider_id, provider_type, "provider connect rejected");
                self.emit_connect_failure(connection, &command.request_id, &error.to_string());
            }
        }
    }

    fn emit_connect_failure(&self, connection: ConnectionId, request_id: &str, error: &str) {
        self.emit(
            connection,
            ModelsMessage::ConnectProviderResponse(ConnectProviderResponseMessage {
                request_id: request_id.to_owned(),
                success: false,
                target: StorageTarget::Local,
                providers: Vec::new(),
                models_may_have_changed: false,
                error: Some(error.to_owned()),
            }),
        );
    }

    async fn connect_through_manager(
        &self,
        input: ConnectProviderInput,
    ) -> Result<ConnectionSnapshot, ConnectionError> {
        let mut manager = self.manager.lock().await;
        let input = ConnectProviderInput {
            expected_revision: manager.revision(),
            ..input
        };
        manager.connect(input, self.adapter.as_ref()).await
    }

    async fn disconnect_provider(
        &self,
        connection: ConnectionId,
        command: &DisconnectProviderCommand,
    ) {
        let input = DisconnectProviderInput {
            provider_id: format!("{RECORD_ID_PREFIX}{}", command.provider_id),
            provider_name: command.provider_name.clone(),
            force: command.force.unwrap_or(false),
            expected_revision: 0,
        };
        let outcome = {
            let mut manager = self.manager.lock().await;
            let mut turns = self.turns.lock().await;
            let mut runtime = self.runtime.lock().await;
            let input = DisconnectProviderInput {
                expected_revision: manager.revision(),
                ..input
            };
            manager
                .disconnect(input, &mut turns, &mut runtime, self.cancellation.as_ref())
                .await
        };
        match outcome {
            Ok(()) => {
                let providers = self.provider_entries().await;
                self.emit(
                    connection,
                    ModelsMessage::DisconnectProviderResponse(DisconnectProviderResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        target: command.target,
                        providers,
                        models_may_have_changed: true,
                        error: None,
                    }),
                );
            }
            Err(error) => {
                tracing::warn!(provider_id = %command.provider_id, error = %error, "provider disconnect rejected");
                self.emit(
                    connection,
                    ModelsMessage::DisconnectProviderResponse(DisconnectProviderResponseMessage {
                        request_id: command.request_id.clone(),
                        success: false,
                        target: command.target,
                        providers: Vec::new(),
                        models_may_have_changed: false,
                        error: Some(error.to_string()),
                    }),
                );
            }
        }
    }

    async fn chatgpt_usage_read(
        &self,
        connection: ConnectionId,
        command: &ChatgptUsageReadCommand,
    ) {
        if matches!(command.target, UsageTarget::Api) {
            self.emit_usage_failure(
                connection,
                &command.request_id,
                command.target,
                USAGE_ERROR_UNSUPPORTED_TARGET,
                "Only the local provider store serves usage.",
            );
            return;
        }
        let Some(provider_name) = self
            .connected_chatgpt(command.provider_name.as_deref())
            .await
        else {
            self.emit_usage_failure(
                connection,
                &command.request_id,
                command.target,
                USAGE_ERROR_NOT_CONNECTED,
                USAGE_NOT_CONNECTED,
            );
            return;
        };
        match self
            .usage
            .read(&provider_name, command.force_refresh.unwrap_or(false))
        {
            Ok(usage) => self.emit(
                connection,
                ModelsMessage::ChatgptUsageReadResponse(Box::new(
                    ChatgptUsageReadResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        target: command.target,
                        usage: Some(usage),
                        error: None,
                    },
                )),
            ),
            Err(error) => self.emit(
                connection,
                ModelsMessage::ChatgptUsageReadResponse(Box::new(
                    ChatgptUsageReadResponseMessage {
                        request_id: command.request_id.clone(),
                        success: false,
                        target: command.target,
                        usage: None,
                        error: Some(error),
                    },
                )),
            ),
        }
    }

    /// Connected `ChatGPT` record name, honoring the command alias override.
    async fn connected_chatgpt(&self, requested: Option<&str>) -> Option<String> {
        let manager = self.manager.lock().await;
        manager
            .snapshots()
            .into_iter()
            .find(|snapshot| {
                snapshot.provider_type == CHATGPT_USAGE_PROVIDER_TYPE
                    && snapshot.is_connected
                    && requested.is_none_or(|name| name == snapshot.provider_name)
                    || (snapshot.provider_name == CHATGPT_USAGE_DEFAULT_PROVIDER
                        && snapshot.is_connected
                        && requested.is_none())
            })
            .map(|snapshot| snapshot.provider_name)
    }

    fn emit_usage_failure(
        &self,
        connection: ConnectionId,
        request_id: &str,
        target: UsageTarget,
        code: &str,
        message: &str,
    ) {
        self.emit(
            connection,
            ModelsMessage::ChatgptUsageReadResponse(Box::new(ChatgptUsageReadResponseMessage {
                request_id: request_id.to_owned(),
                success: false,
                target,
                usage: None,
                error: Some(UsageError {
                    code: code.to_owned(),
                    message: message.to_owned(),
                    retry_after_ms: None,
                }),
            })),
        );
    }

    async fn update_model(&self, connection: ConnectionId, command: &UpdateModelCommand) {
        let Some(handle) = resolve_handle(&command.payload) else {
            self.emit_update_model_failure(connection, command, MODEL_NOT_FOUND);
            return;
        };
        let scope_key = scope_key(&command.runtime);
        let cell = self.cell_for(&scope_key).await;
        let (expected, current) = cell.snapshot();
        let base = current.map(|stored| stored.settings).unwrap_or_default();
        let settings = merged_settings(&base, &command.payload);
        let entries = self.projected_catalog().await.0;
        let availability = CatalogAvailability { entries };
        let service = ModelUpdateService::new(&availability, cell.as_ref());
        match service.update(expected, handle.clone(), settings) {
            Ok(stored) => {
                self.toolsets.lock().await.remove(&scope_key);
                self.emit(
                    connection,
                    ModelsMessage::UpdateModelResponse(UpdateModelResponseMessage {
                        request_id: command.request_id.clone(),
                        success: true,
                        runtime: Some(command.runtime.clone()),
                        applied_to: Some(applied_to(&command.runtime).to_owned()),
                        model_id: Some(
                            command
                                .payload
                                .model_id
                                .clone()
                                .unwrap_or_else(|| handle.to_string()),
                        ),
                        model_handle: Some(handle.to_string()),
                        model_settings: Some(stored.settings),
                        error: None,
                    }),
                );
            }
            Err(error) => {
                let text = match error {
                    ModelUpdateError::Unavailable => MODEL_UNAVAILABLE,
                    _ => MODEL_UPDATE_FAILURE,
                };
                self.emit_update_model_failure(connection, command, text);
            }
        }
    }

    fn emit_update_model_failure(
        &self,
        connection: ConnectionId,
        command: &UpdateModelCommand,
        error: &str,
    ) {
        self.emit(
            connection,
            ModelsMessage::UpdateModelResponse(UpdateModelResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                runtime: Some(command.runtime.clone()),
                applied_to: None,
                model_id: command.payload.model_id.clone(),
                model_handle: command.payload.model_handle.clone(),
                model_settings: None,
                error: Some(error.to_owned()),
            }),
        );
    }

    async fn update_toolset(&self, connection: ConnectionId, command: &UpdateToolsetCommand) {
        let scope_key = scope_key(&command.runtime);
        let hints = self.stored_hints(&scope_key).await;
        let resolved = command
            .toolset_preference
            .resolve(hints.0.as_deref(), hints.1.as_deref());
        self.toolsets
            .lock()
            .await
            .insert(scope_key, command.toolset_preference);
        self.emit(
            connection,
            ModelsMessage::UpdateToolsetResponse(UpdateToolsetResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                runtime: Some(command.runtime.clone()),
                current_toolset: Some(resolved.as_str().to_owned()),
                current_toolset_preference: Some(command.toolset_preference),
                error: None,
            }),
        );
    }

    /// `(provider, model)` hints from the scope's stored handle.
    async fn stored_hints(&self, scope_key: &str) -> (Option<String>, Option<String>) {
        let (_, stored) = self.cell_for(scope_key).await.snapshot();
        let Some(handle) = stored.map(|model| model.handle.to_string()) else {
            return (None, None);
        };
        match handle.split_once('/') {
            Some((provider, model)) => (Some(provider.to_owned()), Some(model.to_owned())),
            None => (None, None),
        }
    }

    /// Per-scope model cell, created empty on first touch.
    async fn cell_for(&self, scope_key: &str) -> Arc<ModelCell> {
        let mut models = self.models.lock().await;
        models
            .entry(scope_key.to_owned())
            .or_insert_with(|| Arc::new(ModelCell(std::sync::Mutex::new(None))))
            .clone()
    }

    fn emit(&self, connection: ConnectionId, message: ModelsMessage) {
        match serde_json::to_string(&message) {
            Ok(body) if body.len() <= WS_FRAME_BYTES_MAX => {
                let _ = (self.forward)(connection, message);
            }
            Ok(_) => tracing::warn!("models response exceeded the frame bound and was dropped"),
            Err(_) => tracing::warn!("models response failed to encode"),
        }
    }
}

impl ModelCell {
    /// `(revision, stored)` snapshot of the cell under one short lock.
    fn snapshot(&self) -> (u64, Option<StoredModel>) {
        let state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            Some(stored) => (stored.revision, Some(stored.clone())),
            None => (0, None),
        }
    }
}

fn resolve_handle(payload: &UpdateModelPayload) -> Option<ModelHandle> {
    payload
        .model_handle
        .as_ref()
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            payload
                .model_id
                .as_ref()
                .filter(|value| !value.is_empty())
                .and_then(|value| value.parse().ok())
        })
}

fn merged_settings(current: &ModelSettings, payload: &UpdateModelPayload) -> ModelSettings {
    let Some(effort) = payload
        .reasoning_effort
        .as_ref()
        .filter(|value| !value.is_empty())
    else {
        return current.clone();
    };
    let overlay = ModelSettings::normalize(&serde_json::json!({ "reasoning_effort": effort }))
        .unwrap_or_default();
    current.merged_with(&overlay)
}

fn scope_key(runtime: &RuntimeScopeRef) -> String {
    format!("{}/{}", runtime.agent_id, runtime.conversation_id)
}

fn applied_to(runtime: &RuntimeScopeRef) -> &'static str {
    if runtime.conversation_id == "default" {
        "agent"
    } else {
        "conversation"
    }
}

/// One declarative connectable provider row served by this group.
struct CatalogRow {
    id: &'static str,
    display_name: &'static str,
    description: &'static str,
    provider_type: &'static str,
    requires_api_key: bool,
}

fn declarative_rows() -> Vec<CatalogRow> {
    vec![
        CatalogRow {
            id: "openai",
            display_name: "OpenAI",
            description: "Connect an OpenAI API key",
            provider_type: "openai",
            requires_api_key: true,
        },
        CatalogRow {
            id: "anthropic",
            display_name: "Claude API",
            description: "Connect an Anthropic API key",
            provider_type: "anthropic",
            requires_api_key: true,
        },
        CatalogRow {
            id: "google-vertex",
            display_name: "Google Vertex AI",
            description: "Connect a Google Vertex AI API key",
            provider_type: "google_vertex",
            requires_api_key: true,
        },
        CatalogRow {
            id: "openai-compatible",
            display_name: "OpenAI-compatible API",
            description: "Connect an OpenAI-compatible Chat Completions endpoint",
            provider_type: "openai-compatible",
            requires_api_key: false,
        },
        CatalogRow {
            id: "ollama",
            display_name: "Ollama (local)",
            description: "Connect Ollama at http://localhost:11434/v1 or a remote URL",
            provider_type: "ollama",
            requires_api_key: false,
        },
        CatalogRow {
            id: "lmstudio",
            display_name: "LM Studio (local)",
            description: "Connect LM Studio at http://127.0.0.1:1234/v1 or a remote URL",
            provider_type: "lmstudio",
            requires_api_key: false,
        },
        CatalogRow {
            id: "llama-cpp",
            display_name: "llama.cpp (local)",
            description: "Connect llama.cpp at http://localhost:8080/v1 or a remote URL",
            provider_type: "llama_cpp",
            requires_api_key: false,
        },
        CatalogRow {
            id: "bedrock",
            display_name: "Amazon Bedrock",
            description: "Connect to Claude on Amazon Bedrock",
            provider_type: "amazon-bedrock",
            requires_api_key: true,
        },
    ]
}

impl CatalogRow {
    fn record_id(&self) -> String {
        format!("{RECORD_ID_PREFIX}{}", self.id)
    }

    fn fields(&self) -> Option<Vec<ConnectField>> {
        match self.id {
            "openai" | "anthropic" => Some(default_api_key_fields()),
            "google-vertex" => Some(google_vertex_fields()),
            "openai-compatible" => Some(openai_compatible_fields()),
            "ollama" => Some(ollama_fields()),
            "lmstudio" => Some(lmstudio_fields()),
            "llama-cpp" => Some(llama_cpp_fields()),
            _ => None,
        }
    }

    fn auth_methods(&self) -> Option<Vec<ConnectProviderAuthMethod>> {
        (self.id == "bedrock").then(baseline_bedrock_auth_methods)
    }

    fn entry(&self, snapshots: &[ConnectionSnapshot]) -> ProviderEntry {
        let connected: Vec<ConnectionState> = snapshots
            .iter()
            .filter(|snapshot| snapshot.provider_name == self.id)
            .map(connection_state)
            .collect();
        ProviderEntry {
            id: self.id.to_owned(),
            display_name: self.display_name.to_owned(),
            description: self.description.to_owned(),
            provider_type: self.provider_type.to_owned(),
            provider_name: self.id.to_owned(),
            provider_names: vec![self.id.to_owned()],
            requires_api_key: self.requires_api_key,
            fields: self.fields(),
            auth_methods: self.auth_methods(),
            connected: connected
                .first()
                .cloned()
                .unwrap_or(ConnectionState::disconnected()),
            connected_providers: connected,
        }
    }

    /// Validates wire fields into a Task 52 connect input. `now` stamps the
    /// record for the store and must come from the bridge clock.
    fn resolve(
        &self,
        command: &ConnectProviderCommand,
        now: String,
    ) -> Result<(ConnectProviderInput, String), String> {
        let (auth, fields) = self.resolve_auth(command)?;
        let input = ConnectProviderInput {
            provider_id: self.record_id(),
            provider_name: self.id.to_owned(),
            provider_type: self.provider_type.to_owned(),
            auth_method: AuthMethod::Api,
            auth,
            fields,
            expected_revision: 0,
            now,
        };
        Ok((input, self.provider_type.to_owned()))
    }

    fn resolve_auth(
        &self,
        command: &ConnectProviderCommand,
    ) -> Result<(ProviderAuth, BTreeMap<String, String>), String> {
        if self.id == "bedrock" {
            return self.resolve_bedrock(command);
        }
        if command.auth_method_id.is_some() {
            return Err(format!("{} does not use auth methods.", self.display_name));
        }
        let fields = self.fields().unwrap_or_default();
        for field in &fields {
            if field.required && field_value(&command.fields, &field.key).is_none() {
                return Err(format!("Missing {}.", field.label));
            }
        }
        let key = match field_value(&command.fields, "apiKey") {
            Some(value) => value,
            None if self.requires_api_key => {
                return Err(format!("Missing {} API key.", self.display_name));
            }
            None => "not-needed".to_owned(),
        };
        let key = ProviderSecret::new(key).map_err(|_| "Provider API key rejected.".to_owned())?;
        Ok((
            ProviderAuth::Api {
                key,
                extras: BTreeMap::default().into_iter().collect(),
            },
            command.fields.clone(),
        ))
    }

    fn resolve_bedrock(
        &self,
        command: &ConnectProviderCommand,
    ) -> Result<(ProviderAuth, BTreeMap<String, String>), String> {
        let methods = self.auth_methods().unwrap_or_default();
        let Some(method_id) = command.auth_method_id.as_deref() else {
            return Err("Select an auth method for Amazon Bedrock.".to_owned());
        };
        let method = methods
            .iter()
            .find(|method| method.id == method_id)
            .ok_or_else(|| format!("Unknown auth method \"{method_id}\" for Amazon Bedrock."))?;
        for field in &method.fields {
            if field.required && field_value(&command.fields, &field.key).is_none() {
                return Err(format!("Missing {}.", field.label));
            }
        }
        let fields = command.fields.clone();
        if method_id == "profile" {
            Ok((
                ProviderAuth::BedrockProfile {
                    extras: serde_json::Map::default(),
                },
                fields,
            ))
        } else {
            let key = field_value(&command.fields, "apiKey")
                .ok_or_else(|| "Missing AWS Secret Access Key.".to_owned())?;
            let key =
                ProviderSecret::new(key).map_err(|_| "Provider API key rejected.".to_owned())?;
            Ok((
                ProviderAuth::Api {
                    key,
                    extras: serde_json::Map::default(),
                },
                fields,
            ))
        }
    }
}

fn field_value(fields: &BTreeMap<String, String>, key: &str) -> Option<String> {
    let value = fields.get(key)?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn connection_state(snapshot: &ConnectionSnapshot) -> ConnectionState {
    ConnectionState {
        is_connected: snapshot.is_connected,
        id: Some(snapshot.id.clone()),
        provider_name: Some(snapshot.provider_name.clone()),
        provider_type: Some(snapshot.provider_type.clone()),
        auth_type: Some(snapshot.auth_type),
        base_url: snapshot.base_url.clone(),
        timeout: snapshot.timeout.clone(),
        region: snapshot.region.clone(),
    }
}

fn orphan_entry(snapshot: &ConnectionSnapshot) -> ProviderEntry {
    ProviderEntry {
        id: snapshot.provider_type.clone(),
        display_name: snapshot.provider_type.clone(),
        description: String::new(),
        provider_type: snapshot.provider_type.clone(),
        provider_name: snapshot.provider_name.clone(),
        provider_names: vec![snapshot.provider_name.clone()],
        requires_api_key: true,
        fields: None,
        auth_methods: None,
        connected: connection_state(snapshot),
        connected_providers: vec![connection_state(snapshot)],
    }
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known models/providers
/// commands, including unknown toolset preferences or targets.
pub fn decode(frame: &DecodedFrame) -> Result<Option<ModelsCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let parsed = match tag {
        Tag::ListModels => typed::<ListModelsCommand>(frame).map(ModelsCommand::ListModels),
        Tag::ListConnectProviders => {
            typed::<ListConnectProvidersCommand>(frame).map(ModelsCommand::ListConnectProviders)
        }
        Tag::ConnectProvider => {
            typed::<ConnectProviderCommand>(frame).map(ModelsCommand::ConnectProvider)
        }
        Tag::DisconnectProvider => {
            typed::<DisconnectProviderCommand>(frame).map(ModelsCommand::DisconnectProvider)
        }
        Tag::ChatgptUsageRead => {
            typed::<ChatgptUsageReadCommand>(frame).map(ModelsCommand::ChatgptUsageRead)
        }
        Tag::UpdateModel => typed::<UpdateModelCommand>(frame).map(ModelsCommand::UpdateModel),
        Tag::UpdateToolset => {
            typed::<UpdateToolsetCommand>(frame).map(ModelsCommand::UpdateToolset)
        }
        _ => return Ok(None),
    }?;
    Ok(Some(parsed))
}

fn typed<T: serde::de::DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ProtocolErrorEnvelope> {
    serde_json::from_value(frame.value.clone()).map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "models_command_invalid",
        "invalid models command",
        frame.request_id.clone(),
    )
}

#[cfg(test)]
pub(crate) fn inert_forwarder() -> ModelsForwarder {
    Arc::new(|_, _| Ok(()))
}

#[cfg(test)]
#[path = "models_commands_tests.rs"]
mod commands;
#[cfg(test)]
#[path = "models_disconnect_tests.rs"]
mod disconnect;
#[cfg(test)]
#[path = "models_fixture_round_trip_tests.rs"]
mod fixture_round_trip;
#[cfg(test)]
#[path = "models_redaction_tests.rs"]
mod redaction;
#[cfg(test)]
#[path = "models_support.rs"]
mod support;
#[cfg(test)]
#[path = "models_updates_tests.rs"]
mod updates;
