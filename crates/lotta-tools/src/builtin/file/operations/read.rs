use super::{
    FileError, FileState, LINE_CHARS_MAX, LINES_DEFAULT, OperationControl, RESULTS_MAX,
    TEXT_FILE_BYTES_MAX, Value, file_type, integer, push_bounded, read_regular, string,
    workspace_relative,
};
use crate::builtin::file::glob::GlobMatcher;
use std::path::{Path, PathBuf};

pub(super) fn read(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
    gemini: bool,
) -> Result<String, FileError> {
    control.check()?;
    let path = workspace_relative(state, string(input, "file_path")?)?;
    let text = text_file(&state.workspace, &path, control)?;
    let supplied_offset = integer(input, "offset")?;
    let offset = if gemini {
        supplied_offset
            .map(|value| value.checked_add(1).ok_or(FileError::Tool))
            .transpose()?
            .unwrap_or(0)
    } else {
        supplied_offset.unwrap_or(0)
    };
    let supplied_limit = integer(input, "limit")?;
    let limit = supplied_limit.unwrap_or(LINES_DEFAULT).min(LINES_DEFAULT);
    render_read(
        &text,
        &resolved_path(state, &path),
        offset,
        limit,
        supplied_limit.is_some(),
        Some(control),
    )
}

pub(super) fn text_file(
    dir: &cap_std::fs::Dir,
    path: &Path,
    control: &OperationControl,
) -> Result<String, FileError> {
    let bytes = read_regular(dir, path, TEXT_FILE_BYTES_MAX, control)?;
    if bytes.contains(&0) {
        return Err(FileError::Tool);
    }
    String::from_utf8(bytes).map_err(|_| FileError::Tool)
}

pub(super) fn render_read(
    text: &str,
    path: &Path,
    offset: usize,
    limit: usize,
    explicit_limit: bool,
    control: Option<&OperationControl>,
) -> Result<String, FileError> {
    if text.trim().is_empty() {
        return Ok(empty_notice(path));
    }
    let lines: Vec<_> = text.split('\n').collect();
    let total = lines.len();
    let end = offset.saturating_add(limit).min(total);
    let mut output = String::new();
    let mut long = false;
    for (index, line) in lines.iter().enumerate().skip(offset).take(limit) {
        check(control)?;
        let retained = line.char_indices().nth(LINE_CHARS_MAX).map(|(i, _)| i);
        long |= retained.is_some();
        let line = retained.map_or(*line, |position| &line[..position]);
        if !output.is_empty() {
            push_bounded(&mut output, "\n")?;
        }
        push_bounded(&mut output, &format!("{}\t{}", index + 1, line))?;
        if retained.is_some() {
            push_bounded(&mut output, "... [line truncated]")?;
        }
    }
    append_notices(&mut output, long, offset, end, total, explicit_limit)?;
    Ok(output)
}

fn resolved_path(state: &FileState, path: &Path) -> PathBuf {
    state.workspace_root.join(path)
}

fn empty_notice(path: &Path) -> String {
    format!(
        "<system-reminder>\nThe file {} exists but has empty contents.\n</system-reminder>",
        path.display()
    )
}

fn append_notices(
    output: &mut String,
    long: bool,
    offset: usize,
    end: usize,
    total: usize,
    explicit_limit: bool,
) -> Result<(), FileError> {
    if end < total && !explicit_limit {
        let notice = format!(
            "\n\n[File truncated: showing lines {}-{end} of {total} total lines. \
             Use offset and limit parameters to read other sections.]",
            offset + 1
        );
        push_bounded(output, &notice)?;
    }
    if long {
        push_bounded(
            output,
            "\n\n[Some lines exceeded 2,000 characters and were truncated.]",
        )?;
    }
    Ok(())
}

fn check(control: Option<&OperationControl>) -> Result<(), FileError> {
    control.map_or(Ok(()), OperationControl::check)
}

