use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) const INCLUDE_FILE_BYTES_MAX: usize = 64 * 1024;
const INCLUDE_ENTRIES_MAX: usize = 256;
const SETTINGS_BYTES_MAX: usize = 1024 * 1024;
const PROVISION_FILE_BYTES_MAX: u64 = 16 * 1024 * 1024;
const PROVISION_TOTAL_BYTES_MAX: u64 = 128 * 1024 * 1024;
const PROVISION_DEPTH_MAX: usize = 32;
const PROVISION_FILES_MAX: usize = 4096;

#[derive(Default, Serialize)]
pub(super) struct Report {
    done: Vec<String>,
    skipped: Vec<String>,
    errors: Vec<String>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Config {
    copy_local_settings: bool,
    link_hooks: bool,
    include: Vec<String>,
    symlink_directories: Vec<String>,
}
struct Budget {
    bytes: u64,
    files: usize,
}

pub(super) fn provision(
    primary: &Path,
    worktree: &Path,
    input: &Value,
    hooks_path: Option<&Path>,
) -> Report {
    let mut report = Report::default();
    let config = load_config(primary, &mut report);
    if config.copy_local_settings {
        apply(&mut report, "settings", || {
            copy_file(
                primary,
                worktree,
                Path::new(".letta/settings.local.json"),
                &mut Budget { bytes: 0, files: 0 },
            )
        });
    }
    let mut includes = config.include;
    includes.extend(read_includes(primary, &mut report));
    let mut budget = Budget { bytes: 0, files: 0 };
    for value in includes {
        let label = format!("include:{value}");
        apply(&mut report, &label, || {
            copy_entry(primary, worktree, &safe_relative(&value)?, 0, &mut budget)
        });
    }
    if config.link_hooks
        && let Some(relative) = hooks_path
    {
        apply(&mut report, "hooks", || {
            link_primary_hooks(primary, worktree, relative)
        });
    }
    let dependencies = input
        .get("symlink_dependencies")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if dependencies {
        let dirs = if config.symlink_directories.is_empty() {
            vec!["node_modules".into()]
        } else {
            config.symlink_directories
        };
        for value in dirs {
            let label = format!("dependency:{value}");
            apply(&mut report, &label, || {
                link_directory(primary, worktree, &safe_relative(&value)?)
            });
        }
    }
    report
}

fn load_config(primary: &Path, report: &mut Report) -> Config {
    let path = primary.join(".letta/settings.json");
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return Config::default();
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > SETTINGS_BYTES_MAX as u64
    {
        report.errors.push("settings-config".into());
        return Config::default();
    }
    let Ok(bytes) = fs::read(path) else {
        report.errors.push("settings-config".into());
        return Config::default();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        report.errors.push("settings-config".into());
        return Config::default();
    };
    serde_json::from_value(
        value
            .get("worktree")
            .cloned()
            .unwrap_or(Value::Object(Map::default())),
    )
    .unwrap_or_else(|_| {
        report.errors.push("settings-config".into());
        Config::default()
    })
}
fn read_includes(primary: &Path, report: &mut Report) -> Vec<String> {
    let path = primary.join(".worktreeinclude");
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return Vec::new();
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > INCLUDE_FILE_BYTES_MAX as u64
    {
        report.errors.push("include-file".into());
        return Vec::new();
    }
    let Ok(bytes) = fs::read(path) else {
        report.errors.push("include-file".into());
        return Vec::new();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        report.errors.push("include-file".into());
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .take(INCLUDE_ENTRIES_MAX)
        .map(str::to_owned)
        .collect()
}
fn apply<F>(report: &mut Report, label: &str, operation: F)
where
    F: FnOnce() -> Result<(), ()>,
{
    match operation() {
        Ok(()) => report.done.push(label.into()),
        Err(()) => report.skipped.push(label.into()),
    }
}
fn copy_entry(
    primary: &Path,
    worktree: &Path,
    relative: &Path,
    depth: usize,
    budget: &mut Budget,
) -> Result<(), ()> {
    if depth > PROVISION_DEPTH_MAX {
        return Err(());
    }
    let source = primary.join(relative);
    let destination = worktree.join(relative);
    safe_parents(primary, &source)?;
    safe_destination(worktree, &destination)?;
    let identity = fs::symlink_metadata(&source).map_err(|_| ())?;
    if identity.file_type().is_symlink() {
        return Err(());
    }
    if identity.is_file() {
        return copy_file(primary, worktree, relative, budget);
    }
    if !identity.is_dir() {
        return Err(());
    }
    fs::create_dir(&destination)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|_| ())?;
    for entry in fs::read_dir(&source).map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        copy_entry(
            primary,
            worktree,
            &relative.join(entry.file_name()),
            depth + 1,
            budget,
        )?;
    }
    Ok(())
}
fn copy_file(
    primary: &Path,
    worktree: &Path,
    relative: &Path,
    budget: &mut Budget,
) -> Result<(), ()> {
    let source = primary.join(relative);
    let destination = worktree.join(relative);
    safe_parents(primary, &source)?;
    safe_destination(worktree, &destination)?;
    let before = regular(&source)?;
    if before.len() > PROVISION_FILE_BYTES_MAX
        || budget.bytes.saturating_add(before.len()) > PROVISION_TOTAL_BYTES_MAX
        || budget.files >= PROVISION_FILES_MAX
    {
        return Err(());
    }
    if let Some(parent) = destination.parent() {
        create_parents_no_symlink(worktree, parent)?;
    }
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(());
    }
    fs::copy(&source, &destination).map_err(|_| ())?;
    let after = regular(&source)?;
    if before.len() != after.len() {
        let _ = fs::remove_file(destination);
        return Err(());
    }
    budget.bytes += before.len();
    budget.files += 1;
    Ok(())
}
fn link_primary_hooks(primary: &Path, worktree: &Path, relative: &Path) -> Result<(), ()> {
    let hooks = primary.join(relative);
    safe_parents(primary, &hooks)?;
    let metadata = fs::symlink_metadata(&hooks).map_err(|_| ())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || fs::read_dir(&hooks).map_err(|_| ())?.next().is_none()
    {
        return Err(());
    }
    let destination = worktree.join(relative);
    if fs::symlink_metadata(&destination).is_ok() {
        return Ok(());
    }
    link_absolute(
        &hooks.canonicalize().map_err(|_| ())?,
        &destination,
        worktree,
    )
}

