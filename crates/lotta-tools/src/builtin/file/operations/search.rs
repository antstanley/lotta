use super::read::text_file;
use super::{
    FileError, FileState, OperationControl, READ_MANY_AGGREGATE_BYTES_MAX, RESULTS_MAX, Value,
    integer, push_bounded, push_bounded_limit, string, workspace_relative,
};
use crate::builtin::file::glob::{GlobMatcher, matching_files, slash, workspace_files};
use regex::{Regex, RegexBuilder};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

const GREP_FILES_MAX: usize = 10_000;
const GREP_BYTES_MAX: usize = 64 * 1024 * 1024;
const GREP_LINES_MAX: usize = 1_000_000;
const GREP_MATCHES_MAX: usize = 100_000;
const DEFAULT_EXCLUDES: [&str; 8] = [
    "**/node_modules/**",
    "**/.git/**",
    "**/dist/**",
    "**/build/**",
    "**/.next/**",
    "**/coverage/**",
    "**/*.min.js",
    "**/*.bundle.js",
];

pub(super) fn glob(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    control.check()?;
    let base = base_path(state, input)?;
    let files = matching_files(state, &base, string(input, "pattern")?, true, control)?;
    let mut output = String::new();
    for path in files {
        control.check()?;
        if !output.is_empty() {
            push_bounded(&mut output, "\n")?;
        }
        push_bounded(&mut output, &slash(&path))?;
    }
    Ok(output)
}

pub(super) fn grep(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    control.check()?;
    let regex = grep_regex(input)?;
    let base = base_path(state, input)?;
    let matchers = grep_matchers(input)?;
    let files = workspace_files(state, &base, control)?;
    grep_files(state, input, &regex, &matchers, &base, &files, control)
}

fn base_path(state: &FileState, input: &Value) -> Result<PathBuf, FileError> {
    let raw = input
        .get("dir_path")
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)
        .unwrap_or(".");
    if raw == "." {
        Ok(PathBuf::new())
    } else {
        workspace_relative(state, raw)
    }
}

fn validate_maximum(value: usize, maximum: usize) -> Result<(), FileError> {
    if value > maximum {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

fn validate_regex_pattern_bytes(bytes: usize) -> Result<(), FileError> {
    validate_maximum(bytes, 4_096)
}

fn validate_context(value: usize) -> Result<(), FileError> {
    validate_maximum(value, 100)
}

fn validate_pattern_counts(include: usize, exclude: usize) -> Result<(), FileError> {
    if include == 0 {
        return Err(FileError::Tool);
    }
    validate_maximum(include, 32)?;
    validate_maximum(exclude, 32)
}

fn grep_regex(input: &Value) -> Result<Regex, FileError> {
    let pattern = string(input, "pattern")?;
    let multiline = input
        .get("multiline")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    validate_regex_pattern_bytes(pattern.len())?;
    RegexBuilder::new(pattern)
        .case_insensitive(input.get("-i").and_then(Value::as_bool).unwrap_or(false))
        .multi_line(multiline)
        .dot_matches_new_line(multiline)
        .size_limit(1024 * 1024)
        .build()
        .map_err(|_| FileError::Tool)
}

fn grep_matchers(input: &Value) -> Result<Vec<GlobMatcher>, FileError> {
    let kind = input.get("type").map(type_pattern).transpose()?;
    let include = input
        .get("include")
        .or_else(|| input.get("glob"))
        .map(|value| value.as_str().ok_or(FileError::Tool))
        .transpose()?;
    let count = usize::from(kind.is_some()) + usize::from(include.is_some());
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|_| FileError::Tool)?;
    if let Some(pattern) = kind {
        output.push(GlobMatcher::compile(pattern, true)?);
    }
    if let Some(pattern) = include {
        output.push(GlobMatcher::compile(pattern, true)?);
    }
    if output.is_empty() {
        output.try_reserve_exact(1).map_err(|_| FileError::Tool)?;
        output.push(GlobMatcher::compile("**", true)?);
    }
    Ok(output)
}

fn type_pattern(value: &Value) -> Result<&'static str, FileError> {
    match value.as_str().ok_or(FileError::Tool)? {
        "js" | "jsx" => Ok("**/*.{js,jsx}"),
        "ts" | "tsx" => Ok("**/*.{ts,tsx}"),
        "py" => Ok("**/*.py"),
        "rust" | "rs" => Ok("**/*.rs"),
        "go" => Ok("**/*.go"),
        "java" => Ok("**/*.java"),
        "json" => Ok("**/*.json"),
        "md" | "markdown" => Ok("**/*.md"),
        "toml" => Ok("**/*.toml"),
        "yaml" | "yml" => Ok("**/*.{yaml,yml}"),
        _ => Err(FileError::Tool),
    }
}

