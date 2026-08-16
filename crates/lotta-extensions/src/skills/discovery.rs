//! Explicit-root, deterministic skill discovery.

use super::frontmatter::parse_skill_document;
use super::{Skill, SkillError, SkillSource, SkillSources, limits};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Explicit authorities used for skill discovery. Core discovery performs no ambient reads.
#[derive(Clone, Debug)]
pub struct SkillRoots {
    /// Project working root; `.agents/skills` is appended.
    pub project_working_root: PathBuf,
    /// Already-resolved canonical agent skills directory.
    pub agent_skills_directory: Option<PathBuf>,
    /// Optional memory root; `skills` is appended only as agent fallback.
    pub memory_root: Option<PathBuf>,
    /// Explicit global skills directory.
    pub global_skills_directory: PathBuf,
    /// Explicit bundled skills directory.
    pub bundled_skills_directory: PathBuf,
}

/// Stateless bounded skill catalog discovery.
pub struct SkillDiscovery;

impl SkillDiscovery {
    /// Discovers, parses, merges by canonical precedence, and sorts by ID.
    ///
    /// # Errors
    /// Returns fixed typed errors for malformed, special, duplicate, or overbound inputs.
    pub fn discover(
        roots: &SkillRoots,
        sources: impl Into<SkillSources>,
    ) -> Result<Vec<Skill>, SkillError> {
        let sources = sources.into();
        let mut merged = BTreeMap::new();
        for source in SkillSource::PRECEDENCE {
            if !sources.contains(source) {
                continue;
            }
            let Some(root) = selected_root(roots, source)? else {
                continue;
            };
            for skill in discover_root(&root, source)? {
                merged.entry(skill.id.clone()).or_insert(skill);
            }
        }
        Ok(merged.into_values().collect())
    }

    /// Selects exact runtime IDs from an already precedence-merged catalog.
    ///
    /// # Errors
    /// Missing or duplicate selected IDs return fixed errors.
    pub fn select(
        catalog: &[Skill],
        ids: &[String],
    ) -> Result<Vec<super::SelectedSkill>, SkillError> {
        if ids.len() > limits::SKILLS_SELECTED_ITEMS_MAX {
            return Err(SkillError::LimitExceeded);
        }
        let mut requested = BTreeSet::new();
        for id in ids {
            if !requested.insert(id.as_str()) {
                return Err(SkillError::DuplicateSelection);
            }
        }
        let by_id: BTreeMap<&str, &Skill> = catalog
            .iter()
            .map(|skill| (skill.id.as_str(), skill))
            .collect();
        let mut selected = Vec::new();
        selected
            .try_reserve(ids.len())
            .map_err(|_| SkillError::LimitExceeded)?;
        for id in requested {
            let skill = by_id.get(id).ok_or(SkillError::MissingSelection)?;
            selected.push(super::SelectedSkill {
                id: skill.id.clone(),
                name: skill.name.clone(),
                description: skill.description.clone(),
                location: display_path(&skill.skill_file),
            });
        }
        Ok(selected)
    }
}

fn selected_root(roots: &SkillRoots, source: SkillSource) -> Result<Option<PathBuf>, SkillError> {
    let candidate = match source {
        SkillSource::Project => {
            let canonical = roots.project_working_root.join(".agents/skills");
            if path_exists(&canonical)? {
                canonical
            } else {
                roots.project_working_root.join(".skills")
            }
        }
        SkillSource::Agent => {
            if let Some(canonical) = roots.agent_skills_directory.as_ref()
                && path_exists(canonical)?
            {
                canonical.clone()
            } else if let Some(memory) = roots.memory_root.as_ref() {
                memory.join("skills")
            } else {
                return Ok(None);
            }
        }
        SkillSource::Global => roots.global_skills_directory.clone(),
        SkillSource::Bundled => roots.bundled_skills_directory.clone(),
    };
    Ok(path_exists(&candidate)?.then_some(candidate))
}

fn path_exists(path: &Path) -> Result<bool, SkillError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                let target = fs::metadata(path).map_err(|_| SkillError::Malformed)?;
                if !target.is_dir() {
                    return Err(SkillError::SpecialFile);
                }
            } else if !metadata.is_dir() {
                return Err(SkillError::SpecialFile);
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(SkillError::Infrastructure),
    }
}

fn discover_root(root: &Path, source: SkillSource) -> Result<Vec<Skill>, SkillError> {
    let canonical_root = fs::canonicalize(root).map_err(|_| SkillError::Infrastructure)?;
    let files = skill_files(root)?;
    let mut by_id = BTreeMap::new();
    for file in files {
        let canonical_file = fs::canonicalize(&file).map_err(|_| SkillError::Infrastructure)?;
        if !canonical_file.starts_with(&canonical_root) {
            return Err(SkillError::Escape);
        }
        let relative_dir = relative_directory(root, &file)?;
        let bytes = read_bounded(&file, limits::SKILL_DOCUMENT_BYTES_MAX)?;
        let (_, id, name, description) = parse_skill_document(&bytes, &relative_dir)?;
        let skill = Skill {
            id: id.clone(),
            name,
            description,
            source,
            root: root.to_owned(),
            skill_file: file,
        };
        if by_id.insert(id, skill).is_some() {
            return Err(SkillError::DuplicateSkill);
        }
    }
    Ok(by_id.into_values().collect())
}

