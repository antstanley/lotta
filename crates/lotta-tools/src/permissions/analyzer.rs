use super::matcher::{
    PERMISSION_PATH_BYTES_MAX, PermissionError, canonicalize_invocation_path, path_within,
};
use std::path::{Path, PathBuf};

/// Maximum UTF-8 bytes in one analyzed shell command.
pub const SHELL_COMMAND_BYTES_MAX: usize = 64 * 1_024;
/// Maximum command segments retained by one analysis.
pub const SHELL_SEGMENTS_MAX: usize = 128;
/// Maximum shell tokens retained by one analysis.
pub const SHELL_TOKENS_MAX: usize = 1_024;
/// Maximum nested executable wrappers inspected without recursion.
pub const SHELL_WRAPPER_DEPTH_MAX: usize = 8;

/// One bounded parsed shell command.
#[derive(Clone, Debug)]
pub struct ShellAnalysis {
    segments: Vec<String>,
    canonical_paths: Vec<PathBuf>,
    read_only: bool,
}
impl ShellAnalysis {
    /// Returns normalized nonempty command segments.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }
    /// Returns canonical policy-snapshot paths discovered in arguments.
    #[must_use]
    pub fn canonical_paths(&self) -> &[PathBuf] {
        &self.canonical_paths
    }
    /// Returns whether every segment belongs to the modest known read-only set.
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        self.read_only
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Quote {
    None,
    Single,
    Double,
}
#[derive(Clone, Copy)]
struct ExecutablePosition {
    index: usize,
    wrapped: bool,
}

/// Analyzes a shell command without executing it.
///
/// Relevant path arguments are canonicalized as a bounded policy snapshot and must remain under
/// `allowed_roots`. This snapshot is not an I/O capability: a sandbox/effect adapter must resolve
/// and no-follow-check every path again immediately before each effect.
///
/// # Errors
/// Returns a fixed unsafe-shell error for malformed syntax, launchers, traversal, confinement
/// escape, missing required option values, or a named bound violation.
pub fn analyze_shell(
    command: &str,
    cwd: &Path,
    roots: &[PathBuf],
) -> Result<ShellAnalysis, PermissionError> {
    if command.is_empty() || command.len() > SHELL_COMMAND_BYTES_MAX || command.contains('\0') {
        return Err(PermissionError::UnsafeShell);
    }
    let raw = split_segments(command, true)?;
    let mut segments = Vec::new();
    let mut paths = Vec::new();
    segments
        .try_reserve(raw.len())
        .map_err(|_| PermissionError::UnsafeShell)?;
    paths
        .try_reserve(SHELL_TOKENS_MAX)
        .map_err(|_| PermissionError::UnsafeShell)?;
    let mut total = 0usize;
    let mut read_only = true;
    for segment in raw {
        if segment.contains("..\\") || segment.contains("../") {
            return Err(PermissionError::UnsafeShell);
        }
        let tokens = tokenize(&segment)?;
        total = total
            .checked_add(tokens.len())
            .ok_or(PermissionError::UnsafeShell)?;
        if total > SHELL_TOKENS_MAX || tokens.is_empty() {
            return Err(PermissionError::UnsafeShell);
        }
        let position = inspect_launcher(&tokens, cwd, roots, &mut paths)?;
        if let Some(position) = position {
            collect_paths(&tokens, position.index, cwd, roots, &mut paths)?;
            read_only &= read_only_segment(&tokens, position);
        } else {
            read_only = false;
        }
        segments.push(tokens.join(" "));
    }
    Ok(ShellAnalysis {
        segments,
        canonical_paths: paths,
        read_only,
    })
}
pub(super) fn split_raw_segments(command: &str) -> Result<Vec<String>, PermissionError> {
    split_segments(command, false)
}
fn split_segments(command: &str, reject: bool) -> Result<Vec<String>, PermissionError> {
    let mut output = Vec::new();
    output
        .try_reserve(SHELL_SEGMENTS_MAX)
        .map_err(|_| PermissionError::UnsafeShell)?;
    let mut current = String::new();
    current
        .try_reserve(command.len())
        .map_err(|_| PermissionError::UnsafeShell)?;
    let (mut quote, mut escaping, mut index) = (Quote::None, false, 0usize);
    while index < command.len() {
        let character = command[index..]
            .chars()
            .next()
            .ok_or(PermissionError::UnsafeShell)?;
        let len = character.len_utf8();
        if escaping {
            current.push(character);
            escaping = false;
            index += len;
            continue;
        }
        if character == '\\' && quote != Quote::Single {
            escaping = true;
            current.push(character);
            index += len;
            continue;
        }
        if update_quote(character, &mut quote) {
            current.push(character);
            index += len;
            continue;
        }
        if reject {
            reject_operator(command, index, quote)?;
        }
        let separator = quote == Quote::None && separator_len(command, index) > 0;
        if separator {
            push_segment(&mut output, &current)?;
            current.clear();
            index += separator_len(command, index);
        } else {
            current.push(character);
            index += len;
        }
    }
    if escaping || quote != Quote::None {
        return Err(PermissionError::UnsafeShell);
    }
    push_segment(&mut output, &current)?;
    Ok(output)
}
fn update_quote(c: char, quote: &mut Quote) -> bool {
    match (c, *quote) {
        ('\'', Quote::None) => *quote = Quote::Single,
        ('\'', Quote::Single) | ('"', Quote::Double) => *quote = Quote::None,
        ('"', Quote::None) => *quote = Quote::Double,
        _ => return false,
    }
    true
}
fn reject_operator(command: &str, index: usize, quote: Quote) -> Result<(), PermissionError> {
    let tail = &command[index..];
    let c = tail.chars().next().ok_or(PermissionError::UnsafeShell)?;
    if quote != Quote::Single && (c == '`' || tail.starts_with("$(")) {
        return Err(PermissionError::UnsafeShell);
    }
    if quote == Quote::None && matches!(c, '<' | '>') {
        return Err(PermissionError::UnsafeShell);
    }
    Ok(())
}
fn separator_len(command: &str, index: usize) -> usize {
    let tail = &command[index..];
    if tail.starts_with("&&") || tail.starts_with("||") {
        2
    } else {
        usize::from(
            tail.starts_with(';')
                || tail.starts_with('|')
                || tail.starts_with('&')
                || tail.starts_with('\n')
                || tail.starts_with('\r'),
        )
    }
}
fn push_segment(output: &mut Vec<String>, current: &str) -> Result<(), PermissionError> {
    let segment = current.trim();
    if segment.is_empty() || output.len() >= SHELL_SEGMENTS_MAX {
        return Err(PermissionError::UnsafeShell);
    }
    output.push(segment.to_owned());
    Ok(())
}
fn tokenize(segment: &str) -> Result<Vec<String>, PermissionError> {
    let mut tokens = Vec::new();
    tokens
        .try_reserve(segment.len().min(SHELL_TOKENS_MAX))
        .map_err(|_| PermissionError::UnsafeShell)?;
    let (mut current, mut quote, mut escaping) = (String::new(), Quote::None, false);
    for c in segment.chars() {
        if escaping {
            current.push(c);
            escaping = false;
        } else if c == '\\' && quote != Quote::Single {
            escaping = true;
        } else if update_quote(c, &mut quote) {
        } else if c.is_whitespace() && quote == Quote::None {
            flush_token(&mut tokens, &mut current)?;
        } else {
            current.push(c);
        }
    }
    if escaping || quote != Quote::None {
        return Err(PermissionError::UnsafeShell);
    }
    flush_token(&mut tokens, &mut current)?;
    Ok(tokens)
}
fn flush_token(tokens: &mut Vec<String>, current: &mut String) -> Result<(), PermissionError> {
    if !current.is_empty() {
        if tokens.len() >= SHELL_TOKENS_MAX || current.len() > PERMISSION_PATH_BYTES_MAX {
            return Err(PermissionError::UnsafeShell);
        }
        tokens.push(std::mem::take(current));
    }
    Ok(())
}

fn inspect_launcher(
    tokens: &[String],
    cwd: &Path,
    roots: &[PathBuf],
    paths: &mut Vec<PathBuf>,
) -> Result<Option<ExecutablePosition>, PermissionError> {
    let mut index = 0usize;
    while tokens.get(index).is_some_and(|value| is_assignment(value)) {
        index += 1;
    }
    let mut wrapped = false;
    for _ in 0..=SHELL_WRAPPER_DEPTH_MAX {
        let executable = tokens.get(index).ok_or(PermissionError::UnsafeShell)?;
        match basename(executable) {
            "bash" | "sh" | "zsh" | "dash" | "ksh" => return Err(PermissionError::UnsafeShell),
            "env" => index = env_executable(tokens, index + 1, cwd, roots, paths)?,
            "command" => match command_executable(tokens, index + 1)? {
                Some(next) => index = next,
                None => return Ok(None),
            },
            "xargs" => match xargs_executable(tokens, index + 1, cwd, roots, paths)? {
                Some(next) => index = next,
                None => return Ok(None),
            },
            "find" => {
                reject_find_launcher(tokens, index + 1)?;
                return Ok(Some(ExecutablePosition { index, wrapped }));
            }
            "rg" => {
                reject_rg_launcher(tokens, index + 1)?;
                return Ok(Some(ExecutablePosition { index, wrapped }));
            }
            _ => return Ok(Some(ExecutablePosition { index, wrapped })),
        }
        wrapped = true;
    }
    Err(PermissionError::UnsafeShell)
}
fn env_executable(
    tokens: &[String],
    mut index: usize,
    cwd: &Path,
    roots: &[PathBuf],
    paths: &mut Vec<PathBuf>,
) -> Result<usize, PermissionError> {
    while let Some(value) = tokens.get(index) {
        if value == "--" {
            return required_next(tokens, index);
        }
        if is_assignment(value) || matches!(value.as_str(), "-i" | "--ignore-environment") {
            index += 1;
        } else if matches!(value.as_str(), "-u" | "--unset" | "-S" | "--split-string") {
            required_next(tokens, index)?;
            index += 2;
        } else if matches!(value.as_str(), "-C" | "--chdir") {
            let path_index = required_next(tokens, index)?;
            push_path(&tokens[path_index], cwd, roots, paths)?;
            index += 2;
        } else if value.starts_with("--unset=") || value.starts_with("--split-string=") {
            index += 1;
        } else if let Some(path) = value.strip_prefix("--chdir=") {
            push_path(path, cwd, roots, paths)?;
            index += 1;
        } else if value.starts_with('-') {
            return Err(PermissionError::UnsafeShell);
        } else {
            return Ok(index);
        }
    }
    Err(PermissionError::UnsafeShell)
}
fn command_executable(
    tokens: &[String],
    mut index: usize,
) -> Result<Option<usize>, PermissionError> {
    while let Some(value) = tokens.get(index) {
        match value.as_str() {
            "--" => return Ok(Some(required_next(tokens, index)?)),
            "-p" => index += 1,
            "-v" | "-V" => return Ok(None),
            _ if value.starts_with('-') => return Err(PermissionError::UnsafeShell),
            _ => return Ok(Some(index)),
        }
    }
    Err(PermissionError::UnsafeShell)
}
fn xargs_executable(
    tokens: &[String],
    mut index: usize,
    cwd: &Path,
    roots: &[PathBuf],
    paths: &mut Vec<PathBuf>,
) -> Result<Option<usize>, PermissionError> {
    while let Some(value) = tokens.get(index) {
        if value == "--" {
            return Ok(tokens.get(index + 1).map(|_| index + 1));
        }
        if !value.starts_with('-') {
            return Ok(Some(index));
        }
        if matches!(
            value.as_str(),
            "-0" | "--null" | "-r" | "--no-run-if-empty" | "-t" | "--verbose" | "-x" | "--exit"
        ) {
            index += 1;
        } else if xargs_separate_option(value) {
            let argument = required_next(tokens, index)?;
            if matches!(value.as_str(), "-a" | "--arg-file") {
                push_path(&tokens[argument], cwd, roots, paths)?;
            }
            index += 2;
        } else if let Some(path) = xargs_arg_file(value) {
            push_path(path, cwd, roots, paths)?;
            index += 1;
        } else if xargs_attached_option(value) {
            index += 1;
        } else {
            return Err(PermissionError::UnsafeShell);
        }
    }
    Ok(None)
}
fn xargs_separate_option(value: &str) -> bool {
    matches!(
        value,
        "-a" | "--arg-file"
            | "-d"
            | "--delimiter"
            | "-E"
            | "--eof"
            | "-I"
            | "--replace"
            | "-L"
            | "--max-lines"
            | "-n"
            | "--max-args"
            | "-P"
            | "--max-procs"
            | "-s"
            | "--max-chars"
    )
}
fn xargs_arg_file(value: &str) -> Option<&str> {
    value
        .strip_prefix("--arg-file=")
        .or_else(|| value.strip_prefix("-a").filter(|path| !path.is_empty()))
}
fn xargs_attached_option(value: &str) -> bool {
    [
        "--delimiter=",
        "--eof=",
        "--replace=",
        "--max-lines=",
        "--max-args=",
        "--max-procs=",
        "--max-chars=",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
        || ["-d", "-E", "-I", "-L", "-n", "-P", "-s"]
            .iter()
            .any(|prefix| value.starts_with(prefix) && value.len() > prefix.len())
}
fn required_next(tokens: &[String], index: usize) -> Result<usize, PermissionError> {
    let next = index.checked_add(1).ok_or(PermissionError::UnsafeShell)?;
    tokens
        .get(next)
        .map(|_| next)
        .ok_or(PermissionError::UnsafeShell)
}
fn reject_find_launcher(tokens: &[String], start: usize) -> Result<(), PermissionError> {
    if tokens
        .iter()
        .skip(start)
        .any(|value| matches!(value.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
    {
        Err(PermissionError::UnsafeShell)
    } else {
        Ok(())
    }
}
fn reject_rg_launcher(tokens: &[String], start: usize) -> Result<(), PermissionError> {
    let mut index = start;
    while let Some(value) = tokens.get(index) {
        if matches!(value.as_str(), "--search-zip" | "-z") {
            return Err(PermissionError::UnsafeShell);
        }
        if matches!(value.as_str(), "--pre" | "--hostname-bin") {
            required_next(tokens, index)?;
            return Err(PermissionError::UnsafeShell);
        }
        if value.starts_with("--pre=") || value.starts_with("--hostname-bin=") {
            return Err(PermissionError::UnsafeShell);
        }
        index += 1;
    }
    Ok(())
}

fn collect_paths(
    tokens: &[String],
    executable_index: usize,
    cwd: &Path,
    roots: &[PathBuf],
    output: &mut Vec<PathBuf>,
) -> Result<(), PermissionError> {
    let executable = basename(&tokens[executable_index]);
    let known_reader = matches!(
        executable,
        "cat" | "head" | "tail" | "grep" | "rg" | "ls" | "wc" | "git" | "find"
    );
    let data_command = matches!(executable, "echo" | "printf");
    let mut positional = false;
    let mut index = executable_index + 1;
    while index < tokens.len() {
        let raw = &tokens[index];
        if raw == "--" {
            positional = true;
            index += 1;
            continue;
        }
        if let Some((value, consumed)) = path_option_value(executable, raw, tokens.get(index + 1))?
        {
            push_path(value, cwd, roots, output)?;
            index += 1 + usize::from(consumed);
            continue;
        }
        if !positional && raw.starts_with('-') {
            index += 1;
            continue;
        }
        if !data_command && (known_reader || looks_path_like(raw)) {
            push_path(raw, cwd, roots, output)?;
        }
        index += 1;
    }
    Ok(())
}
fn path_option_value<'a>(
    executable: &str,
    raw: &'a str,
    next: Option<&'a String>,
) -> Result<Option<(&'a str, bool)>, PermissionError> {
    let separate = matches!(
        (executable, raw),
        ("git", "-C" | "--git-dir" | "--work-tree")
            | ("grep", "-f" | "--file" | "--exclude-from")
            | ("rg", "-f" | "--file" | "--ignore-file")
            | ("head" | "tail", "--files0-from")
            | ("find", "-fprint" | "-fls" | "-fprintf")
    );
    if separate {
        return next
            .map(String::as_str)
            .map(|value| Some((value, true)))
            .ok_or(PermissionError::UnsafeShell);
    }
    if executable == "git"
        && let Some(value) = raw.strip_prefix("-C").filter(|value| !value.is_empty())
    {
        return Ok(Some((value, false)));
    }
    for prefix in path_option_prefixes(executable) {
        if let Some(value) = raw.strip_prefix(prefix) {
            if value.is_empty() {
                return Err(PermissionError::UnsafeShell);
            }
            return Ok(Some((value, false)));
        }
    }
    Ok(None)
}
fn path_option_prefixes(executable: &str) -> &'static [&'static str] {
    match executable {
        "git" => &["--git-dir=", "--work-tree="],
        "grep" => &["--file=", "--exclude-from="],
        "rg" => &["--file=", "--ignore-file="],
        "head" | "tail" => &["--files0-from="],
        _ => &[],
    }
}
fn push_path(
    value: &str,
    cwd: &Path,
    roots: &[PathBuf],
    output: &mut Vec<PathBuf>,
) -> Result<(), PermissionError> {
    if value.is_empty() || has_traversal(value) {
        return Err(PermissionError::UnsafeShell);
    }
    let canonical =
        canonicalize_invocation_path(cwd, value).map_err(|_| PermissionError::UnsafeShell)?;
    if !roots.iter().any(|root| path_within(&canonical, root)) || output.len() >= SHELL_TOKENS_MAX {
        return Err(PermissionError::UnsafeShell);
    }
    output.push(canonical);
    Ok(())
}
fn has_traversal(value: &str) -> bool {
    value.replace('\\', "/").split('/').any(|part| part == "..")
}
fn looks_path_like(value: &str) -> bool {
    value.starts_with('.')
        || value.starts_with('/')
        || value.contains('/')
        || value.contains('\\')
        || value.contains('.')
}
fn read_only_segment(tokens: &[String], position: ExecutablePosition) -> bool {
    if position.wrapped && basename(&tokens[position.index]) == "xargs" {
        return false;
    }
    let executable = basename(&tokens[position.index]);
    match executable {
        "cat" | "head" | "tail" | "grep" | "ls" | "pwd" | "wc" => true,
        "find" => !tokens.iter().skip(position.index + 1).any(|value| {
            value == "-delete"
                || matches!(value.as_str(), "-fprint" | "-fls" | "-fprintf")
                || value.starts_with("-fprint")
        }),
        "rg" => !tokens.iter().any(|value| {
            value == "--search-zip"
                || value == "-z"
                || value == "--pre"
                || value == "--hostname-bin"
                || value.starts_with("--pre=")
                || value.starts_with("--hostname-bin=")
        }),
        "git" => {
            !unsafe_git_option(tokens)
                && git_subcommand(tokens, position.index)
                    .is_some_and(|value| matches!(value, "status" | "diff" | "log" | "show"))
        }
        _ => false,
    }
}
fn unsafe_git_option(tokens: &[String]) -> bool {
    tokens.iter().any(|value| {
        matches!(
            value.as_str(),
            "--ext-diff" | "--output" | "--textconv" | "--exec"
        ) || value.starts_with("--output=")
            || value.starts_with("--textconv=")
            || value.starts_with("--exec=")
    })
}
fn git_subcommand(tokens: &[String], executable_index: usize) -> Option<&str> {
    let mut index = executable_index + 1;
    while let Some(value) = tokens.get(index) {
        if matches!(value.as_str(), "-C" | "--git-dir" | "--work-tree") {
            index = index.checked_add(2)?;
        } else if value.starts_with("-C")
            || value.starts_with("--git-dir=")
            || value.starts_with("--work-tree=")
            || value.starts_with('-')
        {
            index = index.checked_add(1)?;
        } else {
            return Some(value);
        }
    }
    None
}
fn basename(value: &str) -> &str {
    value.rsplit(['/', '\\']).next().unwrap_or(value)
}
fn is_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|value| value == '_' || value.is_ascii_alphabetic())
        && name
            .chars()
            .all(|value| value == '_' || value.is_ascii_alphanumeric())
}
#[cfg(test)]
#[path = "tests/analyzer.rs"]
mod tests;
