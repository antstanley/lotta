use lotta_domain::RuntimeScope;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt, path::PathBuf};

/// Maximum mod identity or registration name size.
pub const MOD_ID_BYTES_MAX: usize = 256;
/// Maximum registrations accepted from one host generation.
pub const MOD_REGISTRATIONS_ITEMS_MAX: usize = 1_024;
/// Maximum diagnostics retained in one snapshot.
pub const MOD_DIAGNOSTICS_ITEMS_MAX: usize = 256;
/// Maximum diagnostic message size.
pub const MOD_DIAGNOSTIC_BYTES_MAX: usize = 1_024;

macro_rules! bounded_text {
    ($name:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            /// Validates a non-empty, NUL-free bounded value.
            pub fn new(value: String) -> Result<Self, ModError> {
                if value.is_empty() || value.len() > MOD_ID_BYTES_MAX || value.contains('\0') {
                    return Err(ModError::InvalidIdentity);
                }
                Ok(Self(value))
            }
            /// Borrows the validated value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = ModError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }
    };
}

bounded_text!(ModId, "A stable bounded mod identity.");
bounded_text!(RegistrationName, "A bounded registration identity.");
bounded_text!(
    ConversationHandle,
    "An opaque runtime-scoped conversation handle."
);
impl ConversationHandle {
    /// Mints a process-unique opaque handle. Scope data is retained only by the broker.
    pub fn mint() -> Result<Self, ModError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        Self::new(format!("lotta-mod-{sequence:016x}"))
    }
}

/// Monotonically increasing mod generation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Generation(pub u64);
impl Generation {
    /// Advances a generation without wrapping.
    pub fn next(self) -> Result<Self, ModError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(ModError::GenerationExhausted)
    }
}

/// Exact owner attached to every mod registration and call.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModOwner {
    /// Stable mod identifier.
    pub id: ModId,
    /// Activation generation.
    pub generation: Generation,
}

/// Exact pinned capability declarations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Capability {
    /// Tool registration and execution.
    #[serde(rename = "tools")]
    Tools,
    /// Command registration and execution.
    #[serde(rename = "commands")]
    Commands,
    /// Provider registration.
    #[serde(rename = "providers")]
    Providers,
    /// Permission registration.
    #[serde(rename = "permissions")]
    Permissions,
    /// Lifecycle events.
    #[serde(rename = "events.lifecycle")]
    EventsLifecycle,
    /// Turn events.
    #[serde(rename = "events.turns")]
    EventsTurns,
    /// Tool events.
    #[serde(rename = "events.tools")]
    EventsTools,
    /// Compaction events.
    #[serde(rename = "events.compact")]
    EventsCompact,
    /// Language-model events.
    #[serde(rename = "events.llm")]
    EventsLlm,
    /// Panel metadata.
    #[serde(rename = "ui.panels")]
    UiPanels,
}

/// Runtime scope retained privately by the capability broker, including a confined cwd.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModRuntimeScope {
    runtime: RuntimeScope,
    cwd: PathBuf,
}
impl ModRuntimeScope {
    /// Constructs a broker-owned scope from validated composition values.
    #[must_use]
    pub fn new(runtime: RuntimeScope, cwd: PathBuf) -> Self {
        Self { runtime, cwd }
    }
    /// Borrows the runtime identity for an authorized capability adapter.
    #[must_use]
    pub fn runtime(&self) -> &RuntimeScope {
        &self.runtime
    }
    /// Borrows the confined cwd for an authorized capability adapter.
    #[must_use]
    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }
}

/// Stable mod subsystem failures without source or credential contents.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ModError {
    /// An identity or schema field was malformed.
    #[error("invalid mod identity")]
    InvalidIdentity,
    /// A generation could not advance.
    #[error("mod generation exhausted")]
    GenerationExhausted,
    /// A capability was not declared.
    #[error("mod capability not declared")]
    UndeclaredCapability,
    /// Runtime scope, handle, owner, or generation did not match.
    #[error("mod scope is invalid")]
    InvalidScope,
    /// Registration schema, bound, or duplicate validation failed.
    #[error("mod registration rejected")]
    InvalidRegistration,
    /// Host protocol validation failed.
    #[error("mod host protocol rejected")]
    Protocol,
    /// Host call exceeded its local timeout.
    #[error("mod host call timed out")]
    Timeout,
    /// Host call was cancelled.
    #[error("mod host call cancelled")]
    Cancelled,
    /// Host process or queue is unavailable.
    #[error("mod host unavailable")]
    Unavailable,
    /// Registry publication failed.
    #[error("mod registry publication failed")]
    Publication,
    /// Remote host returned a typed JSON-RPC error.
    #[error("mod host remote error {code} for {owner:?}")]
    Remote {
        /// Stable mod owner identifier.
        owner: ModId,
        /// Stable JSON-RPC error code.
        code: i32,
    },
}
