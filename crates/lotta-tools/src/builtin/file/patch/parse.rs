use super::super::{control::OperationControl, fs::FileError};

pub(super) const PATCH_BYTES_MAX: usize = 4 * 1024 * 1024;
const OPERATIONS_MAX: usize = 1_024;
const LINES_MAX: usize = 100_000;
const HUNKS_MAX: usize = 4_096;

#[derive(Debug)]
pub(super) struct Patch<'a> {
    pub(super) operations: Vec<Operation<'a>>,
}

#[derive(Debug)]
pub(super) enum Operation<'a> {
    Add {
        path: &'a str,
        lines: Vec<&'a str>,
    },
    Delete {
        path: &'a str,
    },
    Update {
        source: &'a str,
        destination: Option<&'a str>,
        hunks: Vec<Hunk<'a>>,
    },
}

#[derive(Debug)]
pub(super) struct Hunk<'a> {
    pub(super) header: Option<&'a str>,
    pub(super) old: Vec<&'a str>,
    pub(super) new: Vec<&'a str>,
    pub(super) eof: bool,
}

impl<'a> Patch<'a> {
    pub(super) fn paths(&self) -> Result<Vec<&'a str>, FileError> {
        let mut paths = Vec::new();
        paths
            .try_reserve_exact(self.operations.len().saturating_mul(2))
            .map_err(|_| FileError::Tool)?;
        for operation in &self.operations {
            match operation {
                Operation::Add { path, .. } | Operation::Delete { path } => paths.push(*path),
                Operation::Update {
                    source,
                    destination,
                    ..
                } => {
                    paths.push(*source);
                    if let Some(destination) = destination {
                        paths.push(*destination);
                    }
                }
            }
        }
        Ok(paths)
    }
}

pub(super) fn parse_patch<'a>(
    input: &'a str,
    control: Option<&OperationControl>,
) -> Result<Patch<'a>, FileError> {
    validate_patch_bytes(input.len())?;
    if input.contains('\0') {
        return Err(FileError::Tool);
    }
    let normalized = normalize(input)?;
    let lines = split_lines(normalized)?;
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        return Err(FileError::Tool);
    }
    parse_operations(&lines, control)
}

fn parse_operations<'a>(
    lines: &[&'a str],
    control: Option<&OperationControl>,
) -> Result<Patch<'a>, FileError> {
    let mut operations = Vec::new();
    operations.try_reserve(8).map_err(|_| FileError::Tool)?;
    let mut index = 1;
    let end = lines.len().checked_sub(1).ok_or(FileError::Tool)?;
    let mut hunk_count = 0usize;
    while index < end {
        check(control)?;
        validate_operations(operations.len().checked_add(1).ok_or(FileError::Tool)?)?;
        let (operation, next, added_hunks) = parse_operation(lines, index, end, control)?;
        hunk_count = hunk_count.checked_add(added_hunks).ok_or(FileError::Tool)?;
        validate_hunks(hunk_count)?;
        operations.push(operation);
        index = next;
    }
    if operations.is_empty() {
        Err(FileError::Tool)
    } else {
        Ok(Patch { operations })
    }
}

fn parse_operation<'a>(
    lines: &[&'a str],
    index: usize,
    end: usize,
    control: Option<&OperationControl>,
) -> Result<(Operation<'a>, usize, usize), FileError> {
    let line = lines[index];
    if let Some(path) = line.strip_prefix("*** Add File: ") {
        return parse_add(lines, index + 1, end, path, control);
    }
    if let Some(path) = line.strip_prefix("*** Delete File: ") {
        valid_path(path)?;
        return Ok((Operation::Delete { path }, index + 1, 0));
    }
    let source = line
        .strip_prefix("*** Update File: ")
        .ok_or(FileError::Tool)?;
    parse_update(lines, index + 1, end, source, control)
}

fn parse_add<'a>(
    lines: &[&'a str],
    mut index: usize,
    end: usize,
    path: &'a str,
    control: Option<&OperationControl>,
) -> Result<(Operation<'a>, usize, usize), FileError> {
    valid_path(path)?;
    let mut content = Vec::new();
    while index < end && lines[index].starts_with('+') {
        check(control)?;
        if content.len() >= LINES_MAX {
            return Err(FileError::Tool);
        }
        content.try_reserve(1).map_err(|_| FileError::Tool)?;
        content.push(&lines[index][1..]);
        index += 1;
    }
    Ok((
        Operation::Add {
            path,
            lines: content,
        },
        index,
        0,
    ))
}

fn parse_update<'a>(
    lines: &[&'a str],
    mut index: usize,
    end: usize,
    source: &'a str,
    control: Option<&OperationControl>,
) -> Result<(Operation<'a>, usize, usize), FileError> {
    valid_path(source)?;
    let destination = lines
        .get(index)
        .and_then(|line| line.strip_prefix("*** Move to: "));
    if let Some(path) = destination {
        valid_path(path)?;
        index += 1;
    }
    let mut hunks = Vec::new();
    while index < end && !lines[index].starts_with("*** ") {
        check(control)?;
        let (hunk, next) = parse_hunk(lines, index, end, hunks.is_empty(), control)?;
        hunks.try_reserve(1).map_err(|_| FileError::Tool)?;
        hunks.push(hunk);
        index = next;
    }
    if hunks.is_empty() {
        return Err(FileError::Tool);
    }
    let count = hunks.len();
    Ok((
        Operation::Update {
            source,
            destination,
            hunks,
        },
        index,
        count,
    ))
}

