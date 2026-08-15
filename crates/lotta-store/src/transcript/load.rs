//! Bounded baseline-compatible transcript loading.

use super::projection;
use super::{TranscriptPaths, bounds, manifest};
use crate::adapter::read_record;
use crate::atomic::FileRevision;
use crate::confinement::validate_regular_file;
use crate::{StoreError, StoreErrorKind};
use lotta_domain::{Conversation, LocalMessage, TranscriptManifest, TranscriptMessageFormat};
use serde_json::Value;
use std::io::Read as _;
use std::path::Path;

const NONEMPTY_PREFIX_BYTES_MAX: usize = 4_096;

/// Loaded active transcript projection and its canonical conversation snapshot.
#[derive(Clone, Debug)]
pub struct LoadedTranscript {
    manifest: Option<TranscriptManifest>,
    messages: Vec<LocalMessage>,
    conversation: Conversation,
    clipped_tool_results: bool,
    message_revision: FileRevision,
    manifest_revision: FileRevision,
}

impl LoadedTranscript {
    /// Returns the supported manifest, absent only for an empty unversioned transcript.
    #[must_use]
    pub const fn manifest(&self) -> Option<&TranscriptManifest> {
        self.manifest.as_ref()
    }

    /// Returns the active messages after baseline orphan repair and clipping.
    #[must_use]
    pub fn messages(&self) -> &[LocalMessage] {
        &self.messages
    }

    /// Returns the canonical conversation snapshot.
    #[must_use]
    pub const fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    /// Reports whether at least one oversized tool-result text part was clipped.
    #[must_use]
    pub const fn clipped_tool_results(&self) -> bool {
        self.clipped_tool_results
    }

    pub(crate) const fn message_revision(&self) -> &FileRevision {
        &self.message_revision
    }

    pub(crate) const fn manifest_revision(&self) -> &FileRevision {
        &self.manifest_revision
    }
}

pub(crate) fn load(paths: &TranscriptPaths) -> Result<LoadedTranscript, StoreError> {
    load_observed(paths, |_| Ok(()))
}

pub(crate) fn load_observed(
    paths: &TranscriptPaths,
    before_repair: impl FnOnce(&Path) -> Result<(), StoreError>,
) -> Result<LoadedTranscript, StoreError> {
    let manifest_revision = FileRevision::sample_path(&paths.manifest)?;
    let manifest = read_manifest(paths)?;
    ensure_revision(&paths.manifest, &manifest_revision, false)?;
    let conversation_revision = FileRevision::sample_path(&paths.conversation)?;
    let mut conversation: Conversation = read_record(&paths.conversation)?;
    ensure_revision(&paths.conversation, &conversation_revision, false)?;
    let message_revision =
        FileRevision::sample_path_bounded(&paths.messages, bounds::TRANSCRIPT_BYTES_MAX)?;
    let messages = read_messages(paths, manifest.as_ref())?;
    ensure_revision(&paths.messages, &message_revision, true)?;
    let projection = projection::active(
        messages,
        conversation.in_context_message_ids.as_slice(),
        &paths.messages,
    )?;
    before_repair(&paths.conversation)?;
    super::repair::persist_context(
        paths,
        &mut conversation,
        &conversation_revision,
        &projection,
    )?;
    Ok(LoadedTranscript {
        manifest,
        messages: projection.messages,
        conversation,
        clipped_tool_results: projection.clipped,
        message_revision,
        manifest_revision,
    })
}

