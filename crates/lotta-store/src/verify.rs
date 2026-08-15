//! Non-mutating strict transcript diagnostics.

use crate::confinement::{validate_existing, validate_regular_file};
use crate::transcript::{self, TranscriptPaths};
use crate::{ConversationKey, StoreError, StoreErrorKind, StorePaths};
use lotta_domain::{LocalMessage, LocalMessageRole};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Maximum canonical conversation directories inspected by one verification.
pub const VERIFY_CONVERSATIONS_MAX: usize = 100_000;
/// Maximum findings retained by one verification.
pub const VERIFY_FINDINGS_MAX: usize = 1_000_000;
/// Maximum rows and graph identifiers retained per transcript.
pub const VERIFY_ROWS_MAX: usize = 1_000_000;

/// Strict diagnostic category, ordered in stable report order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum VerificationClass {
    /// Two canonical rows carry the same entry identifier.
    DuplicateEntryId,
    /// A parent link is absent from the preceding row set or otherwise invalid.
    InvalidParentLink,
    /// The session header is absent, repeated, or not row one.
    InvalidSessionHeader,
    /// A tool result has no matching immediate assistant tool-call window.
    OrphanToolResult,
    /// A compaction retained-entry reference is invalid.
    InvalidCompactionReference,
    /// A canonical row has fields outside its exact outer wire schema.
    UnsupportedOuterField,
}

impl VerificationClass {
    /// Returns the stable CLI identifier.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::DuplicateEntryId => "duplicate_entry_id",
            Self::InvalidParentLink => "invalid_parent_link",
            Self::InvalidSessionHeader => "invalid_session_header",
            Self::OrphanToolResult => "orphan_tool_result",
            Self::InvalidCompactionReference => "invalid_compaction_reference",
            Self::UnsupportedOuterField => "unsupported_outer_field",
        }
    }
}

/// One content-free strict diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationFinding {
    /// Canonical transcript path.
    pub path: PathBuf,
    /// One-based JSONL row number.
    pub row: usize,
    /// Stable diagnostic class.
    pub class: VerificationClass,
}

/// Deterministic full-root verification report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationReport {
    /// Explicit storage root.
    pub storage_root: PathBuf,
    /// Number of canonical conversations checked.
    pub conversations_checked: usize,
    /// Number of JSONL rows checked.
    pub rows_checked: usize,
    /// Findings sorted by path, row, and class.
    pub findings: Vec<VerificationFinding>,
}

/// Strictly verifies every canonical current transcript without mutation.
///
/// This operation takes no storage lock and performs no repair, rewrite, backup, or persistence.
///
/// # Errors
/// Returns typed confinement, malformed/unsupported input, bounds, allocation, or I/O failures.
pub fn verify_transcripts(
    storage_root: impl Into<PathBuf>,
) -> Result<VerificationReport, StoreError> {
    let paths = StorePaths::new(storage_root.into())?;
    let directories = conversation_directories(&paths)?;
    let mut report = VerificationReport {
        storage_root: paths.root().to_path_buf(),
        conversations_checked: 0,
        rows_checked: 0,
        findings: Vec::new(),
    };
    for directory in directories {
        verify_one(&paths, directory, &mut report)?;
        report.conversations_checked = report
            .conversations_checked
            .checked_add(1)
            .ok_or_else(|| limit(paths.root()))?;
    }
    report.findings.sort_by(|left, right| {
        (&left.path, left.row, left.class).cmp(&(&right.path, right.row, right.class))
    });
    Ok(report)
}

