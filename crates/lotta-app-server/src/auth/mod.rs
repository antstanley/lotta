use std::{fs::File, io::Read, path::Path};

use axum::http::{HeaderMap, header::AUTHORIZATION};
use lotta_domain::{Clock, Secret};
use sha2::{Digest, Sha256};

use crate::{config::ServerArgs, error::AppServerError};

/// Constant-time capability-token verification.
pub mod capability_token;
/// Origin-bearing upgrade policy.
pub mod origin;
/// Dependency-minimal signed-bearer verification.
#[path = "signed_bearer.rs"]
mod signed_bearer_token;

/// Largest accepted Authorization field value.
pub const AUTHORIZATION_HEADER_BYTES_MAX: usize = 16_384;
const SIGNED_BEARER_SECRET_BYTES_MIN: usize = 32;
/// Largest accepted secret file before trimming.
pub const SECRET_FILE_BYTES_MAX: u64 = 65_536;

/// Fully prepared websocket authentication policy.
pub enum AuthPolicy {
    /// No credentials are configured.
    None,
    /// Compare candidate token hashes against a protected digest.
    CapabilityToken {
        /// Protected expected SHA-256 digest.
        digest: Secret<[u8; 32]>,
    },
    /// Verify an HS256 signed bearer token.
    SignedBearer {
        /// Protected HMAC key.
        shared_secret: Secret<Vec<u8>>,
        /// Optional exact issuer.
        issuer: Option<String>,
        /// Optional exact audience.
        audience: Option<String>,
        /// Allowed temporal skew.
        skew_seconds: u32,
    },
}

impl std::fmt::Debug for AuthPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.mode_name())
    }
}

impl AuthPolicy {
    /// Reads and validates policy material once.
    ///
    /// # Errors
    /// Returns a scrubbed configuration or secret-file error before bind.
    pub fn prepare(args: &ServerArgs) -> Result<Self, AppServerError> {
        let any = args.ws_token_file.is_some()
            || args.ws_token_sha256.is_some()
            || args.ws_shared_secret_file.is_some()
            || args.ws_issuer.is_some()
            || args.ws_audience.is_some()
            || args.ws_max_clock_skew_seconds.is_some();
        match args.ws_auth.as_deref() {
            None if any => Err(AppServerError::Config("auth flags require --ws-auth")),
            None => Ok(Self::None),
            Some("capability-token") => prepare_capability(args),
            Some("signed-bearer-token") => prepare_signed(args),
            Some(_) => Err(AppServerError::Config("unsupported --ws-auth mode")),
        }
    }

    /// Returns whether no policy is configured.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Returns a safe mode name for diagnostics.
    #[must_use]
    pub const fn mode_name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CapabilityToken { .. } => "capability-token",
            Self::SignedBearer { .. } => "signed-bearer-token",
        }
    }

    /// Authorizes exactly one bounded Bearer header using the configured mode.
    ///
    /// # Errors
    /// Returns a fixed unauthorized error for absent, malformed, or invalid credentials.
    pub fn authorize(&self, headers: &HeaderMap, clock: &dyn Clock) -> Result<(), AppServerError> {
        match self {
            Self::None => Ok(()),
            Self::CapabilityToken { digest } => {
                let token = bearer_token(headers)?;
                capability_token::verify(token.as_bytes(), digest)
            }
            Self::SignedBearer {
                shared_secret,
                issuer,
                audience,
                skew_seconds,
            } => {
                let token = bearer_token(headers)?;
                signed_bearer_token::verify_token(
                    token,
                    shared_secret,
                    issuer.as_deref(),
                    audience.as_deref(),
                    *skew_seconds,
                    clock,
                )
            }
        }
    }

    /// Returns a stable verified principal independent of token renewal.
    ///
    /// # Errors
    /// Returns unauthorized when credentials cannot yield a verified principal.
    pub fn principal(&self, headers: &HeaderMap) -> Result<String, AppServerError> {
        match self {
            Self::None => Ok("anonymous".to_owned()),
            Self::CapabilityToken { .. } => Ok("capability-token".to_owned()),
            Self::SignedBearer { .. } => {
                signed_bearer_token::verified_principal(bearer_token(headers)?)
            }
        }
    }
}

fn prepare_capability(args: &ServerArgs) -> Result<AuthPolicy, AppServerError> {
    if args.ws_shared_secret_file.is_some()
        || args.ws_issuer.is_some()
        || args.ws_audience.is_some()
        || args.ws_max_clock_skew_seconds.is_some()
    {
        return Err(AppServerError::Config(
            "signed bearer flags require signed mode",
        ));
    }
    let digest = match (&args.ws_token_file, &args.ws_token_sha256) {
        (Some(_), Some(_)) => return Err(AppServerError::Config("capability sources conflict")),
        (None, None) => return Err(AppServerError::Config("capability source is required")),
        (Some(path), None) => {
            let token = read_secret(path)?;
            Sha256::digest(token).into()
        }
        (None, Some(hex)) => capability_token::decode_digest(hex)?,
    };
    Ok(AuthPolicy::CapabilityToken {
        digest: Secret::new(digest),
    })
}

