use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;

/// Secret plaintext retained only by the authentication store and adapter scope.
pub struct ProviderSecret(String);

impl ProviderSecret {
    /// Creates a bounded nonempty secret.
    ///
    /// # Errors
    /// Returns [`ConnectionError::InvalidInput`] for empty or over-limit plaintext.
    pub fn new(value: String) -> Result<Self, ConnectionError> {
        if value.is_empty() || value.len() > super::PROVIDER_TEXT_BYTES_MAX {
            return Err(ConnectionError::InvalidInput("provider secret"));
        }
        Ok(Self(value))
    }

    /// Exposes the secret only to persistence and adapter callers.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderSecret([REDACTED])")
    }
}

impl Drop for ProviderSecret {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// Persisted API or OAuth authentication material.
pub enum ProviderAuth {
    /// API-key authentication.
    Api {
        /// Plaintext API key.
        key: ProviderSecret,
        /// Preserved API-auth extension fields.
        extras: Map<String, Value>,
    },
    /// OAuth credential fields, preserving baseline extension fields.
    OAuth {
        /// Access token.
        access: ProviderSecret,
        /// Optional refresh token.
        refresh: Option<ProviderSecret>,
        /// Optional identity token.
        id_token: Option<ProviderSecret>,
        /// Baseline expiry value.
        expires: u64,
        /// Optional account identity metadata.
        account_id: Option<String>,
        /// Preserved OAuth extension fields.
        extras: Map<String, Value>,
    },
    /// Baseline Bedrock profile placeholder whose persisted API key is exactly empty.
    BedrockProfile {
        /// Preserved API-auth extension fields.
        extras: Map<String, Value>,
    },
}

impl ProviderAuth {
    /// Consumes the one-use Task51 OAuth credential directly into the auth-store representation.
    #[must_use]
    pub fn from_oauth_credential(credential: crate::host::oauth::OAuthCredential) -> Self {
        let crate::host::oauth::OAuthCredential {
            access,
            refresh,
            id,
            expires_at,
        } = credential;
        Self::OAuth {
            access: ProviderSecret(access.into_inner()),
            refresh: Some(ProviderSecret(refresh.into_inner())),
            id_token: id.map(|value| ProviderSecret(value.into_inner())),
            expires: expires_at,
            account_id: None,
            extras: Map::new(),
        }
    }

    /// Returns the declarative authentication method identifier.
    #[must_use]
    pub const fn method(&self) -> AuthMethod {
        match self {
            Self::Api { .. } | Self::BedrockProfile { .. } => AuthMethod::Api,
            Self::OAuth { .. } => AuthMethod::OAuth,
        }
    }
}

impl fmt::Debug for ProviderAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderAuth([REDACTED])")
    }
}

/// Declarative provider authentication method.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    /// API key or no-key sentinel.
    Api,
    /// OAuth access and refresh credentials.
    OAuth,
}

/// One exact baseline provider record.
pub struct ProviderRecord {
    /// Stable local provider identifier.
    pub id: String,
    /// Stable provider record-map key and name.
    pub name: String,
    /// Registered provider type.
    pub provider_type: String,
    /// Secret authentication material.
    pub auth: ProviderAuth,
    /// Optional AWS access-key metadata.
    pub access_key: Option<String>,
    /// Optional region metadata.
    pub region: Option<String>,
    /// Optional profile metadata.
    pub profile: Option<String>,
    /// Optional endpoint URL.
    pub base_url: Option<String>,
    /// Optional baseline timeout object.
    pub timeout: Option<Value>,
    /// Preserved forward-compatible record fields.
    pub extras: Map<String, Value>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last-update timestamp.
    pub updated_at: String,
}

impl fmt::Debug for ProviderRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("provider_type", &self.provider_type)
            .field("auth", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// Declarative connection field description.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConnectField {
    /// Wire key.
    pub key: String,
    /// Human-readable label.
    pub label: String,
    /// Optional placeholder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Whether UI and diagnostics treat this field as secret.
    #[serde(skip_serializing_if = "is_false")]
    pub secret: bool,
    /// Whether the field must be supplied.
    pub required: bool,
}

