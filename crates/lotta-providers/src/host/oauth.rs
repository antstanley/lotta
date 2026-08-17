//! Bounded `OpenAI` Codex OAuth manager.
#![allow(clippy::missing_errors_doc)]

use base64::Engine as _;
use serde::ser::{Serialize, SerializeStruct, Serializer};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq as _;
use url::Url;

/// Canonical OAuth state lifetime from spec 06.
pub const OAUTH_STATE_TTL_SECONDS: u64 = 600;
/// Maximum concurrent pending OAuth flows.
pub const OAUTH_PENDING_FLOWS_MAX: usize = 64;
/// Maximum OAuth protocol string bytes.
pub const OAUTH_TEXT_BYTES_MAX: usize = 4 * 1024;
/// Maximum authorization URL bytes.
pub const OAUTH_URL_BYTES_MAX: usize = 8 * 1024;
/// Maximum scope count.
pub const OAUTH_SCOPES_MAX: usize = 16;
/// Maximum OAuth HTTP response body bytes.
pub const OAUTH_HTTP_RESPONSE_BYTES_MAX: usize = 64 * 1024;
/// Maximum OAuth HTTP operation duration.
pub const OAUTH_HTTP_TIMEOUT_SECONDS: u64 = 30;
/// `OpenAI` Codex device-flow lifetime from pinned pi-ai.
pub const OAUTH_DEVICE_TTL_SECONDS: u64 = 15 * 60;
/// RFC 8628 default polling interval.
pub const OAUTH_DEVICE_INTERVAL_SECONDS_DEFAULT: u64 = 5;
/// RFC 8628 minimum polling interval.
pub const OAUTH_DEVICE_INTERVAL_SECONDS_MIN: u64 = 1;
/// RFC 8628 slow-down increment.
pub const OAUTH_DEVICE_SLOW_DOWN_SECONDS: u64 = 5;

const STATE_BYTES: usize = 32;
const VERIFIER_BYTES: usize = 32;
const DEVICE_ID_BYTES: usize = 32;
const OPENAI_PROVIDER: &str = "openai-codex";
const OPENAI_ISSUER: &str = "https://auth.openai.com";
const OPENAI_AUTHORIZE: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN: &str = "https://auth.openai.com/oauth/token";
const OPENAI_DEVICE: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const OPENAI_DEVICE_TOKEN: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OPENAI_DEVICE_VERIFY: &str = "https://auth.openai.com/codex/device";
const OPENAI_DEVICE_REDIRECT: &str = "https://auth.openai.com/deviceauth/callback";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_REDIRECT: &str = "http://localhost:1455/auth/callback";
const OPENAI_SCOPE: &str = "openid profile email offline_access";

/// Secret authorization code.
pub struct AuthorizationCode(String);
/// Secret PKCE verifier.
pub struct CodeVerifier(String);
/// Secret access token.
pub struct AccessToken(String);
/// Secret refresh token.
pub struct RefreshToken(String);
/// Secret identity token.
pub struct IdToken(String);

macro_rules! secret_type {
    ($name:ident) => {
        impl $name {
            fn new(value: String) -> Result<Self, OAuthError> {
                validate_secret(&value)?;
                Ok(Self(value))
            }
            fn expose(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
        impl Drop for $name {
            fn drop(&mut self) {
                self.0.clear();
            }
        }
    };
}
secret_type!(AuthorizationCode);
secret_type!(CodeVerifier);
secret_type!(AccessToken);
secret_type!(RefreshToken);
secret_type!(IdToken);

/// Scoped OAuth credentials delivered once to the caller-owned auth-store seam.
pub struct OAuthCredential {
    /// Access token.
    pub access: AccessToken,
    /// Refresh token.
    pub refresh: RefreshToken,
    /// Optional identity token.
    pub id: Option<IdToken>,
    /// Monotonic expiry instant in seconds.
    pub expires_at: u64,
}

impl fmt::Debug for OAuthCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthCredential([REDACTED])")
    }
}