struct GrepConfig<'a> {
    mode: &'a str,
    before: usize,
    after: usize,
    numbers: bool,
    offset: usize,
    head: usize,
    multiline: bool,
}

fn grep_config(input: &Value) -> Result<GrepConfig<'_>, FileError> {
    let mode = input
        .get("output_mode")
        .and_then(Value::as_str)
        .unwrap_or("files_with_matches");
    if !matches!(mode, "content" | "count" | "files_with_matches") {
        return Err(FileError::Tool);
    }
    let before = integer(input, "-C")?.or(integer(input, "-B")?).unwrap_or(0);
    let after = integer(input, "-C")?.or(integer(input, "-A")?).unwrap_or(0);
    validate_context(before)?;
    validate_context(after)?;
    Ok(GrepConfig {
        mode,
        before,
        after,
        numbers: input.get("-n").and_then(Value::as_bool).unwrap_or(true),
        offset: integer(input, "offset")?.unwrap_or(0),
        head: integer(input, "head_limit")?.unwrap_or(100),
        multiline: input
            .get("multiline")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

struct GrepState {
    output: String,
    entries: usize,
    bytes: usize,
    files: usize,
    lines: usize,
    matches: usize,
}

fn grep_files(
    state: &FileState,
    input: &Value,
    regex: &Regex,
    matchers: &[GlobMatcher],
    base: &Path,
    files: &[PathBuf],
    control: &OperationControl,
) -> Result<String, FileError> {
    let config = grep_config(input)?;
    let mut scan = GrepState {
        output: String::new(),
        entries: 0,
        bytes: 0,
        files: 0,
        lines: 0,
        matches: 0,
    };
    let source = GrepSource {
        state,
        base,
        matchers,
        regex,
        config: &config,
        control,
    };
    for path in files {
        scan_file(&source, path, &mut scan)?;
    }
    finish_grep(&scan, &config, files.is_empty())
}

struct GrepSource<'a> {
    state: &'a FileState,
    base: &'a Path,
    matchers: &'a [GlobMatcher],
    regex: &'a Regex,
    config: &'a GrepConfig<'a>,
    control: &'a OperationControl,
}

fn scan_file(source: &GrepSource<'_>, path: &Path, scan: &mut GrepState) -> Result<(), FileError> {
    source.control.check()?;
    let relative = path.strip_prefix(source.base).unwrap_or(path);
    if !source
        .matchers
        .iter()
        .all(|matcher| matcher.is_match(&slash(relative)))
    {
        return Ok(());
    }
    scan.files = checked_limit(scan.files, 1, GREP_FILES_MAX)?;
    let Ok(text) = text_file(&source.state.workspace, path, source.control) else {
        return Ok(());
    };
    scan.bytes = checked_limit(scan.bytes, text.len(), GREP_BYTES_MAX)?;
    let lines: Vec<_> = text.split('\n').collect();
    scan.lines = checked_limit(scan.lines, lines.len(), GREP_LINES_MAX)?;
    let marks = if source.config.multiline {
        match_multiline(
            &text,
            &lines,
            source.regex,
            source.config,
            scan,
            source.control,
        )?
    } else {
        match_lines(&lines, source.regex, source.config, scan, source.control)?
    };
    render_file(path, &lines, &marks, source, scan)
}

fn finish_grep(
    scan: &GrepState,
    config: &GrepConfig<'_>,
    no_files: bool,
) -> Result<String, FileError> {
    if scan.matches == 0 {
        return Ok(match config.mode {
            "files_with_matches" => "No files found".into(),
            "count" => "0\n\nFound 0 total occurrences across 0 files.".into(),
            _ if no_files => "No matches found".into(),
            _ => "No matches found".into(),
        });
    }
    let mut output = scan.output.trim_end_matches('\n').to_owned();
    if config.mode == "files_with_matches" {
        let shown = selected_count(scan.entries, config);
        let suffix = if shown == scan.entries {
            String::new()
        } else {
            format!(" (showing {shown})")
        };
        let noun = if scan.entries == 1 { "file" } else { "files" };
        output = format!("Found {} {noun}{suffix}\n{output}", scan.entries);
    } else if config.mode == "count" {
        write!(
            output,
            "\n\nFound {} total occurrence{} across {} file{}.",
            scan.matches,
            plural(scan.matches),
            scan.entries,
            plural(scan.entries)
        )
        .map_err(|_| FileError::Tool)?;
    }
    Ok(output)
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn selected_count(entries: usize, config: &GrepConfig<'_>) -> usize {
    let available = entries.saturating_sub(config.offset);
    if config.head == 0 {
        available
    } else {
        available.min(config.head)
    }
}

fn match_multiline(
    text: &str,
    lines: &[&str],
    regex: &Regex,
    config: &GrepConfig<'_>,
    scan: &mut GrepState,
    control: &OperationControl,
) -> Result<LineMarks, FileError> {
    let starts = line_starts(text, lines.len())?;
    let mut marks = LineMarks::new(lines.len())?;
    for found in regex.find_iter(text) {
        control.check()?;
        scan.matches = checked_limit(scan.matches, 1, GREP_MATCHES_MAX)?;
        let start = line_index(&starts, found.start());
        let last = found.end().saturating_sub(1).max(found.start());
        let end = line_index(&starts, last);
        marks.hits[start..=end].fill(true);
        let from = start.saturating_sub(config.before);
        let to = end
            .saturating_add(config.after)
            .saturating_add(1)
            .min(lines.len());
        marks.context[from..to].fill(true);
    }
    Ok(marks)
}

fn line_index(starts: &[usize], byte: usize) -> usize {
    starts
        .partition_point(|offset| *offset <= byte)
        .saturating_sub(1)
}

fn line_starts(text: &str, line_count: usize) -> Result<Vec<usize>, FileError> {
    let mut starts = Vec::new();
    starts
        .try_reserve_exact(line_count)
        .map_err(|_| FileError::Tool)?;
    if line_count != 0 {
        starts.push(0);
    }
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index.checked_add(1).ok_or(FileError::Tool)?);
        }
    }
    if starts.len() != line_count {
        return Err(FileError::Tool);
    }
    Ok(starts)
}

