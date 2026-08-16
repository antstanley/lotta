//! Bounded permission rules, alias normalization, and path matching.

use lotta_domain::{PermissionMode, RuntimeScope};
use lotta_runtime::ports::PermissionAction;
use std::path::{Component, Path, PathBuf};

/// Maximum UTF-8 bytes retained by one rule.
pub const PERMISSION_RULE_BYTES_MAX: usize = 4_096;
/// Maximum UTF-8 bytes accepted for a policy path.
pub const PERMISSION_PATH_BYTES_MAX: usize = 16_384;
/// Maximum components accepted for a policy path or ancestor walk.
pub const PERMISSION_PATH_COMPONENTS_MAX: usize = 256;

/// Effect of one permission rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionEffect {
    /// Block a matching invocation.
    Deny,
    /// Require approval even when a mode would otherwise allow.
    AlwaysAsk,
    /// Permit a matching invocation.
    Allow,
    /// Require ordinary approval.
    Ask,
}

/// Validated typed constraints used by session and mod permission layers.
#[derive(Clone, Debug)]
pub struct PermissionRuleSpec {
    /// Rule effect.
    pub effect: PermissionEffect,
    /// Optional tool-name constraint.
    pub tool: Option<String>,
    /// Optional analyzed shell-command constraint.
    pub command: Option<String>,
    /// Optional absolute, UTF-8, traversal-free path constraint.
    ///
    /// Callers must provide the canonical policy snapshot used at check time. This snapshot cannot
    /// authorize I/O; the sandbox/effect adapter must re-resolve it before every effect.
    pub path: Option<PathBuf>,
    /// Optional absolute, UTF-8, traversal-free canonical working directory.
    pub cwd: Option<PathBuf>,
    /// Optional exact runtime-scope constraint.
    pub scope: Option<RuntimeScope>,
    /// Optional requested-action constraint.
    pub action: Option<PermissionAction>,
}

/// One bounded normalized permission rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionRule {
    effect: PermissionEffect,
    tool: Option<String>,
    command: Option<String>,
    path_pattern: Option<String>,
    cwd: Option<PathBuf>,
    scope: Option<RuntimeScope>,
    action: Option<PermissionAction>,
}

impl PermissionRule {
    /// Parses a baseline `Tool(payload)` rule.
    ///
    /// # Errors
    /// Returns a fixed error for malformed, empty, NUL-containing, or overbound rules.
    pub fn parse(value: &str, effect: PermissionEffect) -> Result<Self, PermissionError> {
        validate_text(value, PERMISSION_RULE_BYTES_MAX)?;
        let trimmed = value.trim();
        let (tool, payload) = split_rule(trimmed)?;
        let tool = canonical_tool_name(tool).to_owned();
        let (command, path_pattern) = if payload.is_some() && tool == "Bash" {
            (payload.map(normalize_command_pattern).transpose()?, None)
        } else if payload.is_some() && is_file_tool(&tool) {
            (None, payload.map(normalize_path_pattern))
        } else if payload.is_none() {
            (None, None)
        } else {
            return Err(PermissionError::InvalidRule);
        };
        Ok(Self {
            effect,
            tool: Some(tool),
            command,
            path_pattern,
            cwd: None,
            scope: None,
            action: None,
        })
    }

    /// Constructs and validates one typed session or mod rule.
    ///
    /// # Errors
    /// Returns a fixed error when a retained string or path exceeds its bound.
    pub fn from_spec(spec: PermissionRuleSpec) -> Result<Self, PermissionError> {
        let tool = spec
            .tool
            .map(|value| {
                validate_text(&value, PERMISSION_RULE_BYTES_MAX)?;
                Ok(canonical_tool_name(value.trim()).to_owned())
            })
            .transpose()?;
        let command = spec
            .command
            .map(|value| {
                validate_text(&value, PERMISSION_RULE_BYTES_MAX)?;
                normalize_command_pattern(&value)
            })
            .transpose()?;
        validate_optional_path(spec.path.as_deref())?;
        validate_optional_path(spec.cwd.as_deref())?;
        Ok(Self {
            effect: spec.effect,
            tool,
            command,
            path_pattern: spec.path.map(|path| path_to_string(&path)),
            cwd: spec.cwd,
            scope: spec.scope,
            action: spec.action,
        })
    }

    /// Returns this rule's effect.
    #[must_use]
    pub const fn effect(&self) -> PermissionEffect {
        self.effect
    }

    /// Returns whether this rule has an analyzed-command constraint.
    #[must_use]
    pub const fn has_command_constraint(&self) -> bool {
        self.command.is_some()
    }