/// One-command wire envelope for scoped secret delivery.
pub struct OAuthCredentialEnvelope<'a>(pub &'a OAuthCredential);

impl fmt::Debug for OAuthCredentialEnvelope<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthCredentialEnvelope([REDACTED])")
    }
}

impl Serialize for OAuthCredentialEnvelope<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("OAuthCredential", 5)?;
        state.serialize_field("type", "oauth")?;
        state.serialize_field("access", self.0.access.expose())?;
        state.serialize_field("refresh", self.0.refresh.expose())?;
        if let Some(id) = &self.0.id {
            state.serialize_field("id", id.expose())?;
        }
        state.serialize_field("expires", &self.0.expires_at)?;
        state.end()
    }
}

/// Stable, deliberately non-sensitive OAuth failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OAuthError {
    /// Configuration or input is not allowlisted and bounded.
    #[error("oauth input rejected")]
    InvalidInput,
    /// Pending flow capacity is exhausted.
    #[error("oauth pending-flow limit reached")]
    Capacity,
    /// Flow was not found or already consumed.
    #[error("oauth flow unavailable")]
    Unavailable,
    /// Callback state did not match in constant time.
    #[error("oauth callback rejected")]
    StateMismatch,
    /// Flow reached its exact monotonic expiry.
    #[error("oauth flow expired")]
    Expired,
    /// User or authorization server denied the flow.
    #[error("oauth authorization denied")]
    Denied,
    /// Flow was explicitly cancelled.
    #[error("oauth flow cancelled")]
    Cancelled,
    /// OAuth transport or response was rejected without reflecting its body.
    #[error("oauth provider rejected response")]
    Provider,
    /// Poll happened before the current interval elapsed.
    #[error("oauth poll interval not elapsed")]
    TooSoon,
}

/// Monotonic clock seam.
pub trait OAuthClock: Send + Sync {
    /// Seconds from an arbitrary non-decreasing origin.
    fn now_seconds(&self) -> u64;
}

/// Cryptographic random source seam.
pub trait OAuthRandom: Send + Sync {
    /// Fills bytes from a cryptographically secure source.
    fn fill(&self, bytes: &mut [u8]) -> Result<(), OAuthError>;
}

/// Explicit browser-opening seam. Production callers decide whether to open.
pub trait OAuthBrowser: Send + Sync {
    /// Opens an already validated authorization URL.
    fn open(&self, url: &Url) -> Result<(), OAuthError>;
}

/// Explicit bounded HTTP seam.
pub trait OAuthHttp: Send + Sync {
    /// Posts form fields and returns a bounded typed response.
    fn post_form(
        &self,
        url: &Url,
        fields: &[(&str, &str)],
    ) -> Result<OAuthHttpResponse, OAuthError>;
    /// Posts JSON fields and returns a bounded typed response.
    fn post_json(
        &self,
        url: &Url,
        fields: &[(&str, &str)],
    ) -> Result<OAuthHttpResponse, OAuthError>;
}

/// Bounded, scrubbed OAuth HTTP response.
pub struct OAuthHttpResponse {
    /// HTTP status.
    pub status: u16,
    /// Parsed response fields. Callers must not log or format this value.
    pub fields: HashMap<String, String>,
}

impl fmt::Debug for OAuthHttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OAuthHttpResponse {{ status: {}, fields: [REDACTED] }}",
            self.status
        )
    }
}

/// Authoritative pinned `OpenAI` OAuth metadata.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OAuthMetadata {
    /// Provider identifier.
    pub provider: String,
    /// Issuer origin.
    pub issuer: String,
    /// Authorization endpoint.
    pub authorization_endpoint: String,
    /// Token endpoint.
    pub token_endpoint: String,
    /// Device authorization endpoint.
    pub device_authorization_endpoint: String,
    /// Device polling endpoint.
    pub device_token_endpoint: String,
    /// Device verification URI.
    pub device_verification_uri: String,
    /// Public client identifier.
    pub client_id: String,
    /// Optional audience; `OpenAI`'s pinned flow omits one.
    pub audience: Option<String>,
    /// Exact scopes.
    pub scopes: Vec<String>,
    /// Exact callback allowlist.
    pub redirect_uris: Vec<String>,
}

