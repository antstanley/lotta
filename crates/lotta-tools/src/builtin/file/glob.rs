use super::{
    FileState,
    control::OperationControl,
    fs::{FileError, file_type},
    operations::{
        DIRECTORY_DEPTH_MAX, DIRECTORY_ENTRIES_MAX, PATH_BYTES_MAX, PATH_COMPONENTS_MAX,
        RESULTS_MAX,
    },
};
use cap_std::fs::Dir;
use regex::{Regex, RegexBuilder};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

const PATTERN_BYTES_MAX: usize = 4_096;
const ALTERNATIVES_MAX: usize = 32;
const COMPILED_BYTES_MAX: usize = 256 * 1024;

pub(super) struct GlobMatcher(Regex);

impl GlobMatcher {
    pub(super) fn compile(pattern: &str, case_sensitive: bool) -> Result<Self, FileError> {
        validate_pattern(pattern)?;
        let alternatives = expand_braces(pattern)?;
        let mut source = String::from("^(?:");
        for (index, alternative) in alternatives.iter().enumerate() {
            if index != 0 {
                source.push('|');
            }
            translate(alternative, &mut source)?;
        }
        source.push_str(")$");
        validate_compiled_source(source.len())?;
        let regex = RegexBuilder::new(&source)
            .case_insensitive(!case_sensitive)
            .size_limit(COMPILED_BYTES_MAX)
            .build()
            .map_err(|_| FileError::Tool)?;
        Ok(Self(regex))
    }

    pub(super) fn is_match(&self, value: &str) -> bool {
        self.0.is_match(value)
    }
}

pub(super) fn matching_files(
    state: &FileState,
    base: &Path,
    pattern: &str,
    case_sensitive: bool,
    control: &OperationControl,
) -> Result<Vec<PathBuf>, FileError> {
    let matcher = GlobMatcher::compile(pattern, case_sensitive)?;
    let files = workspace_files(state, base, control)?;
    let mut output = Vec::new();
    for child in files {
        control.check()?;
        let relative = child.strip_prefix(base).unwrap_or(&child);
        if matcher.is_match(&slash(relative)) {
            validate_results(output.len().checked_add(1).ok_or(FileError::Tool)?)?;
            output.try_reserve(1).map_err(|_| FileError::Tool)?;
            output.push(child);
        }
    }
    Ok(output)
}

pub(super) fn workspace_files(
    state: &FileState,
    base: &Path,
    control: &OperationControl,
) -> Result<Vec<PathBuf>, FileError> {
    let base = if base == Path::new(".") {
        Path::new("")
    } else {
        base
    };
    let mut output = Vec::new();
    let mut queue = VecDeque::new();
    queue.try_reserve(1).map_err(|_| FileError::Tool)?;
    queue.push_back((base.to_owned(), 0usize));
    let mut entries = 0usize;
    let mut visited = 0usize;
    while let Some((directory, depth)) = queue.pop_front() {
        control.check()?;
        visited = visited.checked_add(1).ok_or(FileError::Tool)?;
        validate_depth(depth)?;
        validate_entries(visited)?;
        let mut children = read_children(&state.workspace, &directory, &mut entries, control)?;
        children.sort();
        for child in children {
            control.check()?;
            let kind = file_type(&state.workspace, &child)?;
            if kind.is_dir() {
                if queue.len() >= DIRECTORY_ENTRIES_MAX {
                    return Err(FileError::Tool);
                }
                queue.try_reserve(1).map_err(|_| FileError::Tool)?;
                queue.push_back((child, depth.checked_add(1).ok_or(FileError::Tool)?));
            } else if kind.is_file() {
                validate_entries(output.len().checked_add(1).ok_or(FileError::Tool)?)?;
                output.try_reserve(1).map_err(|_| FileError::Tool)?;
                output.push(child);
            }
        }
    }
    output.sort();
    Ok(output)
}

fn read_children(
    dir: &Dir,
    path: &Path,
    aggregate: &mut usize,
    control: &OperationControl,
) -> Result<Vec<PathBuf>, FileError> {
    let mut output = Vec::new();
    let read_path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    for entry in dir.read_dir(read_path).map_err(|_| FileError::Tool)? {
        control.check()?;
        let entry = entry.map_err(|_| FileError::Tool)?;
        *aggregate = aggregate.checked_add(1).ok_or(FileError::Tool)?;
        validate_entries(*aggregate)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(FileError::Tool)?;
        if name.len() > PATH_BYTES_MAX {
            return Err(FileError::Tool);
        }
        let child = path.join(name);
        if child.components().count() > PATH_COMPONENTS_MAX || slash(&child).len() > PATH_BYTES_MAX
        {
            return Err(FileError::Tool);
        }
        output.try_reserve(1).map_err(|_| FileError::Tool)?;
        output.push(child);
    }
    Ok(output)
}

#[cfg(test)]
pub(super) fn glob_matches(
    pattern: &str,
    value: &str,
    case_sensitive: bool,
) -> Result<bool, FileError> {
    Ok(GlobMatcher::compile(pattern, case_sensitive)?.is_match(value))
}

