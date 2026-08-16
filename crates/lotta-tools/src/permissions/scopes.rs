//! Deterministic bounded loading of file-backed permission scopes.

use super::matcher::{
    PermissionEffect, PermissionError, PermissionRule, canonicalize_invocation_path,
    canonicalize_root,
};
use lotta_domain::PermissionMode;
use serde::Deserialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// Maximum bytes read from one settings file.
pub const PERMISSION_SETTINGS_FILE_BYTES_MAX: usize = 1_048_576;
/// Maximum rules retained in one effect category across loaded sources.
pub const PERMISSION_RULES_PER_CATEGORY_MAX: usize = 1_024;
/// Maximum additional canonical directory roots retained by one policy.
pub const PERMISSION_ADDITIONAL_DIRECTORIES_MAX: usize = 64;

/// Exact four settings locations in least-to-most-specific order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionSourcePaths {
    /// Legacy XDG user settings.
    pub legacy: PathBuf,
    /// Canonical user settings.
    pub user: PathBuf,
    /// Project settings.
    pub project: PathBuf,
    /// Local project settings.
    pub local: PathBuf,
}

impl PermissionSourcePaths {
    /// Constructs exact source paths without reading process environment.
    #[must_use]
    pub fn new(home: &Path, xdg_config_home: &Path, cwd: &Path) -> Self {
        Self {
            legacy: xdg_config_home.join("letta/settings.json"),
            user: home.join(".letta/settings.json"),
            project: cwd.join(".letta/settings.json"),
            local: cwd.join(".letta/settings.local.json"),
        }
    }

    fn ordered(&self) -> [&Path; 4] {
        [&self.legacy, &self.user, &self.project, &self.local]
    }
}

/// Fully merged bounded settings layer.
#[derive(Clone, Debug)]
pub struct LoadedPermissions {
    mode: Option<PermissionMode>,
    rules: Vec<PermissionRule>,
    additional_directories: Vec<PathBuf>,
}

impl LoadedPermissions {
    /// Returns the most-specific configured mode.
    #[must_use]
    pub const fn mode(&self) -> Option<PermissionMode> {
        self.mode
    }

    /// Returns normalized unique file rules.
    #[must_use]
    pub fn rules(&self) -> &[PermissionRule] {
        &self.rules
    }

    /// Returns canonical additional directory roots.
    #[must_use]
    pub fn additional_directories(&self) -> &[PathBuf] {
        &self.additional_directories
    }