impl OAuthMetadata {
    /// Returns pinned `OpenAI` Codex metadata.
    #[must_use]
    pub fn openai_codex() -> Self {
        Self {
            provider: OPENAI_PROVIDER.into(),
            issuer: OPENAI_ISSUER.into(),
            authorization_endpoint: OPENAI_AUTHORIZE.into(),
            token_endpoint: OPENAI_TOKEN.into(),
            device_authorization_endpoint: OPENAI_DEVICE.into(),
            device_token_endpoint: OPENAI_DEVICE_TOKEN.into(),
            device_verification_uri: OPENAI_DEVICE_VERIFY.into(),
            client_id: OPENAI_CLIENT_ID.into(),
            audience: None,
            scopes: OPENAI_SCOPE.split(' ').map(str::to_owned).collect(),
            redirect_uris: vec![OPENAI_REDIRECT.into(), OPENAI_DEVICE_REDIRECT.into()],
        }
    }

    fn validate(&self) -> Result<(), OAuthError> {
        if self != &Self::openai_codex() || self.scopes.len() > OAUTH_SCOPES_MAX {
            return Err(OAuthError::InvalidInput);
        }
        for endpoint in [
            &self.issuer,
            &self.authorization_endpoint,
            &self.token_endpoint,
            &self.device_authorization_endpoint,
            &self.device_token_endpoint,
            &self.device_verification_uri,
        ] {
            validate_https(endpoint)?;
        }
        validate_redirect(OPENAI_REDIRECT)?;
        Ok(())
    }
}

/// Browser authorization start response.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OAuthBegin {
    /// Opaque flow identifier.
    pub flow_id: String,
    /// Validated authorization URL.
    pub authorization_url: String,
    /// Exact expiry in monotonic seconds.
    pub expires_at: u64,
}

/// Device authorization start response.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OAuthDeviceBegin {
    /// Opaque flow identifier.
    pub flow_id: String,
    /// Non-secret user code.
    pub user_code: String,
    /// Validated verification URI.
    pub verification_uri: String,
    /// Current poll interval.
    pub interval_seconds: u64,
    /// Exact expiry in monotonic seconds.
    pub expires_at: u64,
}

/// Callback query with duplicate-preserving pairs.
#[derive(Clone, Copy)]
pub struct OAuthCallback<'a> {
    /// Exact callback origin, including scheme and port.
    pub origin: &'a str,
    /// Exact registered redirect URI.
    pub redirect_uri: &'a str,
    /// Query pairs.
    pub query: &'a [(&'a str, &'a str)],
}

impl fmt::Debug for OAuthCallback<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthCallback([REDACTED])")
    }
}

/// Device polling outcome.
pub enum OAuthDevicePoll {
    /// Authorization is still pending.
    Pending {
        /// Earliest permitted next poll in monotonic seconds.
        next_poll_at: u64,
    },
    /// Authorization completed and credentials are delivered exactly once.
    Complete(OAuthCredential),
}

impl fmt::Debug for OAuthDevicePoll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending { next_poll_at } => f
                .debug_struct("Pending")
                .field("next_poll_at", next_poll_at)
                .finish(),
            Self::Complete(_) => f.write_str("Complete([REDACTED])"),
        }
    }
}

struct PendingBrowser {
    provider: String,
    session: String,
    owner: String,
    redirect: String,
    client: String,
    state: String,
    verifier: CodeVerifier,
    expires_at: u64,
}

struct PendingDevice {
    provider: String,
    session: String,
    owner: String,
    client: String,
    device_auth_id: String,
    user_code: String,
    interval: u64,
    next_poll_at: u64,
    expires_at: u64,
}

