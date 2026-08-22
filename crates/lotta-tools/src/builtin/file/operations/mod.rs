use super::{
    FileState,
    control::OperationControl,
    fs::{FileError, artifact_relative, atomic_write, file_type, read_regular, workspace_relative},
};
use serde_json::Value;

mod artifact;
mod mutate;
mod read;
mod search;

/// Maximum ordinary text or artifact file size in bytes.
pub const TEXT_FILE_BYTES_MAX: usize = 10 * 1024 * 1024;
/// Maximum supported image input size in bytes.
pub const IMAGE_BYTES_MAX: usize = 20 * 1024 * 1024;
pub(super) const PATH_BYTES_MAX: usize = 4_096;
pub(super) const PATH_COMPONENTS_MAX: usize = 256;
pub(super) const DIRECTORY_DEPTH_MAX: usize = 64;
pub(super) const DIRECTORY_ENTRIES_MAX: usize = 100_000;
pub(super) const RESULTS_MAX: usize = 10_000;
const OUTPUT_BYTES_MAX: usize = 1024 * 1024;
const LINES_DEFAULT: usize = 2_000;
const LINE_CHARS_MAX: usize = 2_000;
const EDITS_MAX: usize = 1_024;
const READ_MANY_AGGREGATE_BYTES_MAX: usize = 900_000;

pub(super) fn execute(
    state: &FileState,
    name: &str,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    control.check()?;
    match name {
        "Read" => read::read(state, input, control, false),
        "read_file_gemini" => read::read(state, input, control, true),
        "Write" | "write_file_gemini" => mutate::write(state, input, control),
        "Edit" | "replace" => mutate::edit(state, input, control),
        "MultiEdit" => mutate::multi_edit(state, input, control),
        "LS" | "list_directory" => read::list(state, input, control),
        "Glob" | "glob_gemini" => search::glob(state, input, control),
        "Grep" | "search_file_content" => search::grep(state, input, control),
        "read_many_files" => search::read_many(state, input, control),
        "view_image" => artifact::view_image(state, input, control),
        "read_artifact_file" => artifact::read_artifact(state, input, control),
        "write_artifact_file" => artifact::write_artifact(state, input, control),
        "apply_patch" => super::patch::apply(state, input, control),
        _ => Err(FileError::Tool),
    }
}

/// Raw, unformatted workspace byte read for listener file services.
///
/// Reuses the exact tool confinement (`workspace_relative`) and no-follow
/// bounded read (`read_regular`) used by the `Read` tool executor.
pub(super) fn read_raw(
    state: &FileState,
    value: &str,
    byte_max: usize,
    control: &OperationControl,
) -> Result<Vec<u8>, FileError> {
    let relative = workspace_relative(state, value)?;
    read_regular(&state.workspace, &relative, byte_max, control)
}

/// Raw full-content create-or-replace write for listener file services.
pub(super) fn write_raw(
    state: &FileState,
    value: &str,
    content: &str,
    control: &OperationControl,
) -> Result<(), FileError> {
    mutate::write_core(state, value, content, control)
}

/// Raw tool-`Edit`-semantics replacement for listener file services.
///
/// Returns `(replacements, start_line)` for the pinned response shape.
pub(super) fn edit_raw(
    state: &FileState,
    value: &str,
    old: &str,
    new: &str,
    replace_all: bool,
    expected_replacements: Option<usize>,
    control: &OperationControl,
) -> Result<(usize, usize), FileError> {
    mutate::edit_core(
        state,
        value,
        old,
        new,
        replace_all,
        expected_replacements,
        control,
    )
    .map(|outcome| (outcome.replacements, outcome.start_line))
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, FileError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(FileError::Tool)
}
fn integer(value: &Value, key: &str) -> Result<Option<usize>, FileError> {
    value
        .get(key)
        .map(|item| {
            item.as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or(FileError::Tool)
        })
        .transpose()
}
fn encoded_len(length: usize) -> Result<usize, FileError> {
    length
        .checked_add(2)
        .and_then(|v| v.checked_div(3))
        .and_then(|v| v.checked_mul(4))
        .ok_or(FileError::Tool)
}
fn decoded_upper_bound(length: usize) -> Result<usize, FileError> {
    length
        .checked_add(3)
        .and_then(|v| v.checked_div(4))
        .and_then(|v| v.checked_mul(3))
        .ok_or(FileError::Tool)
}
fn bounded(value: String) -> Result<String, FileError> {
    if value.len() > OUTPUT_BYTES_MAX {
        Err(FileError::Tool)
    } else {
        Ok(value)
    }
}
fn push_bounded(output: &mut String, value: &str) -> Result<(), FileError> {
    push_bounded_limit(output, value, OUTPUT_BYTES_MAX)
}
fn push_bounded_limit(output: &mut String, value: &str, maximum: usize) -> Result<(), FileError> {
    if output
        .len()
        .checked_add(value.len())
        .ok_or(FileError::Tool)?
        > maximum
    {
        return Err(FileError::Tool);
    }
    output.push_str(value);
    Ok(())
}
