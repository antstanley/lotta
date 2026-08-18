use crate::{StoreError, StoreErrorKind};
use base64::Engine as _;
use lotta_domain::{AgentId, ConversationId};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Environment variable overriding the local-backend root.
pub const LETTA_LOCAL_BACKEND_DIR: &str = "LETTA_LOCAL_BACKEND_DIR";
/// Maximum accepted local-backend root path UTF-8 bytes.
pub const STORE_ROOT_PATH_BYTES_MAX: usize = 4_096;
/// Maximum accepted encoded storage key bytes.
pub const STORE_KEY_BYTES_MAX: usize = 1_024;

/// Validated local-backend storage layout rooted at an explicit confined path.
#[derive(Clone, Debug)]
pub struct StorePaths {
    root: PathBuf,
}

impl StorePaths {
    /// Resolves the root from process environment and `HOME` without requiring it to exist.
    ///
    /// # Errors
    /// Returns a typed invalid root or filesystem failure.
    pub fn resolve() -> Result<Self, StoreError> {
        Self::resolve_from(
            std::env::var_os(LETTA_LOCAL_BACKEND_DIR),
            std::env::var_os("HOME"),
        )
    }

    /// Resolves an explicitly supplied override and home value.
    ///
    /// # Errors
    /// Returns a typed invalid root or filesystem failure.
    pub fn resolve_from(
        override_root: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<Self, StoreError> {
        let root = match override_root {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            Some(_) => return Err(invalid_root("LETTA_LOCAL_BACKEND_DIR")),
            None => home
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .map(|value| value.join(".letta/lc-local-backend"))
                .ok_or_else(|| invalid_root("HOME"))?,
        };
        Self::new(root)
    }

    /// Validates an explicit UTF-8 root lexically and rejects an existing symlink root.
    ///
    /// # Errors
    /// Returns a typed invalid root or filesystem failure.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        let text = root
            .to_str()
            .ok_or_else(|| StoreError::new(StoreErrorKind::InvalidPath, &root))?;
        if !root.is_absolute()
            || text.len() > STORE_ROOT_PATH_BYTES_MAX
            || root
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, root));
        }
        if root.exists() {
            crate::confinement::validate_root(&root)?;
        }
        Ok(Self { root })
    }

    /// Returns the configured root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Returns the canonical agents directory location.
    #[must_use]
    pub fn agents(&self) -> PathBuf {
        self.root.join("agents")
    }
    /// Returns the canonical conversations directory location.
    #[must_use]
    pub fn conversations(&self) -> PathBuf {
        self.root.join("conversations")
    }
    /// Returns the canonical `MemFS` directory location.
    #[must_use]
    pub fn memfs(&self) -> PathBuf {
        self.root.join("memfs")
    }
    /// Returns the canonical providers directory location.
    #[must_use]
    pub fn providers(&self) -> PathBuf {
        self.root.join("providers")
    }
    /// Returns the canonical disposable indexes directory location.
    #[must_use]
    pub fn indexes(&self) -> PathBuf {
        self.root.join("indexes")
    }
    /// Returns the canonical runtime state directory location.
    #[must_use]
    pub fn runtime(&self) -> PathBuf {
        self.root.join("runtime")
    }
    /// Returns the provider authentication file location.
    #[must_use]
    pub fn provider_auth(&self) -> PathBuf {
        self.providers().join("auth.json")
    }
    /// Returns a confined agent record path.
    ///
    /// # Errors
    /// Rejects an identifier whose encoded segment exceeds the storage bound.
    pub fn agent_record(&self, id: &AgentId) -> Result<PathBuf, StoreError> {
        let segment = encode_segment(id.as_str())?;
        let mut name = segment;
        name.try_reserve_exact(5)
            .map_err(|_| StoreError::new(StoreErrorKind::Limit, id.as_str()))?;
        name.push_str(".json");
        Ok(self.agents().join(name))
    }
    /// Returns a confined conversation record directory.
    ///
    /// # Errors
    /// Rejects a key whose encoded segment exceeds the storage bound.
    pub fn conversation_dir(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
    ) -> Result<PathBuf, StoreError> {
        let key = ConversationKey::from_ids(agent, conversation);
        Ok(self.conversations().join(key.encoded()?))
    }
    /// Returns one agent's baseline `MemFS` memory directory.
    ///
    /// # Errors
    /// Rejects a non-normal, malicious, or over-limit raw identifier segment.
    pub fn memory(&self, id: &AgentId) -> Result<PathBuf, StoreError> {
        validate_raw_segment(id.as_str())?;
        Ok(self.memfs().join(id.as_str()).join("memory"))
    }
}

