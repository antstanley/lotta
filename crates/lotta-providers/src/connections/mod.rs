//! Baseline provider connection persistence, public catalogs, and hosted streaming.

mod connect;
mod disconnect;
mod store;
mod types;

pub use connect::{
    ConnectionAdapter, ConnectionAdapterFactory, ConnectionAdapterFuture, ConnectionManager,
    HostConnectionAdapterFactory,
};
pub use disconnect::{ActiveTurnRegistry, TurnCancellation};
pub use store::ProviderAuthStore;
pub use types::{
    AuthMethod, ConnectField, ConnectProviderAuthMethod, ConnectProviderInput, ConnectionError,
    ConnectionSnapshot, DisconnectProviderInput, ProviderAuth, ProviderRecord, ProviderSecret,
};

/// Maximum persisted provider connections from the provider specification.
pub const PROVIDERS_MAX: usize = 128;
/// Maximum serialized provider authentication file size.
pub const PROVIDER_AUTH_BYTES_MAX: usize = 8 * 1_024 * 1_024;
/// Maximum provider identifier, name, method, and metadata string bytes.
pub const PROVIDER_TEXT_BYTES_MAX: usize = 4_096;
/// Maximum fields in one declarative provider connection request.
pub const PROVIDER_FIELDS_MAX: usize = 64;

/// Returns the exact default API-key field catalog.
#[must_use]
pub fn default_api_key_fields() -> Vec<ConnectField> {
    vec![field("apiKey", "API Key", None, true, true)]
}

/// Returns the exact local OpenAI-compatible field catalog.
#[must_use]
pub fn openai_compatible_fields() -> Vec<ConnectField> {
    vec![
        field("apiKey", "API Key", None, true, false),
        field("baseUrl", "Base URL", None, false, true),
    ]
}

/// Returns the exact local Ollama field catalog.
#[must_use]
pub fn ollama_fields() -> Vec<ConnectField> {
    local_fields("http://localhost:11434/v1")
}

/// Returns the exact local LM Studio field catalog.
#[must_use]
pub fn lmstudio_fields() -> Vec<ConnectField> {
    local_fields("http://127.0.0.1:1234/v1")
}

/// Returns the exact local llama.cpp field catalog.
#[must_use]
pub fn llama_cpp_fields() -> Vec<ConnectField> {
    local_fields("http://localhost:8080/v1")
}

/// Returns the pinned Google Vertex API-key field catalog.
#[must_use]
pub fn google_vertex_fields() -> Vec<ConnectField> {
    default_api_key_fields()
}

/// Returns the exact baseline AWS Bedrock authentication method catalog.
#[must_use]
pub fn baseline_bedrock_auth_methods() -> Vec<ConnectProviderAuthMethod> {
    vec![
        ConnectProviderAuthMethod {
            id: "iam".into(),
            label: "AWS Access Keys".into(),
            description: "Enter access key and secret key manually".into(),
            fields: vec![
                field(
                    "accessKey",
                    "AWS Access Key ID",
                    Some("AKIA..."),
                    false,
                    true,
                ),
                field("apiKey", "AWS Secret Access Key", None, true, true),
                field("region", "AWS Region", Some("us-east-1"), false, true),
            ],
        },
        ConnectProviderAuthMethod {
            id: "profile".into(),
            label: "AWS Profile".into(),
            description: "Load credentials from ~/.aws/credentials".into(),
            fields: vec![
                field("profile", "Profile Name", Some("default"), false, true),
                field("region", "AWS Region", Some("us-east-1"), false, true),
            ],
        },
    ]
}

fn local_fields(base_url: &str) -> Vec<ConnectField> {
    vec![
        field("baseUrl", "Base URL", Some(base_url), false, false),
        field("apiKey", "API Key", None, true, false),
    ]
}

fn field(
    key: &str,
    label: &str,
    placeholder: Option<&str>,
    secret: bool,
    required: bool,
) -> ConnectField {
    ConnectField {
        key: key.into(),
        label: label.into(),
        placeholder: placeholder.map(str::to_owned),
        secret,
        required,
    }
}

#[cfg(test)]
mod tests;
