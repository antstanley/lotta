use super::read::text_file;
use super::{
    EDITS_MAX, FileError, FileState, OperationControl, TEXT_FILE_BYTES_MAX, Value, atomic_write,
    integer, string, workspace_relative,
};

pub(super) fn write(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    let path = workspace_relative(state, string(input, "file_path")?)?;
    let content = string(input, "content")?;
    if content.len() > TEXT_FILE_BYTES_MAX {
        return Err(FileError::Tool);
    }
    let _guard = state.mutations.lock().map_err(|_| FileError::Tool)?;
    control.check()?;
    atomic_write(&state.workspace, &path, content.as_bytes())?;
    Ok("File written successfully.".into())
}

pub(super) fn edit(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    let path = workspace_relative(state, string(input, "file_path")?)?;
    let text = text_file(&state.workspace, &path, control)?;
    let old = string(input, "old_string")?;
    let new = string(input, "new_string")?;
    let replace_all = input
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let expected = integer(input, "expected_replacements")?;
    let output = replace_text_controlled(&text, old, new, replace_all, expected, control)?;
    let _guard = state.mutations.lock().map_err(|_| FileError::Tool)?;
    control.check()?;
    atomic_write(&state.workspace, &path, output.as_bytes())?;
    Ok("File edited successfully.".into())
}

pub(super) fn multi_edit(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    let path = workspace_relative(state, string(input, "file_path")?)?;
    let mut text = text_file(&state.workspace, &path, control)?;
    let edits = input
        .get("edits")
        .and_then(Value::as_array)
        .ok_or(FileError::Tool)?;
    validate_edit_count(edits.len())?;
    for edit in edits {
        control.check()?;
        text = replace_text_controlled(
            &text,
            string(edit, "old_string")?,
            string(edit, "new_string")?,
            edit.get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            None,
            control,
        )?;
    }
    let _guard = state.mutations.lock().map_err(|_| FileError::Tool)?;
    control.check()?;
    atomic_write(&state.workspace, &path, text.as_bytes())?;
    Ok("File edited successfully.".into())
}

pub(super) fn replace_text_controlled(
    text: &str,
    old: &str,
    new: &str,
    all: bool,
    expected: Option<usize>,
    control: &OperationControl,
) -> Result<String, FileError> {
    control.check()?;
    let normalized = normalize_crlf(text)?;
    let old = normalize_crlf(old)?;
    let new = normalize_crlf(new)?;
    if old.is_empty() || old == new {
        return Err(FileError::Tool);
    }
    let mut count = 0usize;
    for _ in normalized.match_indices(&old) {
        control.check()?;
        count = count.checked_add(1).ok_or(FileError::Tool)?;
    }
    let wanted = expected.unwrap_or(if all { count } else { 1 });
    if count == 0 || count != wanted || (!all && expected.is_none() && count != 1) {
        return Err(FileError::Tool);
    }
    assemble_replacement(&normalized, &old, &new, wanted, control)
}

fn normalize_crlf(value: &str) -> Result<String, FileError> {
    let mut output = String::new();
    output
        .try_reserve_exact(value.len())
        .map_err(|_| FileError::Tool)?;
    let mut rest = value;
    while let Some(index) = rest.find("\r\n") {
        output.push_str(&rest[..index]);
        output.push('\n');
        rest = &rest[index + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

fn validate_edit_count(count: usize) -> Result<(), FileError> {
    if count == 0 || count > EDITS_MAX {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

fn validate_replacement_capacity(capacity: usize) -> Result<(), FileError> {
    if capacity > TEXT_FILE_BYTES_MAX {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

fn assemble_replacement(
    text: &str,
    old: &str,
    new: &str,
    wanted: usize,
    control: &OperationControl,
) -> Result<String, FileError> {
    let removed = old.len().checked_mul(wanted).ok_or(FileError::Tool)?;
    let added = new.len().checked_mul(wanted).ok_or(FileError::Tool)?;
    let capacity = text
        .len()
        .checked_sub(removed)
        .and_then(|n| n.checked_add(added))
        .ok_or(FileError::Tool)?;
    validate_replacement_capacity(capacity)?;
    let mut output = String::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| FileError::Tool)?;
    let mut rest = text;
    for _ in 0..wanted {
        control.check()?;
        let index = rest.find(old).ok_or(FileError::Tool)?;
        output.push_str(&rest[..index]);
        output.push_str(new);
        rest = &rest[index + old.len()..];
    }
    output.push_str(rest);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn control() -> OperationControl {
        OperationControl::new(CancellationToken::new(), Duration::from_secs(1))
            .unwrap_or_else(|_| panic!("control"))
    }

    #[test]
    fn edit_count_boundaries() {
        assert_eq!(validate_edit_count(1_023), Ok(()));
        assert_eq!(validate_edit_count(1_024), Ok(()));
        assert_eq!(validate_edit_count(1_025), Err(FileError::Tool));
        assert_eq!(validate_edit_count(0), Err(FileError::Tool));
    }

    #[test]
    fn replacement_capacity_boundaries() {
        assert_eq!(
            validate_replacement_capacity(TEXT_FILE_BYTES_MAX - 1),
            Ok(())
        );
        assert_eq!(validate_replacement_capacity(TEXT_FILE_BYTES_MAX), Ok(()));
        assert_eq!(
            validate_replacement_capacity(TEXT_FILE_BYTES_MAX + 1),
            Err(FileError::Tool)
        );
    }

    #[test]
    fn crlf_count_and_replacement_share_normalization() {
        let value = replace_text_controlled(
            "a\r\nb\r\na\r\nb",
            "a\nb",
            "x\r\ny",
            true,
            Some(2),
            &control(),
        );
        assert_eq!(value, Ok("x\ny\nx\ny".into()));
    }
}