/// Typed validated baseline conversation directory key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationKey {
    /// Agent-scoped virtual default conversation.
    Default(AgentId),
    /// Named conversation keyed only by its conversation ID.
    Named(ConversationId),
}

impl ConversationKey {
    /// Selects the exact baseline key form for these IDs.
    #[must_use]
    pub fn from_ids(agent: &AgentId, conversation: &ConversationId) -> Self {
        if conversation.is_default() {
            Self::Default(agent.clone())
        } else {
            Self::Named(conversation.clone())
        }
    }

    /// Encodes the key as bounded URL-safe base64 without padding.
    ///
    /// # Errors
    /// Returns a typed limit or allocation failure.
    pub fn encoded(&self) -> Result<String, StoreError> {
        match self {
            Self::Default(agent) => encode_prefixed("default:", agent.as_str()),
            Self::Named(conversation) => encode_prefixed("conversation:", conversation.as_str()),
        }
    }

    /// Decodes and validates one canonical unpadded key segment.
    ///
    /// # Errors
    /// Rejects malformed, noncanonical, empty, or over-limit keys.
    pub fn decode(encoded: &str) -> Result<Self, StoreError> {
        if encoded.is_empty() || encoded.len() > STORE_KEY_BYTES_MAX || encoded.contains('=') {
            return Err(invalid_key(encoded));
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| invalid_key(encoded))?;
        let text = String::from_utf8(bytes).map_err(|_| invalid_key(encoded))?;
        let key = if let Some(id) = text.strip_prefix("default:") {
            Self::Default(AgentId::accept(id).map_err(|_| invalid_key(encoded))?)
        } else if let Some(id) = text.strip_prefix("conversation:") {
            Self::Named(ConversationId::accept(id).map_err(|_| invalid_key(encoded))?)
        } else {
            return Err(invalid_key(encoded));
        };
        if key.encoded()? != encoded {
            return Err(invalid_key(encoded));
        }
        Ok(key)
    }
}

fn encode_segment(text: &str) -> Result<String, StoreError> {
    encode_prefixed("", text)
}

fn encode_prefixed(prefix: &str, id: &str) -> Result<String, StoreError> {
    let source_len = prefix
        .len()
        .checked_add(id.len())
        .ok_or_else(|| invalid_key(id))?;
    let encoded_len = source_len
        .checked_mul(4)
        .and_then(|value| value.checked_add(2))
        .map(|value| value / 3)
        .ok_or_else(|| invalid_key(id))?;
    if encoded_len > STORE_KEY_BYTES_MAX {
        return Err(invalid_key(id));
    }
    let mut text = String::new();
    text.try_reserve_exact(source_len)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, id))?;
    text.push_str(prefix);
    text.push_str(id);
    let mut encoded = String::new();
    encoded
        .try_reserve_exact(encoded_len)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, id))?;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode_string(text.as_bytes(), &mut encoded);
    if encoded.len() > STORE_KEY_BYTES_MAX {
        return Err(invalid_key(id));
    }
    Ok(encoded)
}

fn validate_raw_segment(value: &str) -> Result<(), StoreError> {
    if value.is_empty()
        || value.len() > STORE_KEY_BYTES_MAX
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', '\0'])
        || Path::new(value).is_absolute()
        || !matches!(
            Path::new(value).components().next(),
            Some(Component::Normal(_))
        )
        || Path::new(value).components().count() != 1
    {
        return Err(invalid_key(value));
    }
    Ok(())
}

fn invalid_root(name: &str) -> StoreError {
    StoreError::new(StoreErrorKind::InvalidPath, name)
}
fn invalid_key(value: &str) -> StoreError {
    StoreError::new(StoreErrorKind::InvalidPath, PathBuf::from(value))
}
