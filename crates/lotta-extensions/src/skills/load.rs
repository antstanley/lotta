//! Complete, bounded, non-interpreting skill loader.

use super::discovery::{check_path_bytes, display_path, read_bounded};
use super::frontmatter::{ParsedSkillDocument, parse_skill_document};
use super::{Skill, SkillError, limits};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One exact-byte companion file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillCompanion {
    /// Slash-normalized relative path.
    pub relative_path: String,
    /// Uninterpreted file bytes.
    pub bytes: Vec<u8>,
}

/// Complete loaded skill content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedSkill {
    /// Parsed metadata document retaining complete normalized instructions.
    pub document: ParsedSkillDocument,
    /// Deterministically sorted companion files.
    pub companions: Vec<SkillCompanion>,
}

/// Stateless complete loader.
pub struct SkillLoader;

impl SkillLoader {
    /// Rereads the selected entry and all regular companions without executing them.
    ///
    /// # Errors
    /// Fails closed for changed entries, escapes, special files, or resource limits.
    pub fn load(skill: &Skill) -> Result<LoadedSkill, SkillError> {
        check_selected_entry(skill)?;
        let canonical_directory =
            fs::canonicalize(skill.skill_file.parent().ok_or(SkillError::Malformed)?)
                .map_err(|_| SkillError::MissingEntry)?;
        let bytes = read_bounded(&skill.skill_file, limits::SKILL_DOCUMENT_BYTES_MAX)?;
        let relative = relative_directory(&skill.root, &skill.skill_file)?;
        let (document, id, _, _) = parse_skill_document(&bytes, &relative)?;
        if id != skill.id {
            return Err(SkillError::ChangedEntry);
        }
        let companions = load_companions(&canonical_directory, &skill.skill_file)?;
        Ok(LoadedSkill {
            document,
            companions,
        })
    }
}

fn check_selected_entry(skill: &Skill) -> Result<(), SkillError> {
    let root = fs::canonicalize(&skill.root).map_err(|_| SkillError::MissingEntry)?;
    let metadata = fs::symlink_metadata(&skill.skill_file).map_err(|_| SkillError::MissingEntry)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(SkillError::SpecialFile);
    }
    let file = fs::canonicalize(&skill.skill_file).map_err(|_| SkillError::MissingEntry)?;
    if file.starts_with(root) {
        Ok(())
    } else {
        Err(SkillError::Escape)
    }
}

fn load_companions(directory: &Path, skill_file: &Path) -> Result<Vec<SkillCompanion>, SkillError> {
    let canonical_skill = fs::canonicalize(skill_file).map_err(|_| SkillError::MissingEntry)?;
    let mut pending = Vec::new();
    bounded_push(
        &mut pending,
        (directory.to_owned(), 0usize),
        limits::SKILL_LOAD_DIRECTORIES_ITEMS_MAX,
    )?;
    let mut visited = BTreeSet::new();
    let mut paths = Vec::new();
    let mut entries_seen = 0usize;
    while let Some((current, depth)) = pending.pop() {
        if depth > limits::SKILL_LOAD_DEPTH_MAX {
            return Err(SkillError::LimitExceeded);
        }
        let canonical = fs::canonicalize(&current).map_err(|_| SkillError::Infrastructure)?;
        if !canonical.starts_with(directory) {
            return Err(SkillError::Escape);
        }
        if visited.contains(&canonical) {
            continue;
        }
        if visited.len() >= limits::SKILL_LOAD_DIRECTORIES_ITEMS_MAX {
            return Err(SkillError::LimitExceeded);
        }
        visited.insert(canonical);
        let mut entries = Vec::new();
        for entry in fs::read_dir(&current).map_err(|_| SkillError::Infrastructure)? {
            check_entry_limit(&mut entries_seen, limits::SKILL_LOAD_ENTRIES_ITEMS_MAX)?;
            entries
                .try_reserve(1)
                .map_err(|_| SkillError::LimitExceeded)?;
            entries.push(entry.map_err(|_| SkillError::Infrastructure)?);
        }
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            check_path_bytes(&path)?;
            let lexical = fs::symlink_metadata(&path).map_err(|_| SkillError::Infrastructure)?;
            if lexical.file_type().is_symlink() {
                return Err(SkillError::SpecialFile);
            }
            let canonical = fs::canonicalize(&path).map_err(|_| SkillError::Infrastructure)?;
            if !canonical.starts_with(directory) {
                return Err(SkillError::Escape);
            }
            if lexical.is_dir() {
                bounded_push(
                    &mut pending,
                    (path, depth + 1),
                    limits::SKILL_LOAD_DIRECTORIES_ITEMS_MAX,
                )?;
            } else if lexical.is_file() && canonical != canonical_skill {
                bounded_push(&mut paths, path, limits::SKILL_COMPANION_FILES_ITEMS_MAX)?;
            } else if !lexical.is_file() {
                return Err(SkillError::SpecialFile);
            }
        }
    }
    paths.sort_by_key(|path| display_path(path.strip_prefix(directory).unwrap_or(path)));
    read_companions(directory, paths)
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

fn read_companions(
    directory: &Path,
    paths: Vec<PathBuf>,
) -> Result<Vec<SkillCompanion>, SkillError> {
    let mut aggregate = 0usize;
    let mut output = Vec::new();
    output
        .try_reserve(paths.len())
        .map_err(|_| SkillError::LimitExceeded)?;
    for path in paths {
        let bytes = read_bounded(&path, limits::SKILL_COMPANION_FILE_BYTES_MAX)?;
        aggregate = aggregate
            .checked_add(bytes.len())
            .ok_or(SkillError::LimitExceeded)?;
        if aggregate > limits::SKILL_COMPANIONS_AGGREGATE_BYTES_MAX {
            return Err(SkillError::LimitExceeded);
        }
        let relative = path
            .strip_prefix(directory)
            .map_err(|_| SkillError::Escape)?;
        output.push(SkillCompanion {
            relative_path: display_path(relative),
            bytes,
        });
    }
    Ok(output)
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
        parts.push(
            part.as_os_str()
                .to_str()
                .ok_or(SkillError::Malformed)?
                .to_owned(),
        );
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::check_entry_limit;
    use crate::skills::SkillError;

    #[test]
    fn load_entry_limit_below_at_above() {
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
