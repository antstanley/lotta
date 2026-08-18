use super::disconnect::{self, ActiveTurnRegistry, TurnCancellation};
use super::{
    ConnectProviderInput, ConnectionError, ConnectionSnapshot, DisconnectProviderInput,
    PROVIDER_FIELDS_MAX, PROVIDER_TEXT_BYTES_MAX, PROVIDERS_MAX, ProviderAuth, ProviderAuthStore,
    ProviderRecord,
};
use crate::host::client::HostConfig;
use crate::host::protocol::{HostAuth, HostOptions};
use crate::host::provider::HostProvider;
use lotta_extensions::sidecar::SidecarOwnerIdentity;
use lotta_runtime::ListenerRuntime;
use lotta_runtime::boundary::{ProviderEventText, ProviderName};
use lotta_runtime::ports::{
    ProviderError, ProviderErrorContext, ProviderEventSink, ProviderPort, ProviderRequest,
};
use serde_json::Map;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

/// Adapter validation scope that borrows secret authentication without retaining it.
pub trait ConnectionAdapter: Send + Sync {
    /// Validates credentials and configuration before they become persistent.
    fn validate<'a>(
        &'a self,
        provider_type: &'a str,
        auth: &'a ProviderAuth,
        fields: &'a BTreeMap<String, String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConnectionError>> + Send + 'a>>;
}

/// Future returned by a connection adapter registry.
pub type ConnectionAdapterFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn ProviderPort>, ConnectionError>> + Send + 'a>>;

/// Factory/registry that resolves stored connection material into a real provider adapter.
pub trait ConnectionAdapterFactory: Send + Sync {
    /// Builds one provider adapter while taking ownership of scoped host authentication.
    fn build<'a>(
        &'a self,
        provider_type: &'a str,
        auth: HostAuth,
        options: HostOptions,
    ) -> ConnectionAdapterFuture<'a>;
}

/// Production Task51 host-backed connection adapter registry.
#[derive(Clone)]
pub struct HostConnectionAdapterFactory {
    config: HostConfig,
    owner: SidecarOwnerIdentity,
    options: BTreeMap<String, HostOptions>,
}

impl HostConnectionAdapterFactory {
    /// Creates a production factory from validated host launch configuration and ownership.
    #[must_use]
    pub fn new(config: HostConfig, owner: SidecarOwnerIdentity) -> Self {
        Self {
            config,
            owner,
            options: BTreeMap::new(),
        }
    }

    /// Registers deterministic provider-specific host options.
    #[must_use]
    pub fn with_options(mut self, provider_type: String, options: HostOptions) -> Self {
        self.options.insert(provider_type, options);
        self
    }
}

impl ConnectionAdapterFactory for HostConnectionAdapterFactory {
    fn build<'a>(
        &'a self,
        provider_type: &'a str,
        auth: HostAuth,
        mut options: HostOptions,
    ) -> ConnectionAdapterFuture<'a> {
        Box::pin(async move {
            if let Some(registered) = self.options.get(provider_type) {
                if options.base_url.is_none() {
                    options.base_url.clone_from(&registered.base_url);
                }
                options.env = registered.env.clone();
                options.headers = registered.headers.clone();
                options.provider = registered.provider.clone();
            }
            HostProvider::spawn(self.config.clone(), self.owner.clone(), auth, options)
                .await
                .map(|provider| Box::new(provider) as Box<dyn ProviderPort>)
                .map_err(|_| ConnectionError::Adapter)
        })
    }
}

impl ConnectionAdapter for HostConnectionAdapterFactory {
    fn validate<'a>(
        &'a self,
        provider_type: &'a str,
        auth: &'a ProviderAuth,
        fields: &'a BTreeMap<String, String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConnectionError>> + Send + 'a>> {
        Box::pin(async move {
            let record = validation_record(provider_type, auth, fields);
            let host_auth = host_auth_ref(&record)?;
            let options = HostOptions {
                base_url: record.base_url.clone(),
                ..HostOptions::default()
            };
            self.build(provider_type, host_auth, options)
                .await
                .map(|_| ())
        })
    }
}