pub(crate) fn load_search_nonmutating(
    paths: &TranscriptPaths,
    messages_max: usize,
) -> Result<Vec<LocalMessage>, StoreError> {
    if messages_max == 0 {
        return Err(StoreError::new(StoreErrorKind::Limit, &paths.messages));
    }
    let conversation_revision = FileRevision::sample_path(&paths.conversation)?;
    let conversation: Conversation = read_record(&paths.conversation)?;
    ensure_revision(&paths.conversation, &conversation_revision, false)?;
    let manifest_revision = FileRevision::sample_path(&paths.manifest)?;
    let format = match std::fs::symlink_metadata(&paths.manifest) {
        Ok(_) => manifest::read(&paths.manifest)
            .ok()
            .map(|value| value.message_format),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(StoreError::from_io(&paths.manifest, &error)),
    };
    ensure_revision(&paths.manifest, &manifest_revision, false)?;
    let message_revision =
        FileRevision::sample_path_bounded(&paths.messages, bounds::TRANSCRIPT_BYTES_MAX)?;
    let mut messages = Vec::new();
    messages
        .try_reserve(messages_max)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, &paths.messages))?;
    match std::fs::symlink_metadata(&paths.messages) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(messages),
        Err(error) => return Err(StoreError::from_io(&paths.messages, &error)),
        Ok(_) => {}
    }
    bounds::read_rows(&paths.root, &paths.messages, |_, row| {
        if row.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let Ok(value) = serde_json::from_slice::<Value>(row) else {
            return Ok(());
        };
        let parsed = match format {
            Some(TranscriptMessageFormat::PiSessionEntryJsonl) => {
                current_message(&value, &paths.messages).ok().flatten()
            }
            Some(TranscriptMessageFormat::PiAiMessageJsonl) => {
                parse_message(value, &paths.messages).ok()
            }
            None => current_message(&value, &paths.messages).ok().flatten(),
        };
        if let Some(message) = parsed {
            if messages.len() >= messages_max {
                return Err(StoreError::new(StoreErrorKind::Limit, &paths.messages));
            }
            messages.push(message);
        }
        Ok(())
    })?;
    ensure_revision(&paths.messages, &message_revision, true)?;
    projection::active(
        messages,
        conversation.in_context_message_ids.as_slice(),
        &paths.messages,
    )
    .map(|projection| projection.messages)
}

fn ensure_revision(
    path: &Path,
    expected: &FileRevision,
    transcript: bool,
) -> Result<(), StoreError> {
    let actual = if transcript {
        FileRevision::sample_path_bounded(path, bounds::TRANSCRIPT_BYTES_MAX)?
    } else {
        FileRevision::sample_path(path)?
    };
    if &actual == expected {
        Ok(())
    } else {
        Err(StoreError::new(StoreErrorKind::StorageConflict, path))
    }
}

fn read_messages(
    paths: &TranscriptPaths,
    manifest: Option<&TranscriptManifest>,
) -> Result<Vec<LocalMessage>, StoreError> {
    match manifest.map(|value| value.message_format) {
        None => Ok(Vec::new()),
        Some(TranscriptMessageFormat::PiAiMessageJsonl) => read_legacy(paths),
        Some(TranscriptMessageFormat::PiSessionEntryJsonl) => read_current(paths),
    }
}

fn read_manifest(paths: &TranscriptPaths) -> Result<Option<TranscriptManifest>, StoreError> {
    match std::fs::symlink_metadata(&paths.manifest) {
        Ok(_) => manifest::read(&paths.manifest).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if has_nonempty_jsonl(paths)? {
                Err(StoreError::new(
                    StoreErrorKind::TranscriptMigrationRequired,
                    &paths.messages,
                ))
            } else {
                Ok(None)
            }
        }
        Err(error) => Err(StoreError::from_io(&paths.manifest, &error)),
    }
}

fn has_nonempty_jsonl(paths: &TranscriptPaths) -> Result<bool, StoreError> {
    let metadata = match std::fs::symlink_metadata(&paths.messages) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(StoreError::from_io(&paths.messages, &error)),
        Ok(metadata) => metadata,
    };
    if metadata.len() == 0 {
        return Ok(false);
    }
    validate_regular_file(&paths.root, &paths.messages)?;
    if metadata.len() > NONEMPTY_PREFIX_BYTES_MAX as u64 {
        return Ok(true);
    }
    let file = std::fs::File::open(&paths.messages)
        .map_err(|error| StoreError::from_io(&paths.messages, &error))?;
    let mut prefix = Vec::new();
    prefix
        .try_reserve_exact(NONEMPTY_PREFIX_BYTES_MAX)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, &paths.messages))?;
    file.take(NONEMPTY_PREFIX_BYTES_MAX as u64)
        .read_to_end(&mut prefix)
        .map_err(|error| StoreError::from_io(&paths.messages, &error))?;
    Ok(prefix.iter().any(|byte| !byte.is_ascii_whitespace()))
}