enum Pending {
    Browser(PendingBrowser),
    Device(PendingDevice),
}

/// Production bounded OAuth flow manager.
pub struct OAuthManager {
    metadata: OAuthMetadata,
    clock: Arc<dyn OAuthClock>,
    random: Arc<dyn OAuthRandom>,
    http: Arc<dyn OAuthHttp>,
    browser: Arc<dyn OAuthBrowser>,
    pending: Mutex<HashMap<String, Pending>>,
}

impl fmt::Debug for OAuthManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthManager([REDACTED])")
    }
}

impl OAuthManager {
    /// Constructs a production manager with monotonic time, OS randomness, bounded TLS HTTP,
    /// and the caller-owned browser port.
    pub fn production(browser: Arc<dyn OAuthBrowser>) -> Result<Self, OAuthError> {
        Self::new(
            Arc::new(MonotonicOAuthClock::default()),
            Arc::new(OsOAuthRandom),
            Arc::new(ReqwestOAuthHttp::new()?),
            browser,
        )
    }

    /// Constructs a manager only from explicit side-effect ports.
    pub fn new(
        clock: Arc<dyn OAuthClock>,
        random: Arc<dyn OAuthRandom>,
        http: Arc<dyn OAuthHttp>,
        browser: Arc<dyn OAuthBrowser>,
    ) -> Result<Self, OAuthError> {
        let metadata = OAuthMetadata::openai_codex();
        metadata.validate()?;
        Ok(Self {
            metadata,
            clock,
            random,
            http,
            browser,
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// Returns validated provider metadata without secrets.
    #[must_use]
    pub fn metadata(&self) -> OAuthMetadata {
        self.metadata.clone()
    }

    /// Atomically starts a PKCE authorization-code flow.
    pub fn begin(
        &self,
        provider: &str,
        session: &str,
        owner: &str,
        redirect: &str,
        open_browser: bool,
    ) -> Result<OAuthBegin, OAuthError> {
        validate_binding(provider, session, owner)?;
        if provider != self.metadata.provider || redirect != OPENAI_REDIRECT {
            return Err(OAuthError::InvalidInput);
        }
        validate_redirect(redirect)?;
        let state = self.random_hex(STATE_BYTES)?;
        let flow_id = self.random_hex(DEVICE_ID_BYTES)?;
        let verifier = CodeVerifier::new(self.random_base64(VERIFIER_BYTES)?)?;
        let challenge = pkce_challenge(verifier.expose());
        let now = self.clock.now_seconds();
        let expires_at = now
            .checked_add(OAUTH_STATE_TTL_SECONDS)
            .ok_or(OAuthError::InvalidInput)?;
        let url = authorization_url(&state, &challenge, redirect)?;
        {
            let mut pending = self.pending.lock().map_err(|_| OAuthError::Unavailable)?;
            prune(&mut pending, now);
            if pending.len() >= OAUTH_PENDING_FLOWS_MAX {
                return Err(OAuthError::Capacity);
            }
            pending.insert(
                flow_id.clone(),
                Pending::Browser(PendingBrowser {
                    provider: provider.into(),
                    session: session.into(),
                    owner: owner.into(),
                    redirect: redirect.into(),
                    client: OPENAI_CLIENT_ID.into(),
                    state,
                    verifier,
                    expires_at,
                }),
            );
        }
        if open_browser && let Err(error) = self.browser.open(&url) {
            let _ = self.cancel(&flow_id, provider, session, owner);
            return Err(error);
        }
        Ok(OAuthBegin {
            flow_id,
            authorization_url: url.into(),
            expires_at,
        })
    }

    /// Consumes a callback once, exchanges its code, and returns scoped credentials.
    pub fn callback(
        &self,
        flow_id: &str,
        provider: &str,
        session: &str,
        owner: &str,
        callback: OAuthCallback<'_>,
    ) -> Result<OAuthCredential, OAuthError> {
        validate_binding(provider, session, owner)?;
        validate_callback_location(callback.origin, callback.redirect_uri)?;
        let parsed = parse_callback(callback.query)?;
        let now = self.clock.now_seconds();
        let pending = self.take_browser(flow_id)?;
        if now >= pending.expires_at {
            return Err(OAuthError::Expired);
        }
        if pending.provider != provider
            || pending.session != session
            || pending.owner != owner
            || pending.redirect != callback.redirect_uri
            || pending.client != OPENAI_CLIENT_ID
        {
            return Err(OAuthError::Unavailable);
        }
        if !constant_time_equal(parsed.state, &pending.state) {
            return Err(OAuthError::StateMismatch);
        }
        if parsed.error.is_some() {
            return Err(OAuthError::Denied);
        }
        let code = AuthorizationCode::new(parsed.code.ok_or(OAuthError::InvalidInput)?.to_owned())?;
        let response = self.http.post_form(
            &parse_endpoint(OPENAI_TOKEN)?,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", OPENAI_CLIENT_ID),
                ("code", code.expose()),
                ("code_verifier", pending.verifier.expose()),
                ("redirect_uri", callback.redirect_uri),
            ],
        )?;
        credential(response, now)
    }

    /// Starts the pinned `OpenAI` device-code flow.
    pub fn begin_device(
        &self,
        provider: &str,
        session: &str,
        owner: &str,
    ) -> Result<OAuthDeviceBegin, OAuthError> {
        validate_binding(provider, session, owner)?;
        if provider != OPENAI_PROVIDER {
            return Err(OAuthError::InvalidInput);
        }
        let response = self.http.post_json(
            &parse_endpoint(OPENAI_DEVICE)?,
            &[("client_id", OPENAI_CLIENT_ID)],
        )?;
        if !(200..300).contains(&response.status) {
            return Err(OAuthError::Provider);
        }
        let device_auth_id = response_field(&response, "device_auth_id")?.to_owned();
        let user_code = response_field(&response, "user_code")?.to_owned();
        let interval = response
            .fields
            .get("interval")
            .map_or(Ok(OAUTH_DEVICE_INTERVAL_SECONDS_DEFAULT), |value| {
                value.parse::<u64>().map_err(|_| OAuthError::Provider)
            })?
            .max(OAUTH_DEVICE_INTERVAL_SECONDS_MIN);
        validate_secret(&device_auth_id)?;
        validate_text(&user_code)?;
        let flow_id = self.random_hex(DEVICE_ID_BYTES)?;
        let now = self.clock.now_seconds();
        let expires_at = now
            .checked_add(OAUTH_DEVICE_TTL_SECONDS)
            .ok_or(OAuthError::InvalidInput)?;
        let mut pending = self.pending.lock().map_err(|_| OAuthError::Unavailable)?;
        prune(&mut pending, now);
        if pending.len() >= OAUTH_PENDING_FLOWS_MAX {
            return Err(OAuthError::Capacity);
        }
        pending.insert(
            flow_id.clone(),
            Pending::Device(PendingDevice {
                provider: provider.into(),
                session: session.into(),
                owner: owner.into(),
                client: OPENAI_CLIENT_ID.into(),
                device_auth_id,
                user_code: user_code.clone(),
                interval,
                next_poll_at: now,
                expires_at,
            }),
        );
        Ok(OAuthDeviceBegin {
            flow_id,
            user_code,
            verification_uri: OPENAI_DEVICE_VERIFY.into(),
            interval_seconds: interval,
            expires_at,
        })
    }

    /// Polls once, enforcing interval, slow-down, cancellation, and deadline semantics.
    pub fn poll_device(
        &self,
        flow_id: &str,
        provider: &str,
        session: &str,
        owner: &str,
    ) -> Result<OAuthDevicePoll, OAuthError> {
        validate_binding(provider, session, owner)?;
        let now = self.clock.now_seconds();
        let fields = {
            let mut pending = self.pending.lock().map_err(|_| OAuthError::Unavailable)?;
            let entry = pending.get_mut(flow_id).ok_or(OAuthError::Unavailable)?;
            let Pending::Device(device) = entry else {
                return Err(OAuthError::Unavailable);
            };
            if now >= device.expires_at {
                pending.remove(flow_id);
                return Err(OAuthError::Expired);
            }
            if device.provider != provider
                || device.session != session
                || device.owner != owner
                || device.client != OPENAI_CLIENT_ID
            {
                return Err(OAuthError::Unavailable);
            }
            if now < device.next_poll_at {
                return Err(OAuthError::TooSoon);
            }
            device.next_poll_at = now.saturating_add(device.interval);
            (device.device_auth_id.clone(), device.user_code.clone())
        };
        let response = self.http.post_json(
            &parse_endpoint(OPENAI_DEVICE_TOKEN)?,
            &[("device_auth_id", &fields.0), ("user_code", &fields.1)],
        )?;
        self.handle_device_response(flow_id, response, now)
    }

    /// Cancels a flow only for its exact binding.
    pub fn cancel(
        &self,
        flow_id: &str,
        provider: &str,
        session: &str,
        owner: &str,
    ) -> Result<(), OAuthError> {
        let mut pending = self.pending.lock().map_err(|_| OAuthError::Unavailable)?;
        let value = pending.get(flow_id).ok_or(OAuthError::Unavailable)?;
        if !bound(value, provider, session, owner) {
            return Err(OAuthError::Unavailable);
        }
        pending.remove(flow_id);
        Ok(())
    }

    /// Clears all in-memory state when the owning host dies.
    pub fn host_died(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
    }

    fn take_browser(&self, flow_id: &str) -> Result<PendingBrowser, OAuthError> {
        let mut pending = self.pending.lock().map_err(|_| OAuthError::Unavailable)?;
        match pending.remove(flow_id) {
            Some(Pending::Browser(value)) => Ok(value),
            Some(Pending::Device(value)) => {
                pending.insert(flow_id.into(), Pending::Device(value));
                Err(OAuthError::Unavailable)
            }
            None => Err(OAuthError::Unavailable),
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn handle_device_response(
        &self,
        flow_id: &str,
        response: OAuthHttpResponse,
        now: u64,
    ) -> Result<OAuthDevicePoll, OAuthError> {
        if (200..300).contains(&response.status) {
            let code = AuthorizationCode::new(
                response_field(&response, "authorization_code")?.to_owned(),
            )?;
            let verifier =
                CodeVerifier::new(response_field(&response, "code_verifier")?.to_owned())?;
            self.remove_device(flow_id)?;
            let exchanged = self.http.post_form(
                &parse_endpoint(OPENAI_TOKEN)?,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", OPENAI_CLIENT_ID),
                    ("code", code.expose()),
                    ("code_verifier", verifier.expose()),
                    ("redirect_uri", OPENAI_DEVICE_REDIRECT),
                ],
            )?;
            return credential(exchanged, now).map(OAuthDevicePoll::Complete);
        }
        let code = response.fields.get("error").map_or("", String::as_str);
        match code {
            "deviceauth_authorization_pending" | "authorization_pending" => {
                self.pending_result(flow_id)
            }
            "slow_down" => {
                self.slow_down(flow_id, response.fields.get("interval"))?;
                self.pending_result(flow_id)
            }
            "access_denied" | "authorization_declined" => {
                self.remove_device(flow_id)?;
                Err(OAuthError::Denied)
            }
            "expired_token" => {
                self.remove_device(flow_id)?;
                Err(OAuthError::Expired)
            }
            _ if response.status == 403 || response.status == 404 => self.pending_result(flow_id),
            _ => {
                self.remove_device(flow_id)?;
                Err(OAuthError::Provider)
            }
        }
    }

    fn pending_result(&self, flow_id: &str) -> Result<OAuthDevicePoll, OAuthError> {
        let pending = self.pending.lock().map_err(|_| OAuthError::Unavailable)?;
        let Some(Pending::Device(value)) = pending.get(flow_id) else {
            return Err(OAuthError::Unavailable);
        };
        Ok(OAuthDevicePoll::Pending {
            next_poll_at: value.next_poll_at,
        })
    }

    fn slow_down(&self, flow_id: &str, server: Option<&String>) -> Result<(), OAuthError> {
        let mut pending = self.pending.lock().map_err(|_| OAuthError::Unavailable)?;
        let Some(Pending::Device(value)) = pending.get_mut(flow_id) else {
            return Err(OAuthError::Unavailable);
        };
        value.interval = server
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or_else(|| {
                value
                    .interval
                    .saturating_add(OAUTH_DEVICE_SLOW_DOWN_SECONDS)
            })
            .max(OAUTH_DEVICE_INTERVAL_SECONDS_MIN);
        value.next_poll_at = self.clock.now_seconds().saturating_add(value.interval);
        Ok(())
    }

    fn remove_device(&self, flow_id: &str) -> Result<(), OAuthError> {
        self.pending
            .lock()
            .map_err(|_| OAuthError::Unavailable)?
            .remove(flow_id)
            .map(|_| ())
            .ok_or(OAuthError::Unavailable)
    }

    fn random_hex(&self, bytes: usize) -> Result<String, OAuthError> {
        let mut value = vec![0; bytes];
        self.random.fill(&mut value)?;
        Ok(value
            .iter()
            .fold(String::with_capacity(bytes * 2), |mut output, byte| {
                let _ = write!(output, "{byte:02x}");
                output
            }))
    }

    fn random_base64(&self, bytes: usize) -> Result<String, OAuthError> {
        let mut value = vec![0; bytes];
        self.random.fill(&mut value)?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value))
    }
}

struct ParsedCallback<'a> {
    code: Option<&'a str>,
    state: &'a str,
    error: Option<&'a str>,
}