/// Optimistic provider connection manager and redacted snapshot owner.
pub struct ConnectionManager {
    store: ProviderAuthStore,
    records: BTreeMap<String, ProviderRecord>,
    revision: u64,
    adapter_factory: Option<std::sync::Arc<dyn ConnectionAdapterFactory>>,
}

impl ConnectionManager {
    /// Loads the baseline auth store into one in-process optimistic manager.
    ///
    /// # Errors
    /// Returns a typed persistence or stored-shape failure.
    pub fn load(store: ProviderAuthStore) -> Result<Self, ConnectionError> {
        let (records, revision) = store.load()?;
        Ok(Self {
            store,
            records,
            revision,
            adapter_factory: None,
        })
    }

    /// Loads a manager with the production provider adapter registry/factory.
    ///
    /// # Errors
    /// Returns a typed persistence or stored-shape failure.
    pub fn load_with_factory(
        store: ProviderAuthStore,
        adapter_factory: std::sync::Arc<dyn ConnectionAdapterFactory>,
    ) -> Result<Self, ConnectionError> {
        let mut manager = Self::load(store)?;
        manager.adapter_factory = Some(adapter_factory);
        Ok(manager)
    }

    /// Loads persisted connections through the production pi-ai compatibility host.
    ///
    /// Provider-specific options are explicit and deterministic; no ambient provider settings
    /// are consulted. This is the production-facing constructor for hosted streaming.
    ///
    /// # Errors
    /// Returns a typed persistence or stored-shape failure.
    pub fn load_hosted(
        store: ProviderAuthStore,
        config: HostConfig,
        owner: SidecarOwnerIdentity,
        options: BTreeMap<String, HostOptions>,
    ) -> Result<Self, ConnectionError> {
        let factory = options.into_iter().fold(
            HostConnectionAdapterFactory::new(config, owner),
            |factory, (provider_type, options)| factory.with_options(provider_type, options),
        );
        Self::load_with_factory(store, std::sync::Arc::new(factory))
    }

    /// Returns the current monotonic persistence revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns credential-free, stable provider connection snapshots.
    #[must_use]
    pub fn snapshots(&self) -> Vec<ConnectionSnapshot> {
        self.records
            .values()
            .map(|record| ConnectionSnapshot {
                id: record.id.clone(),
                provider_name: record.name.clone(),
                provider_type: record.provider_type.clone(),
                auth_type: record.auth.method(),
                base_url: record.base_url.clone(),
                timeout: record.timeout.clone(),
                region: record.region.clone(),
                access_key: record.access_key.clone(),
                is_connected: true,
                revision: self.revision,
            })
            .collect()
    }

    /// Returns a scrubbed snapshot for one stable connection ID.
    #[must_use]
    pub fn snapshot(&self, provider_id: &str) -> Option<ConnectionSnapshot> {
        self.snapshots()
            .into_iter()
            .find(|snapshot| snapshot.id == provider_id)
    }

    /// Resolves secret authentication by stable connection ID for one adapter call.
    ///
    /// # Errors
    /// Returns [`ConnectionError::NotFound`] when the connection is absent.
    pub fn with_auth<T>(
        &self,
        provider_id: &str,
        use_auth: impl FnOnce(&ProviderAuth) -> T,
    ) -> Result<T, ConnectionError> {
        self.records
            .values()
            .find(|record| record.id == provider_id)
            .map(|record| use_auth(&record.auth))
            .ok_or(ConnectionError::NotFound)
    }

