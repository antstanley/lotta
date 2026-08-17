//! Secret-contained credential lookup and atomic OAuth refresh.

use super::client::CredentialRef;
use std::{fmt, future::Future, pin::Pin};

/// Maximum credential value accepted from the credential store.
pub const MCP_CREDENTIAL_BYTES_MAX: usize = 256 * 1024;

/// Secret value whose formatting never reveals bytes.
pub struct SecretValue(Vec<u8>);
impl SecretValue {
    /// Creates a non-empty bounded secret value.
    pub fn new(value: Vec<u8>) -> Result<Self, CredentialError> {
        if value.is_empty() || value.len() > MCP_CREDENTIAL_BYTES_MAX {
            return Err(CredentialError::Invalid);
        }
        Ok(Self(value))
    }
    /// Executes an operation with an ephemeral secret byte borrow.
    pub fn expose<T>(&self, operation: impl FnOnce(&[u8]) -> T) -> T {
        operation(&self.0)
    }
}
impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}
impl fmt::Display for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}
impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// Credential value plus optimistic store revision.
pub struct VersionedSecret {
    /// Redacted secret value.
    pub value: SecretValue,
    /// Store revision read with the value.
    pub revision: u64,
}
impl fmt::Debug for VersionedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VersionedSecret([REDACTED])")
    }
}

/// Fixed secret-free credential failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialError {
    /// Reference was unknown.
    #[error("MCP credential was not found")]
    Unknown,
    /// Stored credential was invalid or over bound.
    #[error("MCP credential was invalid")]
    Invalid,
    /// Store revision changed before replacement.
    #[error("MCP credential revision changed")]
    RevisionMismatch,
    /// Credential store operation failed.
    #[error("MCP credential store failed")]
    Store,
}

/// Future returned by credential-store operations.
pub type CredentialFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CredentialError>> + Send + 'a>>;

/// Explicit just-in-time credential store boundary.
pub trait CredentialStorePort: Send + Sync {
    /// Resolves one credential reference immediately before transport construction.
    fn resolve(&self, reference: &CredentialRef) -> CredentialFuture<'_, VersionedSecret>;
    /// Atomically replaces a value only at the revision previously resolved.
    fn compare_replace(
        &self,
        reference: &CredentialRef,
        expected_revision: u64,
        replacement: SecretValue,
    ) -> CredentialFuture<'_, u64>;
}

/// Parsed OAuth token state retained only in the credential store.
pub struct OAuthTokens {
    /// Access token used for Authorization.
    pub access_token: SecretValue,
    /// Optional refresh token.
    pub refresh_token: Option<SecretValue>,
    /// Optional client secret.
    pub client_secret: Option<SecretValue>,
}
impl fmt::Debug for OAuthTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OAuthTokens([REDACTED])")
    }
}

/// Atomically persists refreshed serialized OAuth state at its resolved revision.
pub async fn store_refreshed_tokens(
    store: &dyn CredentialStorePort,
    reference: &CredentialRef,
    expected_revision: u64,
    serialized: SecretValue,
) -> Result<u64, CredentialError> {
    store
        .compare_replace(reference, expected_revision, serialized)
        .await
}
