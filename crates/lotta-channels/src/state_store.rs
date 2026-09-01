//! Typed read-only projection of the canonical persisted channel tree.

use crate::{
    control_plane::{CHANNEL_STATE_ROWS_MAX, ChannelState},
    topology::{ChannelStore, TopologyError},
};
use lotta_domain::{ChannelAccount, ChannelRoute};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Maximum entries recursively inspected below one channel directory.
pub const CHANNEL_TREE_ENTRIES_MAX: usize = 512;

/// Typed canonical channel state reader.
#[derive(Clone, Debug)]
pub struct ChannelStateStore {
    root: PathBuf,
}

/// Exact canonical `config.yaml` object.
///
/// Plugin configuration is intentionally plugin-owned. The object is typed as a
/// YAML mapping rather than flattened into aliases or interpreted by topology.
#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
pub struct ChannelConfig {
    /// Exact plugin-owned YAML mapping.
    pub values: serde_yaml::Mapping,
}

/// Exact canonical `accounts.json` envelope.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelAccounts {
    accounts: Vec<ChannelAccount>,
}

/// Exact canonical `routing.yaml` envelope.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelRouting {
    routes: Vec<ChannelRoute>,
}

/// Exact canonical `pairing.yaml` envelope.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelPairing {
    pending: Vec<Value>,
    approved: Vec<Value>,
}

/// Exact canonical `targets.json` envelope.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelTargets {
    targets: Vec<Value>,
}

/// One fully decoded canonical channel directory.
#[derive(Clone, Debug)]
pub struct CanonicalChannelState {
    /// Canonical channel identifier.
    pub id: String,
    /// Plugin-owned channel configuration.
    pub config: Option<ChannelConfig>,
    /// Canonical accounts envelope.
    pub accounts: ChannelAccounts,
    /// Canonical routing envelope.
    pub routing: ChannelRouting,
    /// Canonical pairing envelope.
    pub pairing: ChannelPairing,
    /// Canonical targets envelope.
    pub targets: ChannelTargets,
}

impl ChannelStateStore {
    /// Creates a typed reader over an already validated canonical root.
    #[must_use]
    pub fn new(store: &ChannelStore) -> Self {
        Self::from_root(store.root())
    }

    pub(crate) fn from_root(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Decodes every canonical channel directory without aliases or shallow parsing.
    ///
    /// # Errors
    /// Fails closed for malformed, oversized, linked, or special state.
    pub fn load(&self) -> Result<Vec<CanonicalChannelState>, TopologyError> {
        let mut channels = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|_| TopologyError::Path)? {
            let entry = entry.map_err(|_| TopologyError::Path)?;
            let metadata = safe_metadata(&entry.path())?;
            if !metadata.is_dir() {
                continue;
            }
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| TopologyError::Path)?;
            if id.starts_with('.') {
                continue;
            }
            validate_channel_id(&id)?;
            validate_tree(&entry.path())?;
            channels.push(load_channel(id, &entry.path())?);
            if channels.len() > CHANNEL_STATE_ROWS_MAX {
                return Err(TopologyError::Path);
            }
        }
        channels.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(channels)
    }

    /// Returns the canonical bounded `/channels` projection.
    ///
    /// # Errors
    /// Fails closed when any canonical channel state is invalid.
    pub fn snapshot(&self) -> Result<Vec<ChannelState>, TopologyError> {
        self.load()?
            .into_iter()
            .map(|channel| {
                bounded(channel.accounts.accounts.len())?;
                bounded(channel.routing.routes.len())?;
                bounded(channel.pairing.pending.len())?;
                bounded(channel.pairing.approved.len())?;
                bounded(channel.targets.targets.len())?;
                let enabled = channel
                    .accounts
                    .accounts
                    .iter()
                    .any(|account| account.enabled && account.configured);
                Ok(ChannelState {
                    id: channel.id,
                    enabled,
                    accounts: channel.accounts.accounts.len(),
                    routes: channel.routing.routes.len(),
                    pending_pairings: channel.pairing.pending.len(),
                    targets: channel.targets.targets.len(),
                })
            })
            .collect()
    }

    /// Returns exact enabled/configured account routes eligible for restoration.
    ///
    /// # Errors
    /// Fails closed when canonical state is invalid or internally inconsistent.
    pub fn restorable_routes(&self) -> Result<Vec<ChannelRoute>, TopologyError> {
        let mut routes = Vec::new();
        for channel in self.load()? {
            for route in channel.routing.routes {
                let account = channel.accounts.accounts.iter().find(|account| {
                    account.channel_id.as_str() == route.channel_id.as_str()
                        && account.account_id.as_str() == route.account_id.as_str()
                });
                if route.enabled
                    && account.is_some_and(|account| account.enabled && account.configured)
                {
                    routes.push(route);
                    if routes.len() > CHANNEL_STATE_ROWS_MAX {
                        return Err(TopologyError::Path);
                    }
                }
            }
        }
        Ok(routes)
    }

    /// Returns whether at least one canonical account is restorable.
    ///
    /// This is intentionally account-based: canonical `config.yaml` has no
    /// topology-level `enabled` alias.
    ///
    /// # Errors
    /// Fails closed when canonical state is malformed.
    pub fn has_restorable_account(&self) -> Result<bool, TopologyError> {
        Ok(self.load()?.iter().any(|channel| {
            channel
                .accounts
                .accounts
                .iter()
                .any(|account| account.enabled && account.configured)
        }))
    }
}

