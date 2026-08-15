//! Baseline-compatible initial-memory label and frontmatter helpers.

use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{InitialMemoryBlock, RepositoryPath};
use std::path::PathBuf;

fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}

/// Normalizes one raw label to its repository-relative Markdown path.
///
/// The ordering intentionally matches `local-backend.ts`: trim, slash normalization, removal of
/// one trailing `.md`, system-prefix handling, empty-segment filtering, then dot rejection.
///
/// # Errors
/// Returns a stable invalid-data error when a retained segment is `.` or `..`.
pub fn normalize_label(raw: &str) -> Result<RepositoryPath, RuntimeError> {
    let slash = raw.trim().replace('\\', "/");
    let stripped = match slash.strip_suffix(".md") {
        Some(value) => value,
        None => &slash,
    };
    let system = stripped == "system" || stripped.starts_with("system/");
    let mut normalized = String::new();
    for segment in stripped.split('/').filter(|part| !part.is_empty()) {
        if segment == "." || segment == ".." {
            return Err(invalid("memory label"));
        }
        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized.push_str(segment);
    }
    let relative = if stripped == "system" {
        "system.md".to_owned()
    } else if system && stripped.ends_with('/') {
        format!("{normalized}/.md")
    } else if system {
        format!("{normalized}.md")
    } else if normalized.is_empty() {
        "system/.md".to_owned()
    } else {
        format!("system/{normalized}.md")
    };
    RepositoryPath::new(PathBuf::from(relative))
}

/// Renders one initial block with exact LF frontmatter and a non-empty description.
///
/// # Errors
/// Returns a stable limit error if frontmatter plus value exceeds the canonical file ceiling.
pub fn render_block(block: &InitialMemoryBlock) -> Result<Vec<u8>, RuntimeError> {
    let raw_label = block.label().as_str();
    let description = block
        .description()
        .map(|value| value.as_str().trim())
        .filter(|value| !value.is_empty())
        .map_or_else(|| format!("Memory block {raw_label}"), String::from);
    let sanitized = description.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let scalar =
        serde_json::to_string(sanitized.trim()).map_err(|_| RuntimeError::InvalidData {
            context: "memory description".into(),
        })?;
    let prefix = format!("---\ndescription: {scalar}\n---\n");
    let total = prefix
        .len()
        .checked_add(block.value().as_slice().len())
        .ok_or_else(|| RuntimeError::LimitExceeded {
            context: crate::MEMORY_FILE_BYTES_MAX.name.into(),
        })?;
    if total > crate::MEMORY_FILE_BYTES_MAX.value {
        return Err(RuntimeError::LimitExceeded {
            context: crate::MEMORY_FILE_BYTES_MAX.name.into(),
        });
    }
    let mut rendered = Vec::new();
    rendered
        .try_reserve(total)
        .map_err(|_| RuntimeError::LimitExceeded {
            context: crate::MEMORY_FILE_BYTES_MAX.name.into(),
        })?;
    rendered.extend_from_slice(prefix.as_bytes());
    rendered.extend_from_slice(block.value().as_slice());
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(value: &str) -> String {
        normalize_label(value)
            .expect("normalization")
            .as_path()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn preserves_system_prefix() {
        assert_eq!(normalized("system"), "system.md");
        assert_eq!(normalized("system/"), "system/.md");
        assert_eq!(normalized("system/notes"), "system/notes.md");
    }

    #[test]
    fn normalizes_backslashes() {
        assert_eq!(normalized("notes\\daily.md"), "system/notes/daily.md");
    }

    #[test]
    fn removes_one_trailing_markdown_suffix() {
        assert_eq!(normalized(" notes.md "), "system/notes.md");
        assert_eq!(normalized("notes.md.md"), "system/notes.md.md");
    }

    #[test]
    fn collapses_empty_label() {
        assert_eq!(normalized(""), "system/.md");
    }

    #[test]
    fn collapses_absolute_looking_labels() {
        assert_eq!(normalized("/notes//daily"), "system/notes/daily.md");
        assert_eq!(normalized("C:\\notes\\daily"), "system/C:/notes/daily.md");
    }

    #[test]
    fn rejects_dot_segments() {
        for value in [".", "..", "system/.", "system/..", "a/./b", "a/../b"] {
            assert!(normalize_label(value).is_err(), "accepted {value}");
        }
    }
}