fn conversation_directories(paths: &StorePaths) -> Result<Vec<PathBuf>, StoreError> {
    let parent = paths.conversations();
    match std::fs::symlink_metadata(&parent) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(StoreError::from_io(&parent, &error)),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(StoreError::new(StoreErrorKind::InvalidPath, parent)),
    }
    validate_existing(paths.root(), &parent)?;
    let mut output = Vec::new();
    let mut scanned = 0_usize;
    for item in std::fs::read_dir(&parent).map_err(|error| StoreError::from_io(&parent, &error))? {
        scanned = scanned.checked_add(1).ok_or_else(|| limit(&parent))?;
        if scanned > VERIFY_CONVERSATIONS_MAX {
            return Err(limit(&parent));
        }
        let item = item.map_err(|error| StoreError::from_io(&parent, &error))?;
        let kind = item
            .file_type()
            .map_err(|error| StoreError::from_io(&item.path(), &error))?;
        if !kind.is_dir() {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, item.path()));
        }
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| StoreError::new(StoreErrorKind::InvalidPath, item.path()))?;
        ConversationKey::decode(&name)?;
        output.try_reserve(1).map_err(|_| limit(&parent))?;
        output.push(item.path());
    }
    output.sort();
    Ok(output)
}

fn verify_one(
    store: &StorePaths,
    directory: PathBuf,
    report: &mut VerificationReport,
) -> Result<(), StoreError> {
    validate_existing(store.root(), &directory)?;
    let paths = TranscriptPaths {
        root: store.root().to_path_buf(),
        conversation: directory.join("conversation.json"),
        manifest: directory.join("manifest.json"),
        messages: directory.join("messages.jsonl"),
        directory,
    };
    validate_regular_file(&paths.root, &paths.manifest)?;
    transcript::manifest::read_current(&paths.manifest)?;
    validate_regular_file(&paths.root, &paths.messages)?;
    let mut state = VerifyState::new(&paths.messages);
    transcript::bounds::read_rows_terminated(
        &paths.root,
        &paths.messages,
        |row, bytes, terminated| state.inspect(row, bytes, terminated),
    )?;
    state.finish()?;
    report.rows_checked = report
        .rows_checked
        .checked_add(state.rows)
        .ok_or_else(|| limit(&paths.messages))?;
    append_findings(&mut report.findings, state.findings, &paths.messages)
}

fn append_findings(
    target: &mut Vec<VerificationFinding>,
    source: Vec<VerificationFinding>,
    path: &Path,
) -> Result<(), StoreError> {
    let total = target
        .len()
        .checked_add(source.len())
        .ok_or_else(|| limit(path))?;
    if total > VERIFY_FINDINGS_MAX {
        return Err(limit(path));
    }
    target.try_reserve(source.len()).map_err(|_| limit(path))?;
    target.extend(source);
    Ok(())
}

struct VerifyState<'a> {
    path: &'a Path,
    seen: BTreeSet<String>,
    graph_rows: BTreeSet<String>,
    message_rows: BTreeSet<String>,
    pending_tools: BTreeSet<String>,
    session_id: Option<String>,
    findings: Vec<VerificationFinding>,
    rows: usize,
    headers: usize,
}

impl<'a> VerifyState<'a> {
    fn new(path: &'a Path) -> Self {
        Self {
            path,
            seen: BTreeSet::new(),
            graph_rows: BTreeSet::new(),
            message_rows: BTreeSet::new(),
            pending_tools: BTreeSet::new(),
            session_id: None,
            findings: Vec::new(),
            rows: 0,
            headers: 0,
        }
    }

