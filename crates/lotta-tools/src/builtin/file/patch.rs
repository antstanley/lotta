#[path = "patch/apply.rs"]
mod apply_patch;
#[path = "patch/parse.rs"]
mod parse;
#[path = "patch/transaction.rs"]
pub(super) mod transaction;

use self::{apply_patch::preflight, parse::parse_patch, transaction::commit};
use super::{FileState, control::OperationControl, fs::FileError};
use serde_json::Value;

pub(super) fn apply(
    state: &FileState,
    input: &Value,
    control: &OperationControl,
) -> Result<String, FileError> {
    let patch = input
        .get("input")
        .and_then(Value::as_str)
        .ok_or(FileError::Tool)?;
    let parsed = parse_patch(patch, Some(control))?;
    let _guard = state
        .mutations
        .lock()
        .map_err(|_| FileError::Infrastructure)?;
    let plan = preflight(state, parsed, control)?;
    control.check()?;
    commit(&state.workspace, &plan)?;
    Ok("Done!".into())
}

/// Parses the complete bounded patch and returns every source and destination path.
pub(crate) fn scan_paths(patch: &str) -> Result<Vec<&str>, ()> {
    let parsed = parse_patch(patch, None).map_err(|_| ())?;
    parsed.paths().map_err(|_| ())
}