    /// Builds one adapter through an injected secret-scoped resolver.
    ///
    /// # Errors
    /// Returns a scrubbed connection error without reflecting credentials or vendor details.
    pub async fn build_with<T, F>(&self, provider_id: &str, build: F) -> Result<T, ConnectionError>
    where
        F: for<'a> FnOnce(
            &'a str,
            &'a ProviderAuth,
            Option<&'a str>,
        )
            -> Pin<Box<dyn Future<Output = Result<T, ConnectionError>> + Send + 'a>>,
    {
        let record = self
            .records
            .values()
            .find(|record| record.id == provider_id)
            .ok_or(ConnectionError::NotFound)?;
        build(
            &record.provider_type,
            &record.auth,
            record.base_url.as_deref(),
        )
        .await
    }

    /// Resolves a stored connection into a real stream without exposing its secret publicly.
    ///
    /// # Errors
    /// Returns a scrubbed unavailable failure for missing records, factories, or adapter errors.
    pub async fn stream(
        &self,
        provider_id: &str,
        request: ProviderRequest,
        events: ProviderEventSink,
    ) -> Result<(), ProviderError> {
        let factory = self
            .adapter_factory
            .as_ref()
            .ok_or_else(provider_unavailable)?;
        let record = self
            .records
            .values()
            .find(|record| record.id == provider_id)
            .ok_or_else(provider_unavailable)?;
        let auth = host_auth(record).map_err(|_| provider_unavailable())?;
        let options = HostOptions {
            base_url: record.base_url.clone(),
            ..HostOptions::default()
        };
        let adapter = factory
            .build(&record.provider_type, auth, options)
            .await
            .map_err(|_| provider_unavailable())?;
        if let Some(descriptor) = record.extras.get("host_descriptor") {
            adapter
                .register_host_adapter(descriptor.clone())
                .await
                .map_err(|_| provider_unavailable())?;
        }
        adapter
            .stream(request, events)
            .await
            .map_err(|_| provider_unavailable())
    }

    /// Validates then atomically persists one API or one-use OAuth connection payload.
    ///
    /// # Errors
    /// Returns typed validation, adapter, capacity, conflict, or persistence failures.
    pub async fn connect<A: ConnectionAdapter>(
        &mut self,
        input: ConnectProviderInput,
        adapter: &A,
    ) -> Result<ConnectionSnapshot, ConnectionError> {
        validate_input(
            &input,
            self.records.len(),
            self.records.contains_key(&input.provider_name),
        )?;
        if input.expected_revision != self.revision {
            return Err(ConnectionError::Conflict);
        }
        tracing::info!(
            provider_id = input.provider_id,
            auth_method = ?input.auth_method,
            marker = "provider.connect.validate",
            "validating provider connection"
        );
        adapter
            .validate(&input.provider_type, &input.auth, &input.fields)
            .await
            .map_err(|_| ConnectionError::Adapter)?;
        let existing = self.records.get(&input.provider_name);
        let record = record(input, existing);
        let name = record.name.clone();
        let prior = self.records.insert(name.clone(), record);
        match self.store.replace(&self.records, self.revision) {
            Ok(next) => self.revision = next,
            Err(error) => {
                match prior {
                    Some(value) => {
                        self.records.insert(name, value);
                    }
                    None => {
                        self.records.remove(&name);
                    }
                }
                return Err(error);
            }
        }
        tracing::info!(
            provider_name = name,
            marker = "provider.connect.persisted",
            "provider connected"
        );
        self.snapshots()
            .into_iter()
            .find(|snapshot| snapshot.provider_name == name)
            .ok_or(ConnectionError::NotFound)
    }

    /// Refuses active use or force-cancels through Task47 before atomic removal.
    ///
    /// # Errors
    /// Returns typed active-turn, cancellation, conflict, not-found, or persistence failures.
    pub async fn disconnect<C: TurnCancellation>(
        &mut self,
        input: DisconnectProviderInput,
        turns: &mut ActiveTurnRegistry,
        runtime: &mut ListenerRuntime,
        cancellation: &C,
    ) -> Result<(), ConnectionError> {
        disconnect::disconnect(
            &self.store,
            &mut self.records,
            &mut self.revision,
            input,
            turns,
            runtime,
            cancellation,
        )
        .await
    }
}

fn validate_input(
    input: &ConnectProviderInput,
    count: usize,
    replacing: bool,
) -> Result<(), ConnectionError> {
    for text in [
        &input.provider_id,
        &input.provider_name,
        &input.provider_type,
        &input.now,
    ] {
        if text.is_empty() || text.len() > PROVIDER_TEXT_BYTES_MAX {
            return Err(ConnectionError::InvalidInput("provider connection text"));
        }
    }
    if input.fields.len() > PROVIDER_FIELDS_MAX {
        return Err(ConnectionError::InvalidInput("provider connection fields"));
    }
    for (key, value) in &input.fields {
        if key.is_empty()
            || key.len() > PROVIDER_TEXT_BYTES_MAX
            || value.len() > PROVIDER_TEXT_BYTES_MAX
        {
            return Err(ConnectionError::InvalidInput("provider connection field"));
        }
    }
    if count >= PROVIDERS_MAX && !replacing {
        return Err(ConnectionError::Capacity);
    }
    if input.auth.method() != input.auth_method {
        return Err(ConnectionError::InvalidInput("provider auth method"));
    }
    Ok(())
}