    fn inspect(&mut self, row: usize, bytes: &[u8], terminated: bool) -> Result<(), StoreError> {
        enforce_row_bound(self.rows, self.path)?;
        if !terminated || bytes.iter().all(u8::is_ascii_whitespace) {
            return Err(StoreError::new(StoreErrorKind::Parse, self.path));
        }
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|_| StoreError::new(StoreErrorKind::Parse, self.path))?;
        let object = value
            .as_object()
            .ok_or_else(|| StoreError::new(StoreErrorKind::Parse, self.path))?;
        self.rows = self.rows.checked_add(1).ok_or_else(|| limit(self.path))?;
        match object.get("type").and_then(Value::as_str) {
            Some("session") => self.inspect_session(row, object)?,
            Some("message") => self.inspect_message(row, object)?,
            Some("compaction") => self.inspect_compaction(row, object)?,
            _ => self.pending_tools.clear(),
        }
        Ok(())
    }

    fn inspect_session(
        &mut self,
        row: usize,
        object: &Map<String, Value>,
    ) -> Result<(), StoreError> {
        self.headers = self
            .headers
            .checked_add(1)
            .ok_or_else(|| limit(self.path))?;
        let valid = object.get("version").and_then(Value::as_u64) == Some(3)
            && object.get("id").is_some_and(Value::is_string)
            && object.get("timestamp").is_some_and(Value::is_string)
            && object.get("cwd").is_some_and(Value::is_string);
        if row != 1 || self.headers > 1 || !valid {
            self.add(row, VerificationClass::InvalidSessionHeader)?;
        }
        if self.session_id.is_none() {
            self.session_id = object.get("id").and_then(Value::as_str).map(str::to_owned);
        }
        self.inspect_id(row, object)?;
        self.unsupported(row, object, &SESSION_FIELDS)?;
        self.pending_tools.clear();
        Ok(())
    }

    fn inspect_message(
        &mut self,
        row: usize,
        object: &Map<String, Value>,
    ) -> Result<(), StoreError> {
        let entry: lotta_domain::MessageEntry =
            serde_json::from_value(Value::Object(object.clone()))
                .map_err(|_| StoreError::new(StoreErrorKind::Parse, self.path))?;
        self.inspect_parent(row, object)?;
        self.inspect_id(row, object)?;
        self.unsupported(row, object, &MESSAGE_FIELDS)?;
        self.inspect_tool_window(row, &entry.message)?;
        insert_bounded(&mut self.message_rows, entry.id.as_str(), self.path)?;
        insert_bounded(&mut self.graph_rows, entry.id.as_str(), self.path)?;
        Ok(())
    }

    fn inspect_compaction(
        &mut self,
        row: usize,
        object: &Map<String, Value>,
    ) -> Result<(), StoreError> {
        let entry: lotta_domain::CompactionEntry =
            serde_json::from_value(Value::Object(object.clone()))
                .map_err(|_| StoreError::new(StoreErrorKind::Parse, self.path))?;
        self.inspect_parent(row, object)?;
        self.inspect_id(row, object)?;
        self.unsupported(row, object, &COMPACTION_FIELDS)?;
        let reference = entry.first_kept_entry_id.as_deref();
        if reference.is_none_or(|id| id == entry.id.as_str() || !self.message_rows.contains(id)) {
            self.add(row, VerificationClass::InvalidCompactionReference)?;
        }
        self.pending_tools.clear();
        insert_bounded(&mut self.graph_rows, entry.id.as_str(), self.path)?;
        Ok(())
    }

    fn inspect_id(&mut self, row: usize, object: &Map<String, Value>) -> Result<(), StoreError> {
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| StoreError::new(StoreErrorKind::Parse, self.path))?;
        if self.seen.len() >= VERIFY_ROWS_MAX {
            return Err(limit(self.path));
        }
        if !self.seen.insert(id.to_owned()) {
            self.add(row, VerificationClass::DuplicateEntryId)?;
        }
        Ok(())
    }

    fn inspect_parent(
        &mut self,
        row: usize,
        object: &Map<String, Value>,
    ) -> Result<(), StoreError> {
        let id = object.get("id").and_then(Value::as_str);
        let parent = object.get("parentId");
        let invalid = match parent {
            Some(Value::Null) => !self.graph_rows.is_empty(),
            Some(Value::String(value)) => {
                Some(value.as_str()) == id
                    || self.session_id.as_deref() == Some(value.as_str())
                    || !self.graph_rows.contains(value)
            }
            _ => true,
        };
        if invalid {
            self.add(row, VerificationClass::InvalidParentLink)?;
        }
        Ok(())
    }

    fn inspect_tool_window(
        &mut self,
        row: usize,
        message: &LocalMessage,
    ) -> Result<(), StoreError> {
        match message.role {
            LocalMessageRole::Assistant => {
                self.pending_tools.clear();
                if assistant_contributes(message) {
                    collect_tool_calls(message, &mut self.pending_tools);
                }
            }
            LocalMessageRole::User => self.pending_tools.clear(),
            LocalMessageRole::ToolResult => {
                if !tool_result_matches(message, &self.pending_tools) {
                    self.add(row, VerificationClass::OrphanToolResult)?;
                }
            }
        }
        Ok(())
    }

    fn unsupported(
        &mut self,
        row: usize,
        object: &Map<String, Value>,
        allowed: &[&str],
    ) -> Result<(), StoreError> {
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            self.add(row, VerificationClass::UnsupportedOuterField)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), StoreError> {
        if self.headers == 0 {
            self.add(0, VerificationClass::InvalidSessionHeader)?;
        }
        Ok(())
    }

    fn add(&mut self, row: usize, class: VerificationClass) -> Result<(), StoreError> {
        if self.findings.len() >= VERIFY_FINDINGS_MAX {
            return Err(limit(self.path));
        }
        self.findings.try_reserve(1).map_err(|_| limit(self.path))?;
        self.findings.push(VerificationFinding {
            path: self.path.to_path_buf(),
            row,
            class,
        });
        Ok(())
    }
}