fn translate(pattern: &str, output: &mut String) -> Result<(), FileError> {
    let mut chars = pattern.chars().peekable();
    while let Some(current) = chars.next() {
        match current {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    output.push_str("(?:.*/)?");
                } else {
                    output.push_str(".*");
                }
            }
            '*' => output.push_str("[^/]*"),
            '?' => output.push_str("[^/]"),
            '\\' => push_literal(chars.next().ok_or(FileError::Tool)?, output),
            literal => push_literal(literal, output),
        }
        validate_compiled_source(output.len())?;
    }
    Ok(())
}

fn push_literal(value: char, output: &mut String) {
    output.push_str(&regex::escape(&value.to_string()));
}

fn expand_braces(pattern: &str) -> Result<Vec<String>, FileError> {
    let Some(open) = pattern.find('{') else {
        return Ok(vec![pattern.to_owned()]);
    };
    if pattern[open + 1..].contains('{') {
        return Err(FileError::Tool);
    }
    let close = pattern[open + 1..]
        .find('}')
        .map(|value| value + open + 1)
        .ok_or(FileError::Tool)?;
    if pattern[close + 1..].contains('}') {
        return Err(FileError::Tool);
    }
    let body = &pattern[open + 1..close];
    let count = body.split(',').count();
    if count == 0 || count > ALTERNATIVES_MAX {
        return Err(FileError::Tool);
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|_| FileError::Tool)?;
    for alternative in body.split(',') {
        if alternative.is_empty() {
            return Err(FileError::Tool);
        }
        let value = format!(
            "{}{}{}",
            &pattern[..open],
            alternative,
            &pattern[close + 1..]
        );
        if value.len() > PATTERN_BYTES_MAX {
            return Err(FileError::Tool);
        }
        output.push(value);
    }
    Ok(output)
}

fn validate_compiled_source(bytes: usize) -> Result<(), FileError> {
    validate_limit(bytes, COMPILED_BYTES_MAX)
}

fn validate_depth(depth: usize) -> Result<(), FileError> {
    validate_limit(depth, DIRECTORY_DEPTH_MAX)
}

fn validate_entries(entries: usize) -> Result<(), FileError> {
    validate_limit(entries, DIRECTORY_ENTRIES_MAX)
}

fn validate_results(results: usize) -> Result<(), FileError> {
    validate_limit(results, RESULTS_MAX)
}

fn validate_limit(value: usize, maximum: usize) -> Result<(), FileError> {
    if value > maximum {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

fn validate_pattern(value: &str) -> Result<(), FileError> {
    if value.is_empty()
        || value.len() > PATTERN_BYTES_MAX
        || value.contains('\0')
        || value.split('/').any(|part| part == "..")
    {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

pub(super) fn slash(path: &Path) -> String {
    let mut output = String::new();
    for component in path.components() {
        if !output.is_empty() {
            output.push('/');
        }
        output.push_str(&component.as_os_str().to_string_lossy());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    fn alternatives(count: usize) -> String {
        format!("{{{}}}", vec!["x"; count].join(","))
    }

    #[test]
    fn glob_pattern_byte_boundaries() {
        assert!(validate_pattern(&"x".repeat(4_095)).is_ok());
        assert!(validate_pattern(&"x".repeat(4_096)).is_ok());
        assert!(validate_pattern(&"x".repeat(4_097)).is_err());
    }

    #[test]
    fn glob_brace_alternative_boundaries() {
        assert_eq!(expand_braces(&alternatives(31)).map(|v| v.len()), Ok(31));
        assert_eq!(expand_braces(&alternatives(32)).map(|v| v.len()), Ok(32));
        assert!(expand_braces(&alternatives(33)).is_err());
        assert!(GlobMatcher::compile(&alternatives(32), true).is_ok());
    }

    #[test]
    fn glob_compiled_source_boundaries() {
        assert_eq!(validate_compiled_source(COMPILED_BYTES_MAX - 1), Ok(()));
        assert_eq!(validate_compiled_source(COMPILED_BYTES_MAX), Ok(()));
        assert_eq!(
            validate_compiled_source(COMPILED_BYTES_MAX + 1),
            Err(FileError::Tool)
        );
    }

    #[test]
    fn glob_traversal_boundaries() {
        for (validator, maximum) in [
            (validate_depth as fn(usize) -> _, DIRECTORY_DEPTH_MAX),
            (validate_entries as fn(usize) -> _, DIRECTORY_ENTRIES_MAX),
            (validate_results as fn(usize) -> _, RESULTS_MAX),
        ] {
            assert_eq!(validator(maximum - 1), Ok(()));
            assert_eq!(validator(maximum), Ok(()));
            assert_eq!(validator(maximum + 1), Err(FileError::Tool));
        }
    }

    #[test]
    fn common_globs_compile_once() {
        assert!(glob_matches("src/**/*.rs", "src/a.rs", true).is_ok_and(|value| value));
        assert!(glob_matches("src/**/*.rs", "src/a/b.rs", true).is_ok_and(|value| value));
        assert!(glob_matches("*.{rs,toml}", "Cargo.toml", true).is_ok_and(|value| value));
        assert!(glob_matches("a?c", "abc", true).is_ok_and(|value| value));
    }
}