fn record(mut input: ConnectProviderInput, existing: Option<&ProviderRecord>) -> ProviderRecord {
    let base_url = input
        .fields
        .remove("baseUrl")
        .or_else(|| existing.and_then(|value| value.base_url.clone()));
    ProviderRecord {
        id: existing.map_or(input.provider_id, |value| value.id.clone()),
        name: input.provider_name,
        provider_type: input.provider_type,
        auth: input.auth,
        access_key: input.fields.remove("accessKey"),
        region: input.fields.remove("region"),
        profile: input.fields.remove("profile"),
        base_url,
        timeout: existing.and_then(|value| value.timeout.clone()),
        extras: existing.map_or_else(Map::new, |value| value.extras.clone()),
        created_at: existing.map_or_else(|| input.now.clone(), |value| value.created_at.clone()),
        updated_at: input.now,
    }
}

fn validation_record<'a>(
    provider_type: &'a str,
    auth: &'a ProviderAuth,
    fields: &'a BTreeMap<String, String>,
) -> ProviderRecordRef<'a> {
    ProviderRecordRef {
        provider_type,
        auth,
        access_key: fields.get("accessKey"),
        region: fields.get("region"),
        profile: fields.get("profile"),
        base_url: fields.get("baseUrl").cloned(),
    }
}

struct ProviderRecordRef<'a> {
    provider_type: &'a str,
    auth: &'a ProviderAuth,
    access_key: Option<&'a String>,
    region: Option<&'a String>,
    profile: Option<&'a String>,
    base_url: Option<String>,
}

fn host_auth_ref(record: &ProviderRecordRef<'_>) -> Result<HostAuth, ConnectionError> {
    match record.auth {
        ProviderAuth::Api { key, .. }
            if key.expose() == "not-needed"
                && matches!(record.provider_type, "bedrock" | "amazon-bedrock")
                && record.profile.is_some() =>
        {
            Err(ConnectionError::Unsupported)
        }
        ProviderAuth::Api { key, .. } if key.expose() == "not-needed" => Ok(HostAuth::None),
        ProviderAuth::Api { key, .. }
            if matches!(record.provider_type, "bedrock" | "amazon-bedrock") =>
        {
            Ok(HostAuth::Aws {
                access_key_id: record
                    .access_key
                    .cloned()
                    .ok_or(ConnectionError::InvalidInput("AWS access key"))?,
                secret_access_key: key.expose().to_owned(),
                session_token: None,
                region: record
                    .region
                    .cloned()
                    .ok_or(ConnectionError::InvalidInput("AWS region"))?,
            })
        }
        ProviderAuth::Api { key, .. } => Ok(HostAuth::ApiKey {
            value: key.expose().to_owned(),
        }),
        ProviderAuth::OAuth { access, .. } => Ok(HostAuth::OAuthAccess {
            value: access.expose().to_owned(),
        }),
        ProviderAuth::BedrockProfile { .. } => Err(ConnectionError::Unsupported),
    }
}

fn host_auth(record: &ProviderRecord) -> Result<HostAuth, ConnectionError> {
    host_auth_ref(&ProviderRecordRef {
        provider_type: &record.provider_type,
        auth: &record.auth,
        access_key: record.access_key.as_ref(),
        region: record.region.as_ref(),
        profile: record.profile.as_ref(),
        base_url: record.base_url.clone(),
    })
}

fn provider_unavailable() -> ProviderError {
    ProviderError::Unavailable(ProviderErrorContext::new(
        ProviderName::new("connection_unavailable".into()).expect("static provider error code"),
        ProviderEventText::new("provider connection unavailable".into())
            .expect("static provider error context"),
    ))
}

impl From<lotta_runtime::RuntimeError> for ConnectionError {
    fn from(_: lotta_runtime::RuntimeError) -> Self {
        Self::Cancellation
    }
}
