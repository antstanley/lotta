use super::{
    super::{
        FileState,
        control::OperationControl,
        fs::{FileError, read_regular, workspace_relative},
    },
    parse::{Hunk, Operation, Patch},
};
use crate::builtin::file::operations::TEXT_FILE_BYTES_MAX;
use std::{collections::BTreeSet, path::PathBuf};

const PATCH_AGGREGATE_BYTES_MAX: usize = 64 * 1024 * 1024;
const SOURCE_LINES_MAX: usize = 100_000;

pub(super) struct Effect {
    pub(super) source: Option<PathBuf>,
    pub(super) destination: Option<PathBuf>,
    pub(super) content: Option<Vec<u8>>,
    pub(super) snapshot: Option<Vec<u8>>,
}

pub(super) fn preflight(
    state: &FileState,
    patch: Patch<'_>,
    control: &OperationControl,
) -> Result<Vec<Effect>, FileError> {
    let mut seen = BTreeSet::new();
    let mut effects = Vec::new();
    effects
        .try_reserve_exact(patch.operations.len())
        .map_err(|_| FileError::Tool)?;
    let mut aggregate = 0usize;
    for operation in patch.operations {
        control.check()?;
        let effect = preflight_operation(state, operation, control, &mut seen)?;
        aggregate = add_effect_bytes(aggregate, &effect)?;
        effects.push(effect);
    }
    Ok(effects)
}

