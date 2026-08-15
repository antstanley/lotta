use crate::{StoreError, StoreErrorKind};
use std::path::{Component, Path, PathBuf};

/// Maximum UTF-8 bytes in a side-store root or resolved path.
pub const SIDE_PATH_BYTES_MAX: usize = 4_096;
/// Maximum path components in a side-store root or resolved path.
pub const SIDE_PATH_DEPTH_MAX: usize = 64;
/// Maximum UTF-8 bytes in a schedule or channel identifier.
pub const SIDE_ID_BYTES_MAX: usize = 255;
/// Maximum explicitly authorized project workspaces.
pub const SIDE_WORKSPACES_MAX: usize = 256;

/// One known per-channel file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelFile {
    /// Channel configuration bytes.
    Config,
    /// Account configuration bytes.
    Accounts,
    /// Routing configuration bytes.
    Routing,
    /// Pairing configuration bytes.
    Pairing,
    /// Target configuration bytes.
    Targets,
}

impl ChannelFile {
    const fn name(self) -> &'static str {
        match self {
            Self::Config => "config.yaml",
            Self::Accounts => "accounts.json",
            Self::Routing => "routing.yaml",
            Self::Pairing => "pairing.yaml",
            Self::Targets => "targets.json",
        }
    }
}

/// One project-local settings file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectFile {
    /// Shared project settings.
    Settings,
    /// Machine-local project settings.
    LocalSettings,
}

impl ProjectFile {
    const fn name(self) -> &'static str {
        match self {
            Self::Settings => "settings.json",
            Self::LocalSettings => "settings.local.json",
        }
    }
}

/// Explicit pure authority for all baseline side-store locations.
#[derive(Clone, Debug)]
pub struct SidePaths {
    home: PathBuf,
    letta_home: PathBuf,
    workspaces: Vec<PathBuf>,
}

impl SidePaths {
    /// Creates paths from explicit absolute roots without reading process environment.
    ///
    /// # Errors
    /// Rejects relative, non-UTF-8, excessive, duplicate, or invalid lexical roots.
    pub fn new(
        home: impl Into<PathBuf>,
        letta_home: Option<PathBuf>,
        workspaces: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, StoreError> {
        let home = validate_root(home.into())?;
        let letta_home = validate_root(letta_home.unwrap_or_else(|| home.join(".letta")))?;
        let mut scoped = Vec::new();
        for workspace in workspaces {
            let workspace = validate_root(workspace)?;
            if scoped.len() >= SIDE_WORKSPACES_MAX {
                return Err(StoreError::new(StoreErrorKind::Limit, &workspace));
            }
            match scoped.binary_search(&workspace) {
                Ok(_) => return Err(invalid(&workspace)),
                Err(index) => {
                    scoped
                        .try_reserve(1)
                        .map_err(|_| StoreError::new(StoreErrorKind::Limit, &workspace))?;
                    scoped.insert(index, workspace);
                }
            }
        }
        Ok(Self {
            home,
            letta_home,
            workspaces: scoped,
        })
    }

    /// Returns the explicit home root.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Returns the cron/run root.
    #[must_use]
    pub fn letta_home(&self) -> &Path {
        &self.letta_home
    }

    /// Returns global settings at `<home>/.letta/settings.json`.
    ///
    /// # Errors
    /// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
    pub fn settings(&self) -> Result<PathBuf, StoreError> {
        join_checked(&self.home, &[".letta", "settings.json"])
    }

    /// Returns `${LETTA_HOME}/crons.json`.
    ///
    /// # Errors
    /// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
    pub fn crons(&self) -> Result<PathBuf, StoreError> {
        join_checked(&self.letta_home, &["crons.json"])
    }

    /// Returns `${LETTA_HOME}/runs/<schedule-id>.jsonl`.
    ///
    /// # Errors
    /// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
    pub fn run_log(&self, schedule_id: &str) -> Result<PathBuf, StoreError> {
        validate_id(schedule_id)?;
        let name = suffixed(schedule_id, ".jsonl")?;
        join_checked(&self.letta_home, &["runs", &name])
    }

    /// Returns the global channels directory.
    ///
    /// # Errors
    /// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
    pub fn channels_root(&self) -> Result<PathBuf, StoreError> {
        join_checked(&self.home, &[".letta", "channels"])
    }

    /// Returns the pending channel control file.
    ///
    /// # Errors
    /// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
    pub fn pending_control(&self) -> Result<PathBuf, StoreError> {
        join_checked(
            &self.home,
            &[".letta", "channels", "pending-control-requests.json"],
        )
    }

    /// Returns one validated channel directory.
    ///
    /// # Errors
    /// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
    pub fn channel_dir(&self, channel_id: &str) -> Result<PathBuf, StoreError> {
        validate_id(channel_id)?;
        join_checked(&self.home, &[".letta", "channels", channel_id])
    }

    /// Returns one known file in a validated channel directory.
    ///
    /// # Errors
    /// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
    pub fn channel_file(&self, channel_id: &str, file: ChannelFile) -> Result<PathBuf, StoreError> {
        validate_id(channel_id)?;
        join_checked(&self.home, &[".letta", "channels", channel_id, file.name()])
    }

    /// Returns one project settings path only when the exact workspace is explicitly scoped.
    ///
    /// # Errors
    /// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
    pub fn project_file(&self, workspace: &Path, file: ProjectFile) -> Result<PathBuf, StoreError> {
        validate_root(workspace.to_path_buf())?;
        if !self.workspaces.iter().any(|root| root == workspace) {
            return Err(invalid(workspace));
        }
        join_checked(workspace, &[".letta", file.name()])
    }
}

fn validate_root(path: PathBuf) -> Result<PathBuf, StoreError> {
    let text = path.to_str().ok_or_else(|| invalid(&path))?;
    let depth = path.components().count();
    if !path.is_absolute()
        || text.len() > SIDE_PATH_BYTES_MAX
        || depth > SIDE_PATH_DEPTH_MAX
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid(&path));
    }
    Ok(path)
}