fn parse_callback<'a>(query: &'a [(&'a str, &'a str)]) -> Result<ParsedCallback<'a>, OAuthError> {
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (name, value) in query {
        let slot = match *name {
            "code" => &mut code,
            "state" => &mut state,
            "error" => &mut error,
            _ => continue,
        };
        if slot.replace(*value).is_some() {
            return Err(OAuthError::InvalidInput);
        }
    }
    let state = state.ok_or(OAuthError::InvalidInput)?;
    if state.is_empty() || code.is_some() == error.is_some() {
        return Err(OAuthError::InvalidInput);
    }
    if code.is_some_and(str::is_empty) || error.is_some_and(str::is_empty) {
        return Err(OAuthError::InvalidInput);
    }
    Ok(ParsedCallback { code, state, error })
}

#[allow(clippy::needless_pass_by_value)]
fn credential(response: OAuthHttpResponse, now: u64) -> Result<OAuthCredential, OAuthError> {
    if !(200..300).contains(&response.status) {
        return Err(OAuthError::Provider);
    }
    let access = AccessToken::new(response_field(&response, "access_token")?.to_owned())?;
    let refresh = RefreshToken::new(response_field(&response, "refresh_token")?.to_owned())?;
    let id = response
        .fields
        .get("id_token")
        .map(|value| IdToken::new(value.clone()))
        .transpose()?;
    let expires = response_field(&response, "expires_in")?
        .parse::<u64>()
        .map_err(|_| OAuthError::Provider)?;
    Ok(OAuthCredential {
        access,
        refresh,
        id,
        expires_at: now.checked_add(expires).ok_or(OAuthError::Provider)?,
    })
}