pub(super) fn safe_hooks_path(value: &str) -> Option<PathBuf> {
    let path = safe_relative(value).ok()?;
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}
fn link_directory(primary: &Path, worktree: &Path, relative: &Path) -> Result<(), ()> {
    let source = primary.join(relative);
    safe_parents(primary, &source)?;
    if !fs::symlink_metadata(&source).map_err(|_| ())?.is_dir() {
        return Err(());
    }
    link_absolute(
        &source.canonicalize().map_err(|_| ())?,
        &worktree.join(relative),
        worktree,
    )
}
fn link_absolute(source: &Path, destination: &Path, worktree: &Path) -> Result<(), ()> {
    safe_destination(worktree, destination)?;
    if let Some(parent) = destination.parent() {
        create_parents_no_symlink(worktree, parent)?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination).map_err(|_| ())
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(source, destination).map_err(|_| ())
    }
}
fn safe_relative(value: &str) -> Result<PathBuf, ()> {
    let path = PathBuf::from(value);
    if value.contains('\0')
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        Err(())
    } else {
        Ok(path)
    }
}
fn safe_parents(root: &Path, path: &Path) -> Result<(), ()> {
    let relative = path.strip_prefix(root).map_err(|_| ())?;
    let mut current = root.to_owned();
    for component in relative.components() {
        current.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&current)
            && metadata.file_type().is_symlink()
        {
            return Err(());
        }
    }
    Ok(())
}
fn safe_destination(root: &Path, path: &Path) -> Result<(), ()> {
    if !path.starts_with(root) {
        return Err(());
    }
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || metadata.is_file())
    {
        return Err(());
    }
    safe_parents(root, path)
}
fn create_parents_no_symlink(root: &Path, parent: &Path) -> Result<(), ()> {
    let relative = parent.strip_prefix(root).map_err(|_| ())?;
    let mut current = root.to_owned();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|_| ())?;
            }
            Ok(_) | Err(_) => return Err(()),
        }
    }
    Ok(())
}
fn regular(path: &Path) -> Result<fs::Metadata, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(());
        }
    }
    Ok(metadata)
}