/// Exact baseline declarative authentication method.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConnectProviderAuthMethod {
    /// Stable method identifier.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Human-readable method description.
    pub description: String,
    /// Bounded fields required by this method.
    pub fields: Vec<ConnectField>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// Validated connect request carrying one owned secret authentication value.
pub struct ConnectProviderInput {
    /// Provider registry identifier.
    pub provider_id: String,
    /// Provider record name.
    pub provider_name: String,
    /// Registered provider type.
    pub provider_type: String,
    /// Selected method.
    pub auth_method: AuthMethod,
    /// Secret authentication payload.
    pub auth: ProviderAuth,
    /// Non-secret connection fields.
    pub fields: BTreeMap<String, String>,
    /// Optimistic store revision.
    pub expected_revision: u64,
    /// Caller-injected RFC3339 timestamp.
    pub now: String,
}

impl fmt::Debug for ConnectProviderInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectProviderInput")
            .field("provider_id", &self.provider_id)
            .field("provider_name", &self.provider_name)
            .field("provider_type", &self.provider_type)
            .field("auth_method", &self.auth_method)
            .field("auth", &"[REDACTED]")
            .field("expected_revision", &self.expected_revision)
            .finish_non_exhaustive()
    }
}

/// Disconnect request with explicit force behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisconnectProviderInput {
    /// Stable provider connection identifier.
    pub provider_id: String,
    /// Optional exact record name selector.
    pub provider_name: Option<String>,
    /// Whether active turns must be cancelled before removal.
    pub force: bool,
    /// Optimistic store revision.
    pub expected_revision: u64,
}

/// Redacted connection state suitable for App Server snapshots.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConnectionSnapshot {
    /// Stable connection identifier.
    pub id: String,
    /// Provider name.
    pub provider_name: String,
    /// Provider type.
    pub provider_type: String,
    /// Authentication method without credential material.
    pub auth_type: AuthMethod,
    /// Optional endpoint URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Optional baseline timeout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Value>,
    /// Optional region.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Optional AWS access-key identifier, matching the baseline public view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key: Option<String>,
    /// Baseline connected-state discriminator.
    pub is_connected: bool,
    /// Monotonic manager revision.
    pub revision: u64,
}

/// Stable failures that never include provider credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConnectionError {
    /// Stored or submitted shape was rejected.
    #[error("provider connection input rejected: {0}")]
    InvalidInput(&'static str),
    /// Version or authentication method is unsupported.
    #[error("provider connection format unsupported")]
    Unsupported,
    /// Provider connection does not exist.
    #[error("provider connection not found")]
    NotFound,
    /// Optimistic revision changed.
    #[error("provider connection revision conflict")]
    Conflict,
    /// Provider capacity is exhausted.
    #[error("provider connection limit reached")]
    Capacity,
    /// Active turns prevent an unforced disconnect.
    #[error("provider connection has active turns")]
    ActiveTurns,
    /// Adapter validation failed without reflecting its details.
    #[error("provider connection validation failed")]
    Adapter,
    /// Turn cancellation did not settle.
    #[error("provider connection cancellation failed")]
    Cancellation,
    /// Persistence operation failed without path or payload reflection.
    #[error("provider connection persistence failed")]
    Store,
}

pub(crate) struct AuthFile {
    pub providers: BTreeMap<String, ProviderRecord>,
}

#[derive(Deserialize)]
pub(crate) struct RawFile {
    pub version: u8,
    pub providers: BTreeMap<String, RawRecord>,
    #[serde(flatten)]
    pub extras: Map<String, Value>,
}

#[derive(Deserialize)]
pub(crate) struct RawRecord {
    pub id: String,
    pub name: String,
    pub provider_type: String,
    pub provider_category: String,
    pub auth: Value,
    pub access_key: Option<String>,
    pub region: Option<String>,
    pub profile: Option<String>,
    pub base_url: Option<String>,
    pub timeout: Option<Value>,
    #[serde(flatten)]
    pub extras: Map<String, Value>,
    pub created_at: String,
    pub updated_at: String,
}