    pub(crate) fn into_parts(self) -> (Vec<PermissionRule>, Vec<PathBuf>) {
        (self.rules, self.additional_directories)
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsPermissions {
    mode: Option<PermissionMode>,
    allow: Option<Vec<String>>,
    deny: Option<Vec<String>>,
    ask: Option<Vec<String>>,
    always_ask: Option<Vec<String>>,
    additional_directories: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct SettingsFile {
    permissions: Option<SettingsPermissions>,
    #[serde(flatten)]
    _other: serde_json::Map<String, serde_json::Value>,
}

/// Loads and merges every existing regular source in deterministic precedence order.
///
/// Missing files are skipped. Symlinks, malformed JSON, non-regular files, and any bound or
/// validation failure return a fixed error.
///
/// # Errors
/// Returns [`PermissionError::InvalidSettings`] without retaining source content.
pub fn load_permissions(
    sources: &PermissionSourcePaths,
    cwd: &Path,
) -> Result<LoadedPermissions, PermissionError> {
    let canonical_cwd = canonicalize_root(cwd).map_err(|_| PermissionError::InvalidSettings)?;
    let mut loaded = LoadedPermissions {
        mode: None,
        rules: Vec::new(),
        additional_directories: Vec::new(),
    };
    loaded
        .rules
        .try_reserve(
            PERMISSION_RULES_PER_CATEGORY_MAX
                .checked_mul(4)
                .ok_or(PermissionError::InvalidSettings)?,
        )
        .map_err(|_| PermissionError::InvalidSettings)?;
    loaded
        .additional_directories
        .try_reserve(PERMISSION_ADDITIONAL_DIRECTORIES_MAX)
        .map_err(|_| PermissionError::InvalidSettings)?;
    let mut counts = [0usize; 4];
    for source in sources.ordered() {
        if let Some(settings) = read_source(source)? {
            merge_settings(&mut loaded, &canonical_cwd, settings, &mut counts)?;
        }
    }
    Ok(loaded)
}

fn read_source(path: &Path) -> Result<Option<SettingsPermissions>, PermissionError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(PermissionError::InvalidSettings),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > PERMISSION_SETTINGS_FILE_BYTES_MAX as u64
    {
        return Err(PermissionError::InvalidSettings);
    }
    let file = fs::File::open(path).map_err(|_| PermissionError::InvalidSettings)?;
    let mut content = Vec::new();
    content
        .try_reserve(usize::try_from(metadata.len()).map_err(|_| PermissionError::InvalidSettings)?)
        .map_err(|_| PermissionError::InvalidSettings)?;
    file.take((PERMISSION_SETTINGS_FILE_BYTES_MAX + 1) as u64)
        .read_to_end(&mut content)
        .map_err(|_| PermissionError::InvalidSettings)?;
    if content.len() > PERMISSION_SETTINGS_FILE_BYTES_MAX {
        return Err(PermissionError::InvalidSettings);
    }
    let settings: SettingsFile =
        serde_json::from_slice(&content).map_err(|_| PermissionError::InvalidSettings)?;
    Ok(settings.permissions)
}

fn merge_settings(
    loaded: &mut LoadedPermissions,
    cwd: &Path,
    settings: SettingsPermissions,
    counts: &mut [usize; 4],
) -> Result<(), PermissionError> {
    if settings.mode.is_some() {
        loaded.mode = settings.mode;
    }
    merge_rules(
        loaded,
        settings.deny,
        PermissionEffect::Deny,
        &mut counts[0],
    )?;
    merge_rules(
        loaded,
        settings.always_ask,
        PermissionEffect::AlwaysAsk,
        &mut counts[1],
    )?;
    merge_rules(
        loaded,
        settings.allow,
        PermissionEffect::Allow,
        &mut counts[2],
    )?;
    merge_rules(loaded, settings.ask, PermissionEffect::Ask, &mut counts[3])?;
    if let Some(directories) = settings.additional_directories {
        for directory in directories {
            if loaded.additional_directories.len() >= PERMISSION_ADDITIONAL_DIRECTORIES_MAX {
                return Err(PermissionError::InvalidSettings);
            }
            let canonical = canonicalize_invocation_path(cwd, &directory)
                .map_err(|_| PermissionError::InvalidSettings)?;
            if !loaded.additional_directories.contains(&canonical) {
                loaded.additional_directories.push(canonical);
            }
        }
    }
    Ok(())
}

fn merge_rules(
    loaded: &mut LoadedPermissions,
    rules: Option<Vec<String>>,
    effect: PermissionEffect,
    count: &mut usize,
) -> Result<(), PermissionError> {
    let Some(rules) = rules else { return Ok(()) };
    if rules.len() > PERMISSION_RULES_PER_CATEGORY_MAX {
        return Err(PermissionError::InvalidSettings);
    }
    for raw in rules {
        let rule =
            PermissionRule::parse(&raw, effect).map_err(|_| PermissionError::InvalidSettings)?;
        if loaded.rules.contains(&rule) {
            continue;
        }
        let next = count
            .checked_add(1)
            .ok_or(PermissionError::InvalidSettings)?;
        if next > PERMISSION_RULES_PER_CATEGORY_MAX {
            return Err(PermissionError::InvalidSettings);
        }
        *count = next;
        loaded.rules.push(rule);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/scopes.rs"]
mod tests;