    /// Tests every present constraint against all six normalized inputs.
    #[must_use]
    pub fn matches(&self, input: &PermissionMatchInput<'_>) -> bool {
        self.tool
            .as_deref()
            .is_none_or(|value| value == "*" || value == input.tool)
            && self.command.as_deref().is_none_or(|pattern| {
                input
                    .command
                    .is_some_and(|value| command_matches(value, pattern))
            })
            && self.path_pattern.as_deref().is_none_or(|pattern| {
                input
                    .path
                    .is_some_and(|value| path_matches(value, input.cwd, pattern))
            })
            && self.cwd.as_deref().is_none_or(|value| value == input.cwd)
            && self.scope.as_ref().is_none_or(|value| value == input.scope)
            && self
                .action
                .as_ref()
                .is_none_or(|value| value == input.action)
    }
}

/// All six normalized values available to the matcher.
pub struct PermissionMatchInput<'a> {
    /// Canonical tool family.
    pub tool: &'a str,
    /// Optional analyzed command.
    pub command: Option<&'a str>,
    /// Optional canonical target path.
    pub path: Option<&'a Path>,
    /// Canonical policy working directory.
    pub cwd: &'a Path,
    /// Exact runtime identity.
    pub scope: &'a RuntimeScope,
    /// Requested action.
    pub action: &'a PermissionAction,
}

/// Typed fixed permission failure without retained input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionError {
    /// A rule is malformed or violates its bound.
    InvalidRule,
    /// A path is malformed, unsafe, or violates its bound.
    InvalidPath,
    /// A settings source is malformed, unsafe, or violates its bound.
    InvalidSettings,
    /// A shell command is malformed, unsafe, or violates its bound.
    UnsafeShell,
}

/// Normalizes the pinned alias families once.
#[must_use]
pub fn canonical_tool_name(value: &str) -> &str {
    match value {
        "Bash" | "shell" | "Shell" | "shell_command" | "ShellCommand" | "exec_command"
        | "write_stdin" | "run_shell_command" | "RunShellCommand" => "Bash",
        "Read" | "read_file" | "ReadFile" | "read_file_gemini" | "ReadFileGemini"
        | "read_many_files" | "ReadManyFiles" | "view_image" | "ViewImage"
        | "read_artifact_file" => "Read",
        "Write"
        | "write_file"
        | "WriteFile"
        | "write_file_gemini"
        | "WriteFileGemini"
        | "write_artifact_file" => "Write",
        "Edit" | "MultiEdit" | "NotebookEdit" | "replace" | "Replace" | "apply_patch"
        | "ApplyPatch" | "memory_apply_patch" => "Edit",
        "Glob" | "glob_gemini" | "GlobGemini" => "Glob",
        "Grep" | "grep_files" | "GrepFiles" | "search_file_content" | "SearchFileContent" => "Grep",
        "list_dir" | "ListDir" | "list_directory" | "ListDirectory" | "LS" => "ListDir",
        "Task" | "task" | "Agent" | "agent" => "Task",
        other => other,
    }
}

/// Lexically normalizes and canonicalizes an invocation path for a bounded policy snapshot.
///
/// Existing ancestors and symlinks are resolved, while a nonexistent tail is appended to the
/// nearest resolved ancestor. This result is never an I/O capability: the sandbox/effect adapter
/// must resolve and no-follow-check the path again immediately before every effect.
///
/// # Errors
/// Returns a fixed path error for malformed, overbound, root-escaping, or inaccessible input.
pub fn canonicalize_invocation_path(cwd: &Path, value: &str) -> Result<PathBuf, PermissionError> {
    validate_text(value, PERMISSION_PATH_BYTES_MAX).map_err(|_| PermissionError::InvalidPath)?;
    let normalized = value.replace('\\', "/");
    let candidate = Path::new(&normalized);
    let component_count = validate_invocation_components(candidate)?;
    let absolute = if candidate.is_absolute() {
        normalize_lexically(candidate)?
    } else {
        normalize_lexically(&cwd.join(candidate))?
    };
    canonicalize_nearest(&absolute, component_count, candidate.is_absolute())
}

/// Canonicalizes a root that must already exist.
///
/// # Errors
/// Returns a fixed path error for malformed or non-canonicalizable roots.
pub fn canonicalize_root(value: &Path) -> Result<PathBuf, PermissionError> {
    validate_optional_path(Some(value))?;
    std::fs::canonicalize(value).map_err(|_| PermissionError::InvalidPath)
}

/// Tests component-aware path containment.
#[must_use]
pub fn path_within(path: &Path, root: &Path) -> bool {
    path == root || path.strip_prefix(root).is_ok()
}