fn response_field<'a>(response: &'a OAuthHttpResponse, name: &str) -> Result<&'a str, OAuthError> {
    let value = response.fields.get(name).ok_or(OAuthError::Provider)?;
    validate_secret(value)?;
    Ok(value)
}

fn authorization_url(state: &str, challenge: &str, redirect: &str) -> Result<Url, OAuthError> {
    let mut url = parse_endpoint(OPENAI_AUTHORIZE)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", OPENAI_CLIENT_ID)
        .append_pair("redirect_uri", redirect)
        .append_pair("scope", OPENAI_SCOPE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", "pi");
    if url.as_str().len() > OAUTH_URL_BYTES_MAX {
        return Err(OAuthError::InvalidInput);
    }
    Ok(url)
}

fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    let lengths = (left.len() as u64)
        .to_le_bytes()
        .ct_eq(&(right.len() as u64).to_le_bytes());
    let mut difference = 0_u8;
    let maximum = left.len().max(right.len());
    for index in 0..maximum {
        difference |= left.as_bytes().get(index).copied().unwrap_or(0)
            ^ right.as_bytes().get(index).copied().unwrap_or(0);
    }
    bool::from(lengths & difference.ct_eq(&0))
}

fn validate_binding(provider: &str, session: &str, owner: &str) -> Result<(), OAuthError> {
    validate_text(provider)?;
    validate_text(session)?;
    validate_text(owner)
}
fn validate_text(value: &str) -> Result<(), OAuthError> {
    if value.is_empty() || value.len() > OAUTH_TEXT_BYTES_MAX || value.contains(['\r', '\n', '\0'])
    {
        Err(OAuthError::InvalidInput)
    } else {
        Ok(())
    }
}
fn validate_secret(value: &str) -> Result<(), OAuthError> {
    validate_text(value)
}
fn validate_https(value: &str) -> Result<(), OAuthError> {
    let url = parse_endpoint(value)?;
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
    {
        Err(OAuthError::InvalidInput)
    } else {
        Ok(())
    }
}
fn parse_endpoint(value: &str) -> Result<Url, OAuthError> {
    if value.len() > OAUTH_URL_BYTES_MAX {
        return Err(OAuthError::InvalidInput);
    }
    Url::parse(value).map_err(|_| OAuthError::InvalidInput)
}
fn validate_production_endpoint(url: &Url) -> Result<(), OAuthError> {
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || url.port().is_some()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || ![
            "/oauth/token",
            "/api/accounts/deviceauth/usercode",
            "/api/accounts/deviceauth/token",
        ]
        .contains(&url.path())
    {
        return Err(OAuthError::InvalidInput);
    }
    Ok(())
}