fn prepare_signed(args: &ServerArgs) -> Result<AuthPolicy, AppServerError> {
    if args.ws_token_file.is_some() || args.ws_token_sha256.is_some() {
        return Err(AppServerError::Config(
            "capability flags require capability mode",
        ));
    }
    let path = args
        .ws_shared_secret_file
        .as_ref()
        .ok_or(AppServerError::Config(
            "signed bearer secret file is required",
        ))?;
    if !path.is_absolute() {
        return Err(AppServerError::Config("secret file path must be absolute"));
    }
    let skew = args
        .ws_max_clock_skew_seconds
        .unwrap_or(crate::config::AUTH_CLOCK_SKEW_SECONDS_DEFAULT);
    if skew > crate::config::AUTH_CLOCK_SKEW_SECONDS_MAX {
        return Err(AppServerError::Config("clock skew exceeds safety maximum"));
    }
    let secret = read_secret(path)?;
    if secret.len() < SIGNED_BEARER_SECRET_BYTES_MIN {
        return Err(AppServerError::Config("signed bearer secret is too short"));
    }
    Ok(AuthPolicy::SignedBearer {
        shared_secret: Secret::new(secret),
        issuer: normalize(args.ws_issuer.clone()),
        audience: normalize(args.ws_audience.clone()),
        skew_seconds: skew,
    })
}

fn normalize(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn read_secret(path: &Path) -> Result<Vec<u8>, AppServerError> {
    if !path.is_absolute() {
        return Err(AppServerError::Config("secret file path must be absolute"));
    }
    let mut file = File::open(path).map_err(|_| secret_file_error(path))?;
    let metadata = file.metadata().map_err(|_| secret_file_error(path))?;
    if metadata.len() > SECRET_FILE_BYTES_MAX {
        return Err(AppServerError::Config("secret file exceeds size limit"));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| AppServerError::Config("secret file exceeds size limit"))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(SECRET_FILE_BYTES_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| secret_file_error(path))?;
    if bytes.len() as u64 > SECRET_FILE_BYTES_MAX {
        return Err(AppServerError::Config("secret file exceeds size limit"));
    }
    let start = bytes.iter().position(|byte| !byte.is_ascii_whitespace());
    let end = bytes.iter().rposition(|byte| !byte.is_ascii_whitespace());
    match (start, end) {
        (Some(first), Some(last)) => Ok(bytes[first..=last].to_vec()),
        _ => Err(AppServerError::Config("secret file must not be empty")),
    }
}

fn secret_file_error(path: &Path) -> AppServerError {
    AppServerError::SecretFile {
        path: path.display().to_string(),
    }
}

/// Parses exactly one case-insensitive non-empty Bearer credential.
///
/// # Errors
/// Returns a fixed unauthorized error for missing, duplicate, malformed, or oversized input.
pub fn bearer_token(headers: &HeaderMap) -> Result<&str, AppServerError> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next().ok_or(AppServerError::Unauthorized)?;
    if values.next().is_some() {
        return Err(AppServerError::Unauthorized);
    }
    let text = value.to_str().map_err(|_| AppServerError::Unauthorized)?;
    if text.len() > AUTHORIZATION_HEADER_BYTES_MAX {
        return Err(AppServerError::Unauthorized);
    }
    let separator = text
        .find(char::is_whitespace)
        .ok_or(AppServerError::Unauthorized)?;
    let (scheme, remainder) = text.split_at(separator);
    let token = remainder.trim();
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.chars().any(char::is_whitespace)
    {
        return Err(AppServerError::Unauthorized);
    }
    Ok(token)
}

#[cfg(test)]
#[path = "tests/capability.rs"]
mod capability;
#[cfg(test)]
#[path = "tests/headers.rs"]
mod headers;
#[cfg(test)]
#[path = "tests/modes.rs"]
mod modes;
#[cfg(test)]
#[path = "tests/non_loopback_requires_auth.rs"]
mod non_loopback_requires_auth;
#[cfg(test)]
#[path = "tests/origin_bearing_loopback_rejected.rs"]
mod origin_bearing_loopback_rejected;
#[cfg(test)]
#[path = "tests/secrets_never_logged.rs"]
mod secrets_never_logged;
#[cfg(test)]
#[path = "tests/signed_bearer.rs"]
mod signed_bearer;
#[cfg(test)]
#[path = "tests/skew_ceiling.rs"]
mod skew_ceiling;
#[cfg(test)]
#[path = "tests/test_support.rs"]
mod test_support;