fn parse_hunk<'a>(
    lines: &[&'a str],
    mut index: usize,
    end: usize,
    allow_missing_marker: bool,
    control: Option<&OperationControl>,
) -> Result<(Hunk<'a>, usize), FileError> {
    let header = if lines[index] == "@@" {
        index += 1;
        None
    } else if let Some(value) = lines[index].strip_prefix("@@ ") {
        index += 1;
        Some(value)
    } else if allow_missing_marker {
        None
    } else {
        return Err(FileError::Tool);
    };
    let mut old = Vec::new();
    let mut new = Vec::new();
    let mut count = 0usize;
    let mut eof = false;
    while index < end {
        check(control)?;
        let line = lines[index];
        if line == "*** End of File" {
            if count == 0 {
                return Err(FileError::Tool);
            }
            eof = true;
            index += 1;
            break;
        }
        if count >= LINES_MAX {
            return Err(FileError::Tool);
        }
        old.try_reserve(1).map_err(|_| FileError::Tool)?;
        new.try_reserve(1).map_err(|_| FileError::Tool)?;
        let prefix = line.as_bytes().first().copied().unwrap_or(b' ');
        match prefix {
            b' ' if line.is_empty() => {
                old.push("");
                new.push("");
            }
            b' ' => {
                old.push(&line[1..]);
                new.push(&line[1..]);
            }
            b'+' => new.push(&line[1..]),
            b'-' => old.push(&line[1..]),
            _ => break,
        }
        count = count.checked_add(1).ok_or(FileError::Tool)?;
        index += 1;
    }
    if count == 0 {
        return Err(FileError::Tool);
    }
    Ok((
        Hunk {
            header,
            old,
            new,
            eof,
        },
        index,
    ))
}

fn split_lines(input: &str) -> Result<Vec<&str>, FileError> {
    let count = input.split('\n').count();
    validate_lines(count)?;
    let mut lines = Vec::new();
    lines
        .try_reserve_exact(count)
        .map_err(|_| FileError::Tool)?;
    lines.extend(
        input
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line)),
    );
    Ok(lines)
}

fn validate_limit(value: usize, maximum: usize) -> Result<(), FileError> {
    if value > maximum {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

fn validate_patch_bytes(value: usize) -> Result<(), FileError> {
    validate_limit(value, PATCH_BYTES_MAX)
}

fn validate_lines(value: usize) -> Result<(), FileError> {
    validate_limit(value, LINES_MAX)
}

fn validate_operations(value: usize) -> Result<(), FileError> {
    validate_limit(value, OPERATIONS_MAX)
}

fn validate_hunks(value: usize) -> Result<(), FileError> {
    validate_limit(value, HUNKS_MAX)
}

fn normalize(input: &str) -> Result<&str, FileError> {
    let trimmed = input.trim_matches([' ', '\t', '\n', '\r']);
    if trimmed.starts_with("*** Begin Patch") {
        return Ok(trimmed);
    }
    for prefix in [
        "<<EOF\n",
        "<<'EOF'\n",
        "<<\"EOF\"\n",
        "<<EOF\r\n",
        "<<'EOF'\r\n",
        "<<\"EOF\"\r\n",
    ] {
        if let Some(body) = trimmed.strip_prefix(prefix)
            && let Some(inner) = body
                .strip_suffix("\nEOF")
                .or_else(|| body.strip_suffix("\r\nEOF"))
        {
            return Ok(inner);
        }
    }
    Err(FileError::Tool)
}

fn valid_path(path: &str) -> Result<(), FileError> {
    if path.is_empty() || path.trim() != path {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

fn check(control: Option<&OperationControl>) -> Result<(), FileError> {
    if let Some(control) = control {
        control.check()
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    #[test]
    fn patch_envelope_and_count_boundaries() {
        for (validator, maximum) in [
            (validate_patch_bytes as fn(usize) -> _, PATCH_BYTES_MAX),
            (validate_lines as fn(usize) -> _, LINES_MAX),
            (validate_operations as fn(usize) -> _, OPERATIONS_MAX),
            (validate_hunks as fn(usize) -> _, HUNKS_MAX),
        ] {
            assert_eq!(validator(maximum - 1), Ok(()));
            assert_eq!(validator(maximum), Ok(()));
            assert_eq!(validator(maximum + 1), Err(FileError::Tool));
        }
    }

    fn operation_patch(count: usize) -> String {
        let mut patch = String::from("*** Begin Patch\n");
        for index in 0..count {
            writeln!(patch, "*** Delete File: file-{index}").unwrap_or_else(|_| panic!("patch"));
        }
        patch.push_str("*** End Patch");
        patch
    }

    #[test]
    fn parser_operation_boundaries() {
        for count in [1_023, 1_024] {
            let patch = operation_patch(count);
            assert_eq!(
                parse_patch(&patch, None).map(|value| value.operations.len()),
                Ok(count)
            );
        }
        assert!(parse_patch(&operation_patch(1_025), None).is_err());
    }
}