fn validate_redirect(value: &str) -> Result<(), OAuthError> {
    if value != OPENAI_REDIRECT {
        return Err(OAuthError::InvalidInput);
    }
    let url = parse_endpoint(value)?;
    if url.scheme() == "http"
        && url.host_str() == Some("localhost")
        && url.port() == Some(1455)
        && url.path() == "/auth/callback"
        && url.query().is_none()
        && url.fragment().is_none()
    {
        Ok(())
    } else {
        Err(OAuthError::InvalidInput)
    }
}
fn validate_callback_location(origin: &str, redirect: &str) -> Result<(), OAuthError> {
    validate_redirect(redirect)?;
    if origin == "http://localhost:1455" {
        Ok(())
    } else {
        Err(OAuthError::InvalidInput)
    }
}
fn bound(value: &Pending, provider: &str, session: &str, owner: &str) -> bool {
    match value {
        Pending::Browser(v) => v.provider == provider && v.session == session && v.owner == owner,
        Pending::Device(v) => v.provider == provider && v.session == session && v.owner == owner,
    }
}
fn prune(pending: &mut HashMap<String, Pending>, now: u64) {
    pending.retain(|_, value| match value {
        Pending::Browser(v) => now < v.expires_at,
        Pending::Device(v) => now < v.expires_at,
    });
}

mod production;
pub use production::{MonotonicOAuthClock, OsOAuthRandom, ReqwestOAuthHttp};

#[cfg(test)]
mod real_host_tests;
#[cfg(test)]
mod tests;