const SESSION_FIELDS: [&str; 5] = ["type", "version", "id", "timestamp", "cwd"];
const MESSAGE_FIELDS: [&str; 5] = ["type", "id", "parentId", "timestamp", "message"];
const COMPACTION_FIELDS: [&str; 9] = [
    "type",
    "id",
    "parentId",
    "timestamp",
    "summary",
    "firstKeptEntryId",
    "tokensBefore",
    "message",
    "details",
];

fn enforce_row_bound(rows: usize, path: &Path) -> Result<(), StoreError> {
    if rows >= VERIFY_ROWS_MAX {
        Err(limit(path))
    } else {
        Ok(())
    }
}

fn insert_bounded(
    target: &mut BTreeSet<String>,
    value: &str,
    path: &Path,
) -> Result<(), StoreError> {
    if target.len() >= VERIFY_ROWS_MAX {
        return Err(limit(path));
    }
    target.insert(value.to_owned());
    Ok(())
}

fn assistant_contributes(message: &LocalMessage) -> bool {
    !matches!(
        message.extras.get("stopReason").and_then(Value::as_str),
        Some("error" | "aborted")
    )
}

fn collect_tool_calls(message: &LocalMessage, pending: &mut BTreeSet<String>) {
    let Some(parts) = message
        .content
        .as_ref()
        .and_then(|value| value.as_value().as_array())
    else {
        return;
    };
    for part in parts {
        if part.get("type").and_then(Value::as_str) == Some("toolCall")
            && let Some(id) = part.get("id").and_then(Value::as_str)
        {
            pending.insert(id.to_owned());
            if let Some((base, _)) = id.split_once('|')
                && !base.is_empty()
            {
                pending.insert(base.to_owned());
            }
        }
    }
}

fn tool_result_matches(message: &LocalMessage, pending: &BTreeSet<String>) -> bool {
    let Some(id) = message.extras.get("toolCallId").and_then(Value::as_str) else {
        return false;
    };
    pending.contains(id)
        || id
            .split_once('|')
            .is_some_and(|(base, _)| !base.is_empty() && pending.contains(base))
        || pending
            .iter()
            .any(|call| call.split_once('|').is_some_and(|(base, _)| base == id))
}

fn limit(path: &Path) -> StoreError {
    StoreError::new(StoreErrorKind::Limit, path)
}

#[cfg(test)]
mod tests;