fn split_rule(value: &str) -> Result<(&str, Option<&str>), PermissionError> {
    if let Some(open) = value.find('(') {
        if !value.ends_with(')') || open == 0 {
            return Err(PermissionError::InvalidRule);
        }
        let tool = value[..open].trim();
        let payload = &value[open + 1..value.len() - 1];
        if tool.is_empty() || payload.contains('(') || payload.contains(')') {
            return Err(PermissionError::InvalidRule);
        }
        Ok((tool, Some(payload)))
    } else if value.is_empty() || value.contains(')') {
        Err(PermissionError::InvalidRule)
    } else {
        Ok((value, None))
    }
}

fn validate_text(value: &str, maximum: usize) -> Result<(), PermissionError> {
    if value.is_empty() || value.len() > maximum || value.contains('\0') {
        Err(PermissionError::InvalidRule)
    } else {
        Ok(())
    }
}

fn validate_optional_path(value: Option<&Path>) -> Result<(), PermissionError> {
    if let Some(path) = value {
        let text = path.to_str().ok_or(PermissionError::InvalidPath)?;
        if !path.is_absolute() || text.is_empty() || text.len() > PERMISSION_PATH_BYTES_MAX {
            return Err(PermissionError::InvalidPath);
        }
        validate_components(path)?;
    }
    Ok(())
}

fn validate_components(path: &Path) -> Result<(), PermissionError> {
    let mut count = 0usize;
    for component in path.components() {
        count = count.checked_add(1).ok_or(PermissionError::InvalidPath)?;
        if count > PERMISSION_PATH_COMPONENTS_MAX || matches!(component, Component::ParentDir) {
            return Err(PermissionError::InvalidPath);
        }
        if matches!(component, Component::Normal(value) if value.is_empty()) {
            return Err(PermissionError::InvalidPath);
        }
    }
    Ok(())
}

fn validate_invocation_components(path: &Path) -> Result<usize, PermissionError> {
    let mut count = 0usize;
    for component in path.components() {
        count = count.checked_add(1).ok_or(PermissionError::InvalidPath)?;
        if count > PERMISSION_PATH_COMPONENTS_MAX
            || matches!(component, Component::Normal(value) if value.is_empty())
        {
            return Err(PermissionError::InvalidPath);
        }
    }
    Ok(count)
}

fn normalize_lexically(path: &Path) -> Result<PathBuf, PermissionError> {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                output.push(component);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() || output.as_os_str().is_empty() {
                    return Err(PermissionError::InvalidPath);
                }
            }
        }
    }
    if !output.is_absolute() {
        return Err(PermissionError::InvalidPath);
    }
    Ok(output)
}

fn canonicalize_nearest(
    path: &Path,
    input_components: usize,
    input_absolute: bool,
) -> Result<PathBuf, PermissionError> {
    let mut ancestor = path;
    let mut tail = Vec::new();
    tail.try_reserve(input_components.min(PERMISSION_PATH_COMPONENTS_MAX))
        .map_err(|_| PermissionError::InvalidPath)?;
    let mut result = loop {
        match std::fs::canonicalize(ancestor) {
            Ok(value) => break value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor.file_name().ok_or(PermissionError::InvalidPath)?;
                if tail.len() >= input_components {
                    return Err(PermissionError::InvalidPath);
                }
                tail.push(name.to_owned());
                ancestor = ancestor.parent().ok_or(PermissionError::InvalidPath)?;
            }
            Err(_) => return Err(PermissionError::InvalidPath),
        }
    };
    for value in tail.iter().rev() {
        result.push(value);
    }
    let text = result.to_str().ok_or(PermissionError::InvalidPath)?;
    if input_absolute && text.len() > PERMISSION_PATH_BYTES_MAX {
        return Err(PermissionError::InvalidPath);
    }
    Ok(result)
}

fn is_file_tool(value: &str) -> bool {
    matches!(
        value,
        "Read" | "Write" | "Edit" | "Glob" | "Grep" | "ListDir"
    )
}

fn normalize_command_pattern(value: &str) -> Result<String, PermissionError> {
    let mut output = String::new();
    output
        .try_reserve(value.len().min(PERMISSION_RULE_BYTES_MAX))
        .map_err(|_| PermissionError::InvalidRule)?;
    for word in value.split_whitespace() {
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(word);
    }
    if output.is_empty() || output.len() > PERMISSION_RULE_BYTES_MAX {
        return Err(PermissionError::InvalidRule);
    }
    Ok(output)
}

fn normalize_path_pattern(value: &str) -> String {
    let normalized = value.trim().replace('\\', "/");
    normalized
        .strip_prefix("./")
        .unwrap_or(&normalized)
        .to_owned()
}

fn command_matches(command: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix(":*") {
        command == prefix
            || command
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with(char::is_whitespace) || tail.starts_with('/'))
    } else {
        command == pattern
    }
}

