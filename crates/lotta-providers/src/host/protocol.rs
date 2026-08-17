//! Bounded provider-host payload protocol carried by Task 42 envelopes.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

use super::oauth::{OAuthBegin, OAuthDeviceBegin, OAuthMetadata};

/// Provider-host payload protocol version.
pub const HOST_PROTOCOL_VERSION: u16 = 1;
/// Maximum concurrent host requests.
pub const HOST_PENDING_REQUESTS_MAX: usize = 64;
/// Maximum provider descriptors in one catalog.
pub const HOST_PROVIDERS_MAX: usize = 128;
/// Maximum models in one complete catalog.
pub const HOST_MODELS_MAX: usize = 10_000;
/// Maximum bounded descriptor string bytes.
pub const HOST_TEXT_BYTES_MAX: usize = 512;
/// Maximum explicit credential bytes retained by one host provider.
pub const HOST_CREDENTIAL_BYTES_MAX: usize = 64 * 1024;
/// Maximum normalized events accepted for one stream.
pub const HOST_STREAM_EVENTS_MAX: usize = 4_096;
/// Maximum bytes accepted in one encoded stream event.
pub const HOST_EVENT_BYTES_MAX: usize = 4 * 1024 * 1024;
/// Maximum fixture raw stream bytes accepted in test mode.
pub const HOST_FIXTURE_BYTES_MAX: usize = 8 * 1024 * 1024;

/// Explicit, scoped host authentication. Values are deliberately not `Debug`.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostAuth {
    /// No credential; used by explicitly configured keyless local providers.
    None,
    /// Provider API key or bearer credential.
    ApiKey {
        /// Secret credential value.
        value: String,
    },
    /// Already refreshed OAuth access credential. OAuth flows themselves are separate scope.
    OAuthAccess {
        /// Secret bearer access credential.
        value: String,
    },
    /// AWS credentials supplied only to the selected Bedrock request.
    Aws {
        /// Secret AWS access-key identifier.
        access_key_id: String,
        /// Secret AWS signing key.
        secret_access_key: String,
        /// Optional secret session credential.
        session_token: Option<String>,
        /// Explicit AWS region.
        region: String,
    },
    /// Google ADC JSON supplied only to the selected Vertex request.
    GoogleCredentials {
        /// Secret Google credential document.
        json: Value,
    },
}

/// Explicit request options; no ambient environment or proxy values are consulted.
#[derive(Clone, Default, Serialize)]
pub struct HostOptions {
    /// Optional explicit provider base URL.
    pub base_url: Option<String>,
    /// Explicit provider-specific non-secret environment values.
    pub env: Value,
    /// Explicit non-secret request headers.
    pub headers: Value,
    /// Provider-specific options merged after safe common options.
    pub provider: Value,
}

/// Test-only deterministic raw stream injection through pi-ai's translation boundary.
#[derive(Clone, Serialize)]
pub struct HostFixtureStream {
    /// Corpus dialect.
    pub dialect: String,
    /// Captured response status.
    pub status: u16,
    /// Captured response headers.
    pub headers: Value,
    /// Captured raw bytes encoded as base64.
    pub bytes_base64: String,
    /// Expected mapped vendor request, asserted by the child before translating.
    pub expected_request: Value,
}

/// Complete bounded parameters for one inference attempt.
#[derive(Clone, Serialize)]
pub struct HostInferenceStart {
    /// Normalized provider request wire object.
    pub request: Value,
    /// Explicit scoped authentication.
    pub auth: HostAuth,
    /// Explicit provider options.
    pub options: HostOptions,
    /// Optional test-only fixture injection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixture: Option<HostFixtureStream>,
}

/// One correlated streamed response from the host.
#[derive(Clone, Debug, Deserialize)]
pub struct HostStreamPayload {
    /// Stream identifier from `inference.start`.
    pub stream_id: String,
    /// Monotonic stream sequence number.
    pub sequence: u64,
    /// Normalized event JSON.
    pub event: Value,
}

/// Supported provider OAuth host commands.
#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostOAuthCommand {
    /// Read validated provider metadata.
    Metadata,
    /// Begin PKCE authorization-code login.
    Begin,
    /// Exchange a callback authorization code.
    Exchange,
    /// Begin device-code login.
    DeviceBegin,
    /// Poll a device-code login.
    DevicePoll,
    /// Cancel either flow.
    Cancel,
}

impl HostOAuthCommand {
    /// Returns the stable host command name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Metadata => "oauth.metadata",
            Self::Begin => "oauth.begin",
            Self::Exchange => "oauth.exchange",
            Self::DeviceBegin => "oauth.device.begin",
            Self::DevicePoll => "oauth.device.poll",
            Self::Cancel => "oauth.cancel",
        }
    }
}

impl fmt::Debug for HostOAuthCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Non-secret OAuth host response.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostOAuthPublicResponse {
    /// Provider metadata.
    Metadata(OAuthMetadata),
    /// Browser flow began.
    Begin(OAuthBegin),
    /// Device flow began.
    DeviceBegin(OAuthDeviceBegin),
    /// Flow remains pending.
    DevicePending {
        /// Earliest next monotonic polling instant.
        next_poll_at: u64,
    },
    /// Flow was cancelled.
    Cancelled,
}

/// A provider-host command payload.
#[derive(Clone, Serialize)]
pub struct HostRequest {
    /// Stable command name.
    pub command: String,
    /// Command-specific bounded parameters.
    pub params: Value,
}

impl fmt::Debug for HostRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostRequest")
            .field("command", &self.command)
            .field("params", &"[REDACTED]")
            .finish()
    }
}

/// A provider-host response payload.
#[derive(Clone, Debug, Deserialize)]
pub struct HostResponse {
    /// Successful command result.
    pub ok: Option<Value>,
    /// Stable rejected-command error.
    pub error: Option<HostResponseError>,
}

/// Rejected provider-host command.
#[derive(Clone, Debug, Deserialize)]
pub struct HostResponseError {
    /// Stable machine code.
    pub code: String,
    /// Non-secret diagnostic.
    pub message: String,
}

/// Validated startup declaration from the real host.
#[derive(Clone, Debug, Deserialize)]
pub struct HostHello {
    /// Actual package-manifest version resolved by the script.
    pub pi_ai_version: String,
    /// Payload protocol version.
    pub protocol_version: u16,
    /// Host challenge echoed exactly.
    pub nonce: String,
    /// Exact granted host capabilities.
    pub capabilities: Vec<String>,
    /// Whether explicit test fixture injection was enabled at launch.
    #[serde(default)]
    pub test_mode: bool,
}
