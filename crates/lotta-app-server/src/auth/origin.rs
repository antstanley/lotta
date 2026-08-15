use axum::http::HeaderMap;

use crate::{auth::AuthPolicy, error::AppServerError};

/// Rejects Origin-bearing upgrades unless an authentication policy exists.
///
/// # Errors
/// Returns a fixed authentication-required error when policy is absent.
pub fn enforce(headers: &HeaderMap, policy: &AuthPolicy) -> Result<(), AppServerError> {
    if headers.contains_key("origin") && policy.is_none() {
        return Err(AppServerError::OriginAuthenticationRequired);
    }
    Ok(())
}
