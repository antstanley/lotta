//! Staged Markdown and protected-frontmatter validation.

use crate::{fs, git};
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::RepositoryPath;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default, Eq, PartialEq)]
struct Frontmatter {
    description: Option<String>,
    read_only: Option<String>,
}

fn invalid() -> RuntimeError {
    RuntimeError::InvalidData {
        context: "memory Markdown frontmatter".into(),
    }
}

fn description(value: &str) -> Result<String, RuntimeError> {
    let value = value.trim();
    if value.starts_with('"') {
        let decoded: String = serde_json::from_str(value).map_err(|_| invalid())?;
        if decoded.trim().is_empty() {
            return Err(invalid());
        }
        return Ok(decoded);
    }
    if value.is_empty() {
        return Err(invalid());
    }
    Ok(value.to_owned())
}

fn set_once(slot: &mut Option<String>, value: String) -> Result<(), RuntimeError> {
    if slot.is_some() {
        return Err(invalid());
    }
    *slot = Some(value);
    Ok(())
}

fn parse_frontmatter(bytes: &[u8]) -> Result<Frontmatter, RuntimeError> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Err(invalid());
    }
    let mut frontmatter = Frontmatter::default();
    let mut limit = false;
    let mut closed = false;
    for line in lines {
        if line == "---" {
            closed = true;
            break;
        }
        if line.is_empty() || line.starts_with([' ', '\t']) {
            continue;
        }
        let (key, value) = line.split_once(':').ok_or_else(invalid)?;
        match key.trim() {
            "description" => {
                let value = description(value)?;
                set_once(&mut frontmatter.description, value)?;
            }
            "read_only" => set_once(&mut frontmatter.read_only, value.trim().to_owned())?,
            "limit" if !limit => limit = true,
            _ => return Err(invalid()),
        }
    }
    if !closed || frontmatter.description.is_none() {
        return Err(invalid());
    }
    Ok(frontmatter)
}

fn revision_exists(repo: &Path, revision: &str) -> Result<bool, RuntimeError> {
    Ok(
        git::run(repo, ["cat-file", "-e", revision], "git revision")?
            .status
            .success(),
    )
}

fn push_staged(
    result: &mut Vec<(char, RepositoryPath)>,
    status: &[u8],
    path: &[u8],
) -> Result<(), RuntimeError> {
    let status = status.first().copied().ok_or_else(invalid)? as char;
    let path = std::str::from_utf8(path).map_err(|_| invalid())?;
    let path = RepositoryPath::new(path.into())?;
    fs::validate_repository_path(&path)?;
    result
        .try_reserve(1)
        .map_err(|_| RuntimeError::LimitExceeded {
            context: crate::MEMORY_FILES_MAX.name.into(),
        })?;
    result.push((status, path));
    fs::validate_file_count(result.len())
}

fn staged_paths(repo: &Path) -> Result<Vec<(char, RepositoryPath)>, RuntimeError> {
    let output = git::checked(
        repo,
        ["diff", "--cached", "--name-status", "-z", "--no-renames"],
        "git staged files",
    )?;
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());
    let mut result = Vec::new();
    while let Some(status) = fields.next() {
        let path = fields.next().ok_or_else(invalid)?;
        push_staged(&mut result, status, path)?;
    }
    Ok(result)
}

fn git_blob(repo: &Path, spec: &str) -> Result<Option<Vec<u8>>, RuntimeError> {
    let output = git::run(repo, ["show", "--no-textconv", spec], "git blob")?;
    if output.status.success() {
        fs::validate_file_bytes(output.stdout.len())?;
        Ok(Some(output.stdout))
    } else {
        Ok(None)
    }
}

fn relevant(path: &RepositoryPath) -> bool {
    let value = path.as_path().to_string_lossy();
    (value.starts_with("system/") || value.starts_with("reference/")) && value.ends_with(".md")
}

fn flat_skill(path: &RepositoryPath) -> bool {
    let mut parts = path.as_path().components();
    parts
        .next()
        .is_some_and(|part| part.as_os_str() == "skills")
        && parts.next().is_some()
        && parts.next().is_none()
        && path
            .as_path()
            .extension()
            .is_some_and(|value| value == "md")
}

pub(crate) fn validate_staged(repo: &Path) -> Result<(), RuntimeError> {
    let has_head = revision_exists(repo, "HEAD")?;
    let paths = staged_paths(repo)?;
    let mut parsed = BTreeMap::new();
    for (status, path) in paths {
        if flat_skill(&path) && status != 'D' {
            return Err(invalid());
        }
        if relevant(&path) {
            validate_staged_path(repo, has_head, status, &path, &mut parsed)?;
        }
    }
    Ok(())
}

fn validate_staged_path(
    repo: &Path,
    has_head: bool,
    status: char,
    path: &RepositoryPath,
    parsed: &mut BTreeMap<RepositoryPath, Frontmatter>,
) -> Result<(), RuntimeError> {
    let name = path.as_path().to_str().ok_or_else(invalid)?;
    let head_bytes = if has_head {
        git_blob(repo, &format!("HEAD:{name}"))?
    } else {
        None
    };
    let head = head_bytes.as_deref().map(parse_frontmatter).transpose()?;
    if status == 'D' {
        if head.as_ref().and_then(|value| value.read_only.as_deref()) == Some("true") {
            return Err(invalid());
        }
        return Ok(());
    }
    let staged_bytes = git_blob(repo, &format!(":{name}"))?.ok_or_else(invalid)?;
    let staged = parse_frontmatter(&staged_bytes)?;
    if head.is_none() && staged.read_only.is_some() {
        return Err(invalid());
    }
    if staged.read_only != head.as_ref().and_then(|value| value.read_only.clone()) {
        return Err(invalid());
    }
    if head.as_ref().and_then(|value| value.read_only.as_deref()) == Some("true")
        && head_bytes.as_deref() != Some(staged_bytes.as_slice())
    {
        return Err(invalid());
    }
    parsed.insert(path.clone(), staged);
    Ok(())
}