struct LineMarks {
    hits: Vec<bool>,
    context: Vec<bool>,
}

impl LineMarks {
    fn new(count: usize) -> Result<Self, FileError> {
        let mut hits = Vec::new();
        let mut context = Vec::new();
        hits.try_reserve_exact(count).map_err(|_| FileError::Tool)?;
        context
            .try_reserve_exact(count)
            .map_err(|_| FileError::Tool)?;
        hits.resize(count, false);
        context.resize(count, false);
        Ok(Self { hits, context })
    }
}

fn match_lines(
    lines: &[&str],
    regex: &Regex,
    config: &GrepConfig<'_>,
    scan: &mut GrepState,
    control: &OperationControl,
) -> Result<LineMarks, FileError> {
    let mut marks = LineMarks::new(lines.len())?;
    for (index, line) in lines.iter().enumerate() {
        control.check()?;
        if regex.is_match(line) {
            scan.matches = checked_limit(scan.matches, 1, GREP_MATCHES_MAX)?;
            marks.hits[index] = true;
            let start = index.saturating_sub(config.before);
            let end = index
                .saturating_add(config.after)
                .saturating_add(1)
                .min(lines.len());
            marks.context[start..end].fill(true);
        }
    }
    Ok(marks)
}

fn render_file(
    path: &Path,
    lines: &[&str],
    marks: &LineMarks,
    source: &GrepSource<'_>,
    scan: &mut GrepState,
) -> Result<(), FileError> {
    let config = source.config;
    let control = source.control;
    let hits = marks.hits.iter().filter(|hit| **hit).count();
    if hits == 0 {
        return Ok(());
    }
    let name = slash(path);
    if config.mode != "content" {
        let value = if config.mode == "count" {
            format!("{name}:{hits}")
        } else {
            name
        };
        return append_grep_entry(scan, config, &value);
    }
    let mut previous = false;
    for (index, line) in lines.iter().enumerate() {
        control.check()?;
        if !marks.context[index] {
            previous = false;
            continue;
        }
        if !previous && !scan.output.is_empty() {
            push_bounded(&mut scan.output, "--\n")?;
        }
        previous = true;
        let sep = if marks.hits[index] { ":" } else { "-" };
        let value = format_line(&name, sep, index, line, config.numbers);
        append_grep_entry(scan, config, &value)?;
    }
    Ok(())
}