fn validate_id(value: &str) -> Result<(), StoreError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > SIDE_ID_BYTES_MAX
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', '\0'])
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err(invalid(path));
    }
    Ok(())
}

fn suffixed(value: &str, suffix: &str) -> Result<String, StoreError> {
    let capacity = value
        .len()
        .checked_add(suffix.len())
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, value))?;
    let mut output = String::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, value))?;
    output.push_str(value);
    output.push_str(suffix);
    Ok(output)
}

fn join_checked(root: &Path, components: &[&str]) -> Result<PathBuf, StoreError> {
    let mut path = root.to_path_buf();
    for component in components {
        path.push(component);
    }
    let text = path.to_str().ok_or_else(|| invalid(&path))?;
    if text.len() > SIDE_PATH_BYTES_MAX || path.components().count() > SIDE_PATH_DEPTH_MAX {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    Ok(path)
}

fn invalid(path: impl Into<PathBuf>) -> StoreError {
    StoreError::new(StoreErrorKind::InvalidPath, path)
}

#[cfg(test)]
mod evidence_impl {
    #[test]
    fn settings() {
        crate::side::evidence::paths::settings();
    }
    #[test]
    fn crons() {
        crate::side::evidence::paths::crons();
    }
    #[test]
    fn run() {
        crate::side::evidence::paths::run();
    }
    #[test]
    fn pending() {
        crate::side::evidence::paths::pending();
    }
    #[test]
    fn config() {
        crate::side::evidence::paths::config();
    }
    #[test]
    fn accounts() {
        crate::side::evidence::paths::accounts();
    }
    #[test]
    fn routing() {
        crate::side::evidence::paths::routing();
    }
    #[test]
    fn pairing() {
        crate::side::evidence::paths::pairing();
    }
    #[test]
    fn targets() {
        crate::side::evidence::paths::targets();
    }
    #[test]
    fn project_settings() {
        crate::side::evidence::paths::project_settings();
    }
    #[test]
    fn project_local() {
        crate::side::evidence::paths::project_local();
    }
    #[test]
    fn letta_home_override() {
        crate::side::evidence::paths::letta_home_override();
    }
}