fn load_channel(id: String, root: &Path) -> Result<CanonicalChannelState, TopologyError> {
    let config = read_optional_yaml(&root.join("config.yaml"))?;
    let accounts = read_json_or(
        &root.join("accounts.json"),
        ChannelAccounts { accounts: vec![] },
    )?;
    let routing = read_yaml_or(
        &root.join("routing.yaml"),
        ChannelRouting { routes: vec![] },
    )?;
    let pairing = read_yaml_or(
        &root.join("pairing.yaml"),
        ChannelPairing {
            pending: vec![],
            approved: vec![],
        },
    )?;
    let targets = read_json_or(
        &root.join("targets.json"),
        ChannelTargets { targets: vec![] },
    )?;
    Ok(CanonicalChannelState {
        id,
        config,
        accounts,
        routing,
        pairing,
        targets,
    })
}

fn read_optional_yaml(path: &Path) -> Result<Option<ChannelConfig>, TopologyError> {
    if !path.exists() {
        return Ok(None);
    }
    let value =
        serde_yaml::from_slice(&read_regular_bounded(path)?).map_err(|_| TopologyError::Path)?;
    Ok(Some(value))
}

fn read_json_or<T: for<'de> Deserialize<'de>>(path: &Path, default: T) -> Result<T, TopologyError> {
    if !path.exists() {
        return Ok(default);
    }
    serde_json::from_slice(&read_regular_bounded(path)?).map_err(|_| TopologyError::Path)
}

fn read_yaml_or<T: for<'de> Deserialize<'de>>(path: &Path, default: T) -> Result<T, TopologyError> {
    if !path.exists() {
        return Ok(default);
    }
    serde_yaml::from_slice(&read_regular_bounded(path)?).map_err(|_| TopologyError::Path)
}

fn read_regular_bounded(path: &Path) -> Result<Vec<u8>, TopologyError> {
    let metadata = safe_metadata(path)?;
    if !metadata.is_file() || metadata.len() > crate::control_plane::CONTROL_FRAME_BYTES_MAX as u64
    {
        return Err(TopologyError::Path);
    }
    #[cfg(unix)]
    if std::os::unix::fs::MetadataExt::nlink(&metadata) != 1 {
        return Err(TopologyError::Path);
    }
    let bytes = std::fs::read(path).map_err(|_| TopologyError::Path)?;
    if bytes.len() > crate::control_plane::CONTROL_FRAME_BYTES_MAX {
        return Err(TopologyError::Path);
    }
    Ok(bytes)
}

fn safe_metadata(path: &Path) -> Result<std::fs::Metadata, TopologyError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| TopologyError::Path)?;
    if metadata.file_type().is_symlink() {
        return Err(TopologyError::Path);
    }
    Ok(metadata)
}

fn validate_tree(root: &Path) -> Result<(), TopologyError> {
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0_usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(|_| TopologyError::Path)? {
            visited = visited.checked_add(1).ok_or(TopologyError::Path)?;
            if visited > CHANNEL_TREE_ENTRIES_MAX {
                return Err(TopologyError::Path);
            }
            let entry = entry.map_err(|_| TopologyError::Path)?;
            let metadata = safe_metadata(&entry.path())?;
            if !(metadata.is_file() || metadata.is_dir()) {
                return Err(TopologyError::Path);
            }
            #[cfg(unix)]
            if metadata.is_file() && std::os::unix::fs::MetadataExt::nlink(&metadata) != 1 {
                return Err(TopologyError::Path);
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

fn validate_channel_id(id: &str) -> Result<(), TopologyError> {
    if id.is_empty()
        || id.len() > crate::topology::CHANNEL_OWNER_ID_BYTES_MAX
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(TopologyError::Path);
    }
    Ok(())
}

fn bounded(count: usize) -> Result<(), TopologyError> {
    (count <= CHANNEL_STATE_ROWS_MAX)
        .then_some(())
        .ok_or(TopologyError::Path)
}