fn preflight_operation(
    state: &FileState,
    operation: Operation<'_>,
    control: &OperationControl,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<Effect, FileError> {
    match operation {
        Operation::Add { path, lines } => preflight_add(state, path, &lines, control, seen),
        Operation::Delete { path } => preflight_delete(state, path, control, seen),
        Operation::Update {
            source,
            destination,
            hunks,
        } => preflight_update(state, source, destination, &hunks, control, seen),
    }
}

fn preflight_add(
    state: &FileState,
    path: &str,
    lines: &[&str],
    control: &OperationControl,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<Effect, FileError> {
    let destination = workspace_relative(state, path)?;
    unique(seen, &destination)?;
    let snapshot = match state.workspace.symlink_metadata(&destination) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Some(source_text(state, &destination, control)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(_) | Err(_) => return Err(FileError::Tool),
    };
    let source = snapshot.as_ref().map(|_| destination.clone());
    Ok(Effect {
        destination: source.is_none().then_some(destination),
        source,
        content: Some(join_lines(lines, true)?),
        snapshot,
    })
}

fn preflight_delete(
    state: &FileState,
    path: &str,
    control: &OperationControl,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<Effect, FileError> {
    let source = workspace_relative(state, path)?;
    unique(seen, &source)?;
    let snapshot = source_text(state, &source, control)?;
    Ok(Effect {
        source: Some(source),
        destination: None,
        content: None,
        snapshot: Some(snapshot),
    })
}

fn preflight_update(
    state: &FileState,
    source: &str,
    destination: Option<&str>,
    hunks: &[Hunk<'_>],
    control: &OperationControl,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<Effect, FileError> {
    let source = workspace_relative(state, source)?;
    unique(seen, &source)?;
    let destination = destination
        .map(|path| workspace_relative(state, path))
        .transpose()?;
    if let Some(path) = &destination {
        unique(seen, path)?;
        if path != &source && state.workspace.symlink_metadata(path).is_ok() {
            return Err(FileError::Tool);
        }
    }
    let snapshot = source_text(state, &source, control)?;
    let content = derive(&snapshot, hunks, control)?;
    Ok(Effect {
        source: Some(source),
        destination,
        content: Some(content),
        snapshot: Some(snapshot),
    })
}

fn add_effect_bytes(mut aggregate: usize, effect: &Effect) -> Result<usize, FileError> {
    if let Some(content) = &effect.content {
        aggregate = bounded_add(aggregate, content.len())?;
    }
    if let Some(snapshot) = &effect.snapshot {
        aggregate = bounded_add(aggregate, snapshot.len())?;
    }
    Ok(aggregate)
}

fn source_text(
    state: &FileState,
    path: &std::path::Path,
    control: &OperationControl,
) -> Result<Vec<u8>, FileError> {
    let bytes = read_regular(&state.workspace, path, TEXT_FILE_BYTES_MAX, control)?;
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        return Err(FileError::Tool);
    }
    Ok(bytes)
}

fn derive(
    original: &[u8],
    hunks: &[Hunk<'_>],
    control: &OperationControl,
) -> Result<Vec<u8>, FileError> {
    let text = std::str::from_utf8(original).map_err(|_| FileError::Tool)?;
    let final_newline = text.ends_with('\n');
    let source = text.strip_suffix('\n').unwrap_or(text);
    let count = source.split('\n').count();
    validate_source_lines(count)?;
    let mut lines = Vec::new();
    lines
        .try_reserve_exact(count)
        .map_err(|_| FileError::Tool)?;
    for line in source.split('\n') {
        control.check()?;
        lines.push(line.to_owned());
    }
    if text.is_empty() {
        lines.clear();
    }
    let mut cursor = 0usize;
    for hunk in hunks {
        control.check()?;
        if let Some(header) = hunk.header {
            cursor = seek_first(&lines, &[header], cursor, false)?
                .checked_add(1)
                .ok_or(FileError::Tool)?;
        }
        if hunk.old.is_empty() {
            let at = if hunk.eof {
                lines.len()
            } else {
                cursor.min(lines.len())
            };
            lines.splice(at..at, hunk.new.iter().map(|line| (*line).to_owned()));
            cursor = at.checked_add(hunk.new.len()).ok_or(FileError::Tool)?;
        } else {
            let at = seek_first(&lines, &hunk.old, cursor, hunk.eof)?;
            let end = at.checked_add(hunk.old.len()).ok_or(FileError::Tool)?;
            lines.splice(at..end, hunk.new.iter().map(|line| (*line).to_owned()));
            cursor = at.checked_add(hunk.new.len()).ok_or(FileError::Tool)?;
        }
    }
    join_owned_lines(&lines, final_newline)
}

fn seek_first(
    lines: &[String],
    pattern: &[&str],
    start: usize,
    eof: bool,
) -> Result<usize, FileError> {
    if pattern.is_empty() {
        return Ok(start);
    }
    let maximum = lines
        .len()
        .checked_sub(pattern.len())
        .ok_or(FileError::Tool)?;
    let first = if eof { maximum } else { start };
    for mode in 0..4 {
        for index in first..=maximum {
            if pattern
                .iter()
                .enumerate()
                .all(|(offset, pattern)| equivalent(&lines[index + offset], pattern, mode))
            {
                return Ok(index);
            }
        }
    }
    Err(FileError::Tool)
}

fn equivalent(left: &str, right: &str, mode: u8) -> bool {
    match mode {
        0 => left == right,
        1 => left.trim_end() == right.trim_end(),
        2 => left.trim() == right.trim(),
        _ => normalize(left) == normalize(right),
    }
}

fn normalize(value: &str) -> String {
    let value = value.trim();
    let mut output = String::new();
    if output.try_reserve_exact(value.len()).is_err() {
        return output;
    }
    for character in value.chars() {
        output.push(match character {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
            '\u{00a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            other => other,
        });
    }
    output
}

fn join_owned_lines(lines: &[String], final_newline: bool) -> Result<Vec<u8>, FileError> {
    let size = joined_size(lines.iter().map(String::as_str), final_newline)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| FileError::Tool)?;
    for (index, line) in lines.iter().enumerate() {
        if index != 0 {
            output.push(b'\n');
        }
        output.extend_from_slice(line.as_bytes());
    }
    if final_newline && !lines.is_empty() {
        output.push(b'\n');
    }
    Ok(output)
}

fn joined_size<'a>(
    lines: impl Iterator<Item = &'a str>,
    final_newline: bool,
) -> Result<usize, FileError> {
    let mut size = 0usize;
    let mut count = 0usize;
    for line in lines {
        size = size.checked_add(line.len()).ok_or(FileError::Tool)?;
        count = count.checked_add(1).ok_or(FileError::Tool)?;
    }
    let separators = count.saturating_sub(1) + usize::from(final_newline && count != 0);
    size = size.checked_add(separators).ok_or(FileError::Tool)?;
    validate_joined_text(size)?;
    Ok(size)
}

fn join_lines(lines: &[&str], final_newline: bool) -> Result<Vec<u8>, FileError> {
    let body = lines.join("\n");
    let append_newline = final_newline && !lines.is_empty();
    let size = body
        .len()
        .checked_add(usize::from(append_newline))
        .ok_or(FileError::Tool)?;
    validate_joined_text(size)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| FileError::Tool)?;
    output.extend_from_slice(body.as_bytes());
    if append_newline {
        output.push(b'\n');
    }
    Ok(output)
}

fn unique(seen: &mut BTreeSet<PathBuf>, path: &std::path::Path) -> Result<(), FileError> {
    if seen.insert(path.to_owned()) {
        Ok(())
    } else {
        Err(FileError::Tool)
    }
}

fn validate_limit(value: usize, maximum: usize) -> Result<(), FileError> {
    if value > maximum {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

fn validate_source_lines(value: usize) -> Result<(), FileError> {
    validate_limit(value, SOURCE_LINES_MAX)
}

fn validate_joined_text(value: usize) -> Result<(), FileError> {
    validate_limit(value, TEXT_FILE_BYTES_MAX)
}

fn validate_aggregate(value: usize) -> Result<(), FileError> {
    validate_limit(value, PATCH_AGGREGATE_BYTES_MAX)
}

fn bounded_add(total: usize, amount: usize) -> Result<usize, FileError> {
    let value = total.checked_add(amount).ok_or(FileError::Tool)?;
    validate_aggregate(value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_apply_boundaries() {
        for (validator, maximum) in [
            (
                validate_aggregate as fn(usize) -> _,
                PATCH_AGGREGATE_BYTES_MAX,
            ),
            (validate_source_lines as fn(usize) -> _, SOURCE_LINES_MAX),
            (validate_joined_text as fn(usize) -> _, TEXT_FILE_BYTES_MAX),
        ] {
            assert_eq!(validator(maximum - 1), Ok(()));
            assert_eq!(validator(maximum), Ok(()));
            assert_eq!(validator(maximum + 1), Err(FileError::Tool));
        }
    }
}