fn path_matches(path: &Path, cwd: &Path, pattern: &str) -> bool {
    let absolute = path_to_string(path);
    let relative = path
        .strip_prefix(cwd)
        .map_or_else(|_| absolute.clone(), path_to_string);
    if pattern == "*" || pattern == "**" {
        return true;
    }
    let normalized = pattern.strip_prefix("//").map_or(pattern, |rest| {
        if pattern.starts_with("///") {
            pattern
        } else {
            rest
        }
    });
    let candidate = if pattern.starts_with('/') || pattern.starts_with("//") {
        &absolute
    } else {
        &relative
    };
    small_glob(candidate, normalized)
}

fn small_glob(value: &str, pattern: &str) -> bool {
    let mut values = [None; PERMISSION_PATH_COMPONENTS_MAX + 1];
    let mut patterns = [None; PERMISSION_PATH_COMPONENTS_MAX + 1];
    let Some(value_len) = split_components(value, &mut values) else {
        return false;
    };
    let Some(pattern_len) = split_components(pattern, &mut patterns) else {
        return false;
    };
    let mut current = [false; PERMISSION_PATH_COMPONENTS_MAX + 1];
    let mut next = [false; PERMISSION_PATH_COMPONENTS_MAX + 1];
    current[0] = true;
    for entry in patterns.iter().take(pattern_len) {
        next.fill(false);
        let Some(component) = *entry else {
            return false;
        };
        if component == "**" {
            next[0] = current[0];
            for index in 1..=value_len {
                next[index] = current[index] || next[index - 1];
            }
        } else {
            for index in 1..=value_len {
                if current[index - 1]
                    && values[index - 1].is_some_and(|value| component_glob(value, component))
                {
                    next[index] = true;
                }
            }
        }
        std::mem::swap(&mut current, &mut next);
    }
    current[value_len]
}

fn split_components<'a>(
    input: &'a str,
    output: &mut [Option<&'a str>; PERMISSION_PATH_COMPONENTS_MAX + 1],
) -> Option<usize> {
    let limit = PERMISSION_PATH_COMPONENTS_MAX.checked_add(usize::from(input.starts_with('/')))?;
    let mut count = 0;
    for component in input.split('/') {
        if count >= limit {
            return None;
        }
        output[count] = Some(component);
        count += 1;
    }
    Some(count)
}

fn component_glob(value: &str, pattern: &str) -> bool {
    let value = value.as_bytes();
    let pattern = pattern.as_bytes();
    let (mut value_index, mut pattern_index) = (0, 0);
    let mut star = None;
    while value_index < value.len() {
        if pattern.get(pattern_index) == Some(&value[value_index]) {
            value_index += 1;
            pattern_index += 1;
        } else if pattern.get(pattern_index) == Some(&b'*') {
            while pattern.get(pattern_index) == Some(&b'*') {
                pattern_index += 1;
            }
            star = Some((pattern_index, value_index));
        } else if let Some((resume, consumed)) = star {
            value_index = consumed + 1;
            pattern_index = resume;
            star = Some((resume, value_index));
        } else {
            return false;
        }
    }
    while pattern.get(pattern_index) == Some(&b'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Returns the mode override for a normalized tool, if any.
#[must_use]
pub fn mode_override(mode: PermissionMode, tool: &str) -> Option<PermissionEffect> {
    match mode {
        PermissionMode::Unrestricted => Some(PermissionEffect::Allow),
        PermissionMode::AcceptEdits if matches!(tool, "Write" | "Edit" | "memory") => {
            Some(PermissionEffect::Allow)
        }
        PermissionMode::Standard | PermissionMode::Strict | PermissionMode::AcceptEdits => None,
    }
}

#[cfg(test)]
mod canonicalize_nearest_tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn root() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "lotta-canonicalize-nearest-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    #[test]
    fn existing_and_nonexisting_tail_snapshots() {
        let cwd = root();
        fs::create_dir(cwd.join("existing")).unwrap();
        assert_eq!(
            canonicalize_invocation_path(&cwd, "existing").unwrap(),
            cwd.join("existing")
        );
        assert_eq!(
            canonicalize_invocation_path(&cwd, "existing/new/tail").unwrap(),
            cwd.join("existing/new/tail")
        );
        fs::remove_dir_all(cwd).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn existing_symlink_snapshot() {
        use std::os::unix::fs::symlink;
        let cwd = root();
        fs::create_dir(cwd.join("target")).unwrap();
        symlink(cwd.join("target"), cwd.join("link")).unwrap();
        assert_eq!(
            canonicalize_invocation_path(&cwd, "link/new").unwrap(),
            cwd.join("target/new")
        );
        fs::remove_dir_all(cwd).unwrap();
    }
}