fn skill_files(root: &Path) -> Result<Vec<PathBuf>, SkillError> {
    let mut pending = Vec::new();
    bounded_push(
        &mut pending,
        (root.to_owned(), 0usize),
        limits::SKILL_DIRECTORIES_ITEMS_MAX,
    )?;
    let mut visited = BTreeSet::new();
    let mut output = Vec::new();
    let mut entries_seen = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        if depth > limits::SKILL_DISCOVERY_DEPTH_MAX {
            return Err(SkillError::LimitExceeded);
        }
        let canonical = fs::canonicalize(&directory).map_err(|_| SkillError::Infrastructure)?;
        if visited.contains(&canonical) {
            continue;
        }
        if visited.len() >= limits::SKILL_DIRECTORIES_ITEMS_MAX {
            return Err(SkillError::LimitExceeded);
        }
        visited.insert(canonical);
        let mut entries = Vec::new();
        for entry in fs::read_dir(&directory).map_err(|_| SkillError::Infrastructure)? {
            check_entry_limit(&mut entries_seen, limits::SKILL_DISCOVERY_ENTRIES_ITEMS_MAX)?;
            entries
                .try_reserve(1)
                .map_err(|_| SkillError::LimitExceeded)?;
            entries.push(entry.map_err(|_| SkillError::Infrastructure)?);
        }
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            check_path_bytes(&path)?;
            let metadata = fs::metadata(&path).map_err(|_| SkillError::Infrastructure)?;
            if metadata.is_dir() {
                bounded_push(
                    &mut pending,
                    (path, depth + 1),
                    limits::SKILL_DIRECTORIES_ITEMS_MAX,
                )?;
            } else if metadata.is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("SKILL.md")
            {
                bounded_push(&mut output, path, limits::SKILLS_DISCOVERED_ITEMS_MAX)?;
            } else if entry
                .file_type()
                .map_err(|_| SkillError::Infrastructure)?
                .is_symlink()
            {
                return Err(SkillError::SpecialFile);
            }
        }
    }
    output.sort_by_key(|path| display_path(path));
    Ok(output)
}

fn check_entry_limit(seen: &mut usize, limit: usize) -> Result<(), SkillError> {
    let next = seen.checked_add(1).ok_or(SkillError::LimitExceeded)?;
    if next > limit {
        return Err(SkillError::LimitExceeded);
    }
    *seen = next;
    Ok(())
}

fn bounded_push<T>(values: &mut Vec<T>, value: T, limit: usize) -> Result<(), SkillError> {
    if values.len() >= limit {
        return Err(SkillError::LimitExceeded);
    }
    values
        .try_reserve(1)
        .map_err(|_| SkillError::LimitExceeded)?;
    values.push(value);
    Ok(())
}

fn relative_directory(root: &Path, file: &Path) -> Result<String, SkillError> {
    let parent = file.parent().ok_or(SkillError::Malformed)?;
    let relative = parent.strip_prefix(root).map_err(|_| SkillError::Escape)?;
    if relative.as_os_str().is_empty() {
        return Ok("root".into());
    }
    let components = relative.components();
    let mut parts = Vec::new();
    parts
        .try_reserve(components.clone().count())
        .map_err(|_| SkillError::LimitExceeded)?;
    for part in components {
        let value = part.as_os_str().to_str().ok_or(SkillError::Malformed)?;
        parts.push(value);
    }
    Ok(parts.join("/"))
}

pub(crate) fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, SkillError> {
    let metadata = fs::metadata(path).map_err(|_| SkillError::MissingEntry)?;
    if !metadata.is_file() {
        return Err(SkillError::SpecialFile);
    }
    let length = usize::try_from(metadata.len()).map_err(|_| SkillError::LimitExceeded)?;
    if length > limit {
        return Err(SkillError::LimitExceeded);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve(length)
        .map_err(|_| SkillError::LimitExceeded)?;
    fs::File::open(path)
        .map_err(|_| SkillError::MissingEntry)?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| SkillError::Infrastructure)?;
    if bytes.len() > limit {
        Err(SkillError::LimitExceeded)
    } else {
        Ok(bytes)
    }
}

pub(crate) fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn check_path_bytes(path: &Path) -> Result<(), SkillError> {
    if display_path(path).len() > limits::SKILL_PATH_BYTES_MAX {
        Err(SkillError::LimitExceeded)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::check_entry_limit;
    use crate::skills::SkillError;

    #[test]
    fn discovery_entry_limit_below_at_above() {
        let mut seen = 0;
        assert_eq!(check_entry_limit(&mut seen, 2), Ok(()));
        assert_eq!(seen, 1);
        assert_eq!(check_entry_limit(&mut seen, 2), Ok(()));
        assert_eq!(seen, 2);
        assert_eq!(
            check_entry_limit(&mut seen, 2),
            Err(SkillError::LimitExceeded)
        );
        assert_eq!(seen, 2);
    }
}