fn format_line(name: &str, separator: &str, index: usize, line: &str, numbers: bool) -> String {
    if numbers {
        format!("{name}{separator}{}{separator}{line}", index + 1)
    } else {
        format!("{name}{separator}{line}")
    }
}

fn append_grep_entry(
    scan: &mut GrepState,
    config: &GrepConfig<'_>,
    value: &str,
) -> Result<(), FileError> {
    let index = scan.entries;
    scan.entries = scan.entries.checked_add(1).ok_or(FileError::Tool)?;
    let end = config
        .offset
        .checked_add(config.head)
        .ok_or(FileError::Tool)?;
    if index < config.offset || (config.head != 0 && index >= end) {
        return Ok(());
    }
    push_bounded(&mut scan.output, value)?;
    push_bounded(&mut scan.output, "\n")
}

fn checked_limit(current: usize, added: usize, maximum: usize) -> Result<usize, FileError> {
    let value = current.checked_add(added).ok_or(FileError::Tool)?;
    if value > maximum {
        Err(FileError::Tool)
    } else {
        Ok(value)
    }
}

pub(super) fn read_many(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    control.check()?;
    let include = input
        .get("include")
        .and_then(Value::as_array)
        .ok_or(FileError::Tool)?;
    let exclude = input
        .get("exclude")
        .map(|value| value.as_array().ok_or(FileError::Tool))
        .transpose()?
        .map_or(&[][..], Vec::as_slice);
    validate_pattern_counts(include.len(), exclude.len())?;
    let includes = compile_matchers(include, &[], control)?;
    let defaults = input
        .get("useDefaultExcludes")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let extra = if defaults { &DEFAULT_EXCLUDES[..] } else { &[] };
    let excludes = compile_matchers(exclude, extra, control)?;
    render_many(state, input, &includes, &excludes, control)
}

fn compile_matchers(
    values: &[Value],
    fixed: &[&str],
    control: &OperationControl,
) -> Result<Vec<GlobMatcher>, FileError> {
    let count = values
        .len()
        .checked_add(fixed.len())
        .ok_or(FileError::Tool)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|_| FileError::Tool)?;
    for value in values {
        control.check()?;
        output.push(GlobMatcher::compile(
            value.as_str().ok_or(FileError::Tool)?,
            true,
        )?);
    }
    for pattern in fixed {
        control.check()?;
        output.push(GlobMatcher::compile(pattern, true)?);
    }
    Ok(output)
}