fn read_current(paths: &TranscriptPaths) -> Result<Vec<LocalMessage>, StoreError> {
    let mut messages = Vec::new();
    bounds::read_rows_terminated(&paths.root, &paths.messages, |_, row, terminated| {
        let Some(value) = parse_recoverable_row(row, terminated, &paths.messages)? else {
            return Ok(());
        };
        if is_legacy_ui_row(&value) || embedded_legacy_ui_row(&value) {
            return Err(StoreError::new(
                StoreErrorKind::TranscriptRepairRequired,
                &paths.messages,
            ));
        }
        if let Some(message) = current_message(&value, &paths.messages)? {
            messages
                .try_reserve(1)
                .map_err(|_| StoreError::new(StoreErrorKind::Limit, &paths.messages))?;
            messages.push(message);
        }
        Ok(())
    })?;
    Ok(messages)
}

fn read_legacy(paths: &TranscriptPaths) -> Result<Vec<LocalMessage>, StoreError> {
    let mut messages = Vec::new();
    bounds::read_rows_terminated(&paths.root, &paths.messages, |_, row, terminated| {
        let Some(value) = parse_recoverable_row(row, terminated, &paths.messages)? else {
            return Ok(());
        };
        if is_legacy_ui_row(&value) {
            return Err(StoreError::new(
                StoreErrorKind::TranscriptRepairRequired,
                &paths.messages,
            ));
        }
        messages
            .try_reserve(1)
            .map_err(|_| StoreError::new(StoreErrorKind::Limit, &paths.messages))?;
        messages.push(parse_message(value, &paths.messages)?);
        Ok(())
    })?;
    Ok(messages)
}

fn parse_recoverable_row(
    row: &[u8],
    terminated: bool,
    path: &Path,
) -> Result<Option<Value>, StoreError> {
    if row.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    match serde_json::from_slice(row) {
        Ok(value) => Ok(Some(value)),
        Err(_) if !terminated => Ok(None),
        Err(_) => Err(StoreError::new(StoreErrorKind::Parse, path)),
    }
}

fn current_message(value: &Value, path: &Path) -> Result<Option<LocalMessage>, StoreError> {
    if !is_append_entry(value) {
        return Ok(None);
    }
    let message = value
        .get("message")
        .cloned()
        .ok_or_else(|| StoreError::new(StoreErrorKind::Parse, path))?;
    parse_message(message, path).map(Some)
}

fn is_append_entry(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let common = object.get("id").is_some_and(Value::is_string)
        && matches!(object.get("parentId"), Some(Value::Null | Value::String(_)))
        && object.get("timestamp").is_some_and(Value::is_string)
        && object.get("message").is_some_and(Value::is_object)
        && object
            .get("message")
            .and_then(Value::as_object)
            .and_then(|message| message.get("id"))
            .is_some_and(Value::is_string);
    match object.get("type").and_then(Value::as_str) {
        Some("message") => common,
        Some("compaction") => {
            common
                && object.get("summary").is_some_and(Value::is_string)
                && matches!(
                    object.get("firstKeptEntryId"),
                    Some(Value::Null | Value::String(_))
                )
                && object.get("tokensBefore").is_some_and(Value::is_number)
        }
        _ => false,
    }
}

fn parse_message(value: Value, path: &Path) -> Result<LocalMessage, StoreError> {
    serde_json::from_value(value).map_err(|_| StoreError::new(StoreErrorKind::Parse, path))
}

fn embedded_legacy_ui_row(value: &Value) -> bool {
    value.get("message").is_some_and(is_legacy_ui_row)
}

fn is_legacy_ui_row(value: &Value) -> bool {
    value.get("parts").is_some_and(Value::is_array)
        && value.get("content").is_none_or(Value::is_null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_row_detection_matches_baseline() {
        assert!(is_legacy_ui_row(&serde_json::json!({"parts": []})));
        assert!(embedded_legacy_ui_row(
            &serde_json::json!({"message": {"parts": [], "content": null}})
        ));
        assert!(!is_legacy_ui_row(&serde_json::json!({"content": []})));
    }
}

#[cfg(test)]
#[path = "load_error_mapping_tests.rs"]
mod error_mapping;
#[cfg(test)]
#[path = "load_upgrade_tests.rs"]
mod legacy_upgrade;
#[cfg(test)]
#[path = "load_recovery_tests.rs"]
mod recovery;
#[cfg(test)]
#[path = "load_tolerances_tests.rs"]
mod tolerances;