pub(super) fn list(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    control.check()?;
    let key = if input.get("dir_path").is_some() {
        "dir_path"
    } else {
        "path"
    };
    let raw_path = string(input, key)?;
    let root_alias = raw_path == ".";
    let path = workspace_relative(state, if root_alias { ".lotta-root" } else { raw_path })?;
    let path = if root_alias {
        Path::new(".").to_path_buf()
    } else {
        path
    };
    if !path.as_os_str().is_empty() && !file_type(&state.workspace, &path)?.is_dir() {
        return Err(FileError::Tool);
    }
    let ignores = list_ignores(input, control)?;
    let mut entries = Vec::new();
    for entry in state
        .workspace
        .read_dir(&path)
        .map_err(|_| FileError::Tool)?
    {
        control.check()?;
        let entry = entry.map_err(|_| FileError::Tool)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| FileError::Tool)?;
        if ignores.iter().any(|matcher| matcher.is_match(&name)) {
            continue;
        }
        let kind = file_type(&state.workspace, &path.join(&name))?;
        if entries.len() == RESULTS_MAX {
            return Err(FileError::Tool);
        }
        entries.try_reserve(1).map_err(|_| FileError::Tool)?;
        entries.push((!kind.is_dir(), name));
    }
    render_entries(entries, control)
}

fn validate_list_ignore_count(count: usize) -> Result<(), FileError> {
    if count > 32 {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

fn list_ignores(input: &Value, control: &OperationControl) -> Result<Vec<GlobMatcher>, FileError> {
    let raw = match input.get("ignore") {
        Some(value) => value.as_array().ok_or(FileError::Tool)?.clone(),
        None => Vec::new(),
    };
    validate_list_ignore_count(raw.len())?;
    let mut ignores = Vec::new();
    ignores
        .try_reserve(raw.len())
        .map_err(|_| FileError::Tool)?;
    for value in raw {
        control.check()?;
        ignores.push(GlobMatcher::compile(
            value.as_str().ok_or(FileError::Tool)?,
            true,
        )?);
    }
    Ok(ignores)
}

fn render_entries(
    mut entries: Vec<(bool, String)>,
    control: &OperationControl,
) -> Result<String, FileError> {
    entries.sort();
    let mut output = String::new();
    for (file, name) in entries {
        control.check()?;
        if !output.is_empty() {
            push_bounded(&mut output, "\n")?;
        }
        push_bounded(&mut output, &name)?;
        if !file {
            push_bounded(&mut output, "/")?;
        }
    }
    if output.is_empty() {
        Ok("empty directory".into())
    } else {
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_ignore_count_boundaries() {
        assert_eq!(validate_list_ignore_count(31), Ok(()));
        assert_eq!(validate_list_ignore_count(32), Ok(()));
        assert_eq!(validate_list_ignore_count(33), Err(FileError::Tool));
    }

    #[test]
    fn render_read_per_line_char_boundaries() {
        for (count, truncated) in [(1_999, false), (2_000, false), (2_001, true)] {
            let output = render_read(&"x".repeat(count), Path::new("x"), 0, 2_000, false, None)
                .unwrap_or_else(|_| panic!("render"));
            assert_eq!(output.contains("... [line truncated]"), truncated);
            assert_eq!(output.contains("Some lines exceeded"), truncated);
        }
    }

    #[test]
    fn render_read_line_count_boundaries_default_notice() {
        for (count, notice) in [(1_999, false), (2_000, false), (2_001, true)] {
            let text = vec!["x"; count].join("\n");
            let output = render_read(&text, Path::new("x"), 0, 2_000, false, None)
                .unwrap_or_else(|_| panic!("render"));
            assert_eq!(output.contains("[File truncated:"), notice);
        }
    }

    #[test]
    fn render_read_line_count_boundaries_explicit_notice() {
        for count in [1_999, 2_000, 2_001] {
            let text = vec!["x"; count].join("\n");
            let output = render_read(&text, Path::new("x"), 0, 2_000, true, None)
                .unwrap_or_else(|_| panic!("render"));
            assert!(!output.contains("[File truncated:"));
        }
    }
}