fn render_many(
    state: &FileState,
    _input: &Value,
    includes: &[GlobMatcher],
    excludes: &[GlobMatcher],
    control: &OperationControl,
) -> Result<String, FileError> {
    let files = workspace_files(state, Path::new(""), control)?;
    let mut output = String::new();
    let mut found = 0usize;
    for path in files {
        control.check()?;
        let name = slash(&path);
        if skip_many(&name, includes, excludes) {
            continue;
        }
        let Ok(text) = text_file(&state.workspace, &path, control) else {
            continue;
        };
        found = checked_limit(found, 1, RESULTS_MAX)?;
        let resolved = state.workspace_root.join(&path);
        let rendered = super::read::render_read(
            &text,
            &resolved,
            0,
            super::LINES_DEFAULT,
            false,
            Some(control),
        )?;
        push_read_many(&mut output, "--- ")?;
        push_read_many(&mut output, &resolved.display().to_string())?;
        push_read_many(&mut output, " ---\n\n")?;
        push_read_many(&mut output, &rendered)?;
        push_read_many(&mut output, "\n\n")?;
    }
    if found == 0 {
        return Ok("No files matching the criteria were found or all were skipped.".into());
    }
    push_read_many(&mut output, "--- End of content ---")?;
    Ok(output)
}

fn push_read_many(output: &mut String, value: &str) -> Result<(), FileError> {
    push_bounded_limit(output, value, READ_MANY_AGGREGATE_BYTES_MAX)
}

fn skip_many(name: &str, includes: &[GlobMatcher], excludes: &[GlobMatcher]) -> bool {
    !includes.iter().any(|matcher| matcher.is_match(name))
        || excludes.iter().any(|matcher| matcher.is_match(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn type_and_include_are_both_required() {
        let matchers = grep_matchers(&json!({"type":"rust","glob":"src/**"}))
            .unwrap_or_else(|_| panic!("matchers"));
        assert!(
            matchers
                .iter()
                .all(|matcher| matcher.is_match("src/lib.rs"))
        );
        assert!(
            !matchers
                .iter()
                .all(|matcher| matcher.is_match("tests/lib.rs"))
        );
        assert!(grep_matchers(&json!({"type":"unknown"})).is_err());
    }

    #[test]
    fn include_exclude_count_boundaries() {
        for count in [31, 32] {
            assert_eq!(validate_pattern_counts(count, count), Ok(()));
        }
        assert_eq!(validate_pattern_counts(33, 31), Err(FileError::Tool));
        assert_eq!(validate_pattern_counts(31, 33), Err(FileError::Tool));
    }

    #[test]
    fn regex_pattern_byte_boundaries() {
        assert_eq!(validate_regex_pattern_bytes(4_095), Ok(()));
        assert_eq!(validate_regex_pattern_bytes(4_096), Ok(()));
        assert_eq!(validate_regex_pattern_bytes(4_097), Err(FileError::Tool));
    }

    #[test]
    fn grep_context_boundaries() {
        assert_eq!(validate_context(99), Ok(()));
        assert_eq!(validate_context(100), Ok(()));
        assert_eq!(validate_context(101), Err(FileError::Tool));
    }

    #[test]
    fn grep_checked_limit_boundaries() {
        for maximum in [
            GREP_FILES_MAX,
            GREP_BYTES_MAX,
            GREP_LINES_MAX,
            GREP_MATCHES_MAX,
        ] {
            assert_eq!(checked_limit(maximum - 1, 0, maximum), Ok(maximum - 1));
            assert_eq!(checked_limit(maximum, 0, maximum), Ok(maximum));
            assert_eq!(checked_limit(maximum, 1, maximum), Err(FileError::Tool));
        }
    }

    #[test]
    fn read_many_aggregate_push_boundaries() {
        let mut output = String::with_capacity(READ_MANY_AGGREGATE_BYTES_MAX);
        output.push_str(&"x".repeat(READ_MANY_AGGREGATE_BYTES_MAX - 1));
        assert_eq!(push_read_many(&mut output, ""), Ok(()));
        assert_eq!(push_read_many(&mut output, "x"), Ok(()));
        assert_eq!(push_read_many(&mut output, "x"), Err(FileError::Tool));
    }
}
