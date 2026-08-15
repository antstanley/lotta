//! Explicit bounded transcript migration for canonical local-backend conversations.

use crate::adapter::{RECORD_BYTES_MAX, read_record};
use crate::atomic::FileRevision;
use crate::confinement::{validate_existing, validate_regular_file};
use crate::transcript::{self, TranscriptPaths};
use crate::{StoreError, StoreErrorKind, StorePaths};
use lotta_domain::{
    Conversation, LocalMessage, MessageEntry, MessageEntryType, NonEmptyString, ProviderStack,
    SessionEntry, SessionEntryType, TranscriptEntry, TranscriptManifest, TranscriptMessageFormat,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

type IdRemapping = BTreeMap<String, Vec<String>>;

const CONVERSATIONS_SCAN_MAX: usize = 100_000;
const TRANSCRIPT_ROWS_MAX: usize = 1_000_000;

/// Deterministic outcome for one canonical conversation directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MigrationDisposition {
    /// The transcript was or would be converted.
    Converted,
    /// A current transcript required no work.
    AlreadyCurrent,
    /// An empty unversioned transcript required no work.
    Empty,
}

/// One deterministic migration report item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationItem {
    /// Canonical conversation directory.
    pub conversation_dir: PathBuf,
    /// Migration disposition.
    pub disposition: MigrationDisposition,
    /// Number of converted messages.
    pub message_count: usize,
    /// Backup path for conversion, absent for no-op entries.
    pub backup_path: Option<PathBuf>,
}

/// Deterministic full-root migration report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationReport {
    /// Explicit storage root.
    pub storage_root: PathBuf,
    /// Whether mutation was disabled.
    pub dry_run: bool,
    /// Canonical items sorted by path.
    pub items: Vec<MigrationItem>,
}

/// Migrates every canonical conversation under an explicit storage root.
///
/// Planning and validation are completed for every item before mutation. Dry run takes no lock and
/// performs no filesystem mutation. A non-dry run holds one root lock for the complete dispatch.
///
/// # Errors
/// Returns typed confinement, parse, unsupported, bound, lock, conflict, or durability failures.
pub fn migrate_transcripts(
    storage_root: impl Into<PathBuf>,
    dry_run: bool,
) -> Result<MigrationReport, StoreError> {
    migrate_transcripts_observed(storage_root, dry_run, &execute::NoopMigrationObserver)
}

pub(crate) fn migrate_transcripts_observed(
    storage_root: impl Into<PathBuf>,
    dry_run: bool,
    observer: &dyn execute::MigrationObserver,
) -> Result<MigrationReport, StoreError> {
    let paths = StorePaths::new(storage_root.into())?;
    let migrated_at = if dry_run {
        None
    } else {
        Some(lotta_domain::Timestamp::from_utc(chrono::Utc::now()))
    };
    let plans = plan_all(&paths, migrated_at)?;
    if dry_run {
        return report(&paths, true, &plans);
    }
    let lock = crate::LottaStorageLock::try_acquire_confined(paths.root())?;
    let mut items = Vec::new();
    items
        .try_reserve_exact(plans.len())
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, paths.root()))?;
    for plan in &plans {
        items.push(execute(plan, &lock, observer)?);
    }
    Ok(MigrationReport {
        storage_root: paths.root().to_path_buf(),
        dry_run: false,
        items,
    })
}

struct Plan {
    paths: TranscriptPaths,
    disposition: MigrationDisposition,
    entries: Vec<TranscriptEntry>,
    conversation_bytes: Vec<u8>,
    original_conversation_bytes: Vec<u8>,
    original_manifest_bytes: Option<Vec<u8>>,
    manifest: TranscriptManifest,
    revisions: Revisions,
}

struct Revisions {
    messages: FileRevision,
    conversation: FileRevision,
    manifest: FileRevision,
}

fn plan_all(
    paths: &StorePaths,
    migrated_at: Option<lotta_domain::Timestamp>,
) -> Result<Vec<Plan>, StoreError> {
    let directory = paths.conversations();
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(StoreError::from_io(&directory, &error)),
        Ok(metadata) if metadata.file_type().is_dir() => {
            validate_existing(paths.root(), &directory)?;
        }
        Ok(_) => return Err(StoreError::new(StoreErrorKind::InvalidPath, directory)),
    }
    let mut plans = Vec::new();
    let mut scanned = 0_usize;
    for entry in
        std::fs::read_dir(&directory).map_err(|error| StoreError::from_io(&directory, &error))?
    {
        scanned = scanned
            .checked_add(1)
            .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, &directory))?;
        if scanned > CONVERSATIONS_SCAN_MAX {
            return Err(StoreError::new(StoreErrorKind::Limit, &directory));
        }
        let entry = entry.map_err(|error| StoreError::from_io(&directory, &error))?;
        if !entry
            .file_type()
            .map_err(|error| StoreError::from_io(&entry.path(), &error))?
            .is_dir()
        {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, entry.path()));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| StoreError::new(StoreErrorKind::InvalidPath, entry.path()))?;
        let key = crate::ConversationKey::decode(&name)?;
        let plan = plan_one(paths, entry.path(), &key, migrated_at)?;
        plans
            .try_reserve(1)
            .map_err(|_| StoreError::new(StoreErrorKind::Limit, &directory))?;
        plans.push(plan);
    }
    plans.sort_by(|left, right| left.paths.directory.cmp(&right.paths.directory));
    Ok(plans)
}

fn plan_one(
    store: &StorePaths,
    directory: PathBuf,
    key: &crate::ConversationKey,
    migrated_at: Option<lotta_domain::Timestamp>,
) -> Result<Plan, StoreError> {
    validate_existing(store.root(), &directory)?;
    let paths = TranscriptPaths {
        root: store.root().to_path_buf(),
        conversation: directory.join("conversation.json"),
        manifest: directory.join("manifest.json"),
        messages: directory.join("messages.jsonl"),
        directory,
    };
    validate_regular_file(&paths.root, &paths.conversation)?;
    validate_regular_file(&paths.root, &paths.messages)?;
    let conversation_revision = FileRevision::sample_path(&paths.conversation)?;
    let original_conversation_bytes = bounded_read(&paths.conversation, RECORD_BYTES_MAX as u64)?;
    let conversation: Conversation = read_record(&paths.conversation)?;
    validate_key(key, &conversation, &paths.conversation)?;
    let message_revision =
        FileRevision::sample_path_bounded(&paths.messages, transcript::TRANSCRIPT_BYTES_MAX)?;
    let (source, manifest_revision) = classify(&paths, &message_revision)?;
    let original_manifest_bytes = if manifest_revision.exists() {
        Some(bounded_read(&paths.manifest, RECORD_BYTES_MAX as u64)?)
    } else {
        None
    };
    build_plan(
        paths,
        conversation,
        original_conversation_bytes,
        original_manifest_bytes,
        &source,
        migrated_at,
        Revisions {
            messages: message_revision,
            conversation: conversation_revision,
            manifest: manifest_revision,
        },
    )
}

#[derive(Clone)]
enum Source {
    Empty,
    Current,
    Unversioned,
    Versioned(TranscriptManifest),
    Repair(TranscriptManifest),
}

fn classify(
    paths: &TranscriptPaths,
    messages: &FileRevision,
) -> Result<(Source, FileRevision), StoreError> {
    let manifest_revision = FileRevision::sample_path(&paths.manifest)?;
    if !manifest_revision.exists() {
        return if messages.length() == 0 {
            Ok((Source::Empty, manifest_revision))
        } else {
            Ok((Source::Unversioned, manifest_revision))
        };
    }
    validate_regular_file(&paths.root, &paths.manifest)?;
    let manifest = transcript::manifest::read(&paths.manifest)?;
    let legacy = inspect_rows(paths)?;
    let source = match (manifest.schema_version, manifest.message_format, legacy) {
        (1, TranscriptMessageFormat::PiAiMessageJsonl, _) => Source::Versioned(manifest),
        (2, TranscriptMessageFormat::PiSessionEntryJsonl, true) => Source::Repair(manifest),
        (2, TranscriptMessageFormat::PiSessionEntryJsonl, false) => Source::Current,
        _ => return Err(StoreError::new(StoreErrorKind::Parse, &paths.manifest)),
    };
    Ok((source, manifest_revision))
}

fn inspect_rows(paths: &TranscriptPaths) -> Result<bool, StoreError> {
    let mut legacy = false;
    let mut rows = 0_usize;
    transcript::bounds::read_rows_terminated(&paths.root, &paths.messages, |_, row, _| {
        if row.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        rows = rows
            .checked_add(1)
            .ok_or_else(|| limit_error(&paths.messages))?;
        if rows > TRANSCRIPT_ROWS_MAX {
            return Err(limit_error(&paths.messages));
        }
        let value: Value = serde_json::from_slice(row)
            .map_err(|_| StoreError::new(StoreErrorKind::Parse, &paths.messages))?;
        legacy |= is_legacy_ui(&value);
        Ok(())
    })?;
    Ok(legacy)
}

fn build_plan(
    paths: TranscriptPaths,
    mut conversation: Conversation,
    original_conversation_bytes: Vec<u8>,
    original_manifest_bytes: Option<Vec<u8>>,
    source: &Source,
    migrated_at: Option<lotta_domain::Timestamp>,
    revisions: Revisions,
) -> Result<Plan, StoreError> {
    let created = conversation.created_at;
    let (disposition, messages, remap, migrated_from) = match source {
        Source::Empty => (
            MigrationDisposition::Empty,
            Vec::new(),
            BTreeMap::new(),
            None,
        ),
        Source::Current => (
            MigrationDisposition::AlreadyCurrent,
            Vec::new(),
            BTreeMap::new(),
            None,
        ),
        Source::Versioned(_source_manifest) => (
            MigrationDisposition::Converted,
            read_versioned(&paths)?,
            BTreeMap::new(),
            Some("versioned-pi-ai-message-jsonl".to_owned()),
        ),
        Source::Repair(_source_manifest) => {
            let (messages, remap) = read_ui(&paths, true, created)?;
            (
                MigrationDisposition::Converted,
                messages,
                remap,
                Some("versioned-pi-transcript-with-legacy-ui-message-rows".to_owned()),
            )
        }
        Source::Unversioned => {
            let (messages, remap) = read_ui(&paths, false, created)?;
            (
                MigrationDisposition::Converted,
                messages,
                remap,
                Some("unversioned-legacy-local-message-jsonl".to_owned()),
            )
        }
    };
    remap_context(&mut conversation, &remap, &paths.conversation)?;
    let entries = entries(&conversation, messages, &paths.messages)?;
    let conversation_bytes = encode_record(&conversation, &paths.conversation)?;
    let manifest = TranscriptManifest {
        schema_version: transcript::manifest::TRANSCRIPT_SCHEMA_VERSION,
        message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
        provider_stack: match source {
            Source::Versioned(manifest) | Source::Repair(manifest) => manifest.provider_stack,
            _ => ProviderStack::PiAi,
        },
        created_at: match source {
            Source::Versioned(manifest) | Source::Repair(manifest) => manifest.created_at,
            _ => created,
        },
        migrated_from,
        migrated_at: (disposition == MigrationDisposition::Converted)
            .then_some(migrated_at)
            .flatten(),
        backup_path: None,
    };
    if disposition == MigrationDisposition::Converted {
        transcript::manifest::encode(&manifest, &paths.manifest)?;
    }
    Ok(Plan {
        paths,
        disposition,
        entries,
        conversation_bytes,
        original_conversation_bytes,
        original_manifest_bytes,
        manifest,
        revisions,
    })
}

fn parse_complete(row: &[u8], _terminated: bool, path: &Path) -> Result<Value, StoreError> {
    serde_json::from_slice(row).map_err(|_| StoreError::new(StoreErrorKind::Parse, path))
}

fn read_versioned(paths: &TranscriptPaths) -> Result<Vec<LocalMessage>, StoreError> {
    let mut messages = Vec::new();
    transcript::bounds::read_rows_terminated(
        &paths.root,
        &paths.messages,
        |_, row, terminated| {
            if row.iter().all(u8::is_ascii_whitespace) {
                return Ok(());
            }
            let value = parse_complete(row, terminated, &paths.messages)?;
            let message = serde_json::from_value(value)
                .map_err(|_| StoreError::new(StoreErrorKind::Parse, &paths.messages))?;
            messages
                .try_reserve(1)
                .map_err(|_| StoreError::new(StoreErrorKind::Limit, &paths.messages))?;
            messages.push(message);
            Ok(())
        },
    )?;
    Ok(messages)
}

fn read_ui(
    paths: &TranscriptPaths,
    repair: bool,
    fallback: lotta_domain::Timestamp,
) -> Result<(Vec<LocalMessage>, IdRemapping), StoreError> {
    let mut values = Vec::new();
    let mut max_id = 0_u64;
    let mut rows = 0_usize;
    transcript::bounds::read_rows_terminated(&paths.root, &paths.messages, |_, row, _| {
        if row.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        rows = rows
            .checked_add(1)
            .ok_or_else(|| limit_error(&paths.messages))?;
        if rows > TRANSCRIPT_ROWS_MAX {
            return Err(limit_error(&paths.messages));
        }
        let value: Value = serde_json::from_slice(row)
            .map_err(|_| StoreError::new(StoreErrorKind::Parse, &paths.messages))?;
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            max_id = max_id.max(numeric_id(id));
        }
        values
            .try_reserve(1)
            .map_err(|_| limit_error(&paths.messages))?;
        values.push(value);
        Ok(())
    })?;
    let mut ids = BTreeSet::new();
    for value in &values {
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            ids.insert(id.to_owned());
        }
    }
    let mut counter = max_id
        .checked_add(1)
        .ok_or_else(|| limit_error(&paths.messages))?;
    let mut messages = Vec::new();
    let mut remap = BTreeMap::new();
    for value in values {
        if repair && is_pi_local(&value) {
            let message = serde_json::from_value(value)
                .map_err(|_| StoreError::new(StoreErrorKind::Parse, &paths.messages))?;
            messages
                .try_reserve(1)
                .map_err(|_| limit_error(&paths.messages))?;
            messages.push(message);
            continue;
        }
        if !is_legacy_ui(&value) {
            continue;
        }
        let original = value.get("id").and_then(Value::as_str).map(str::to_owned);
        let converted = convert_ui(&value, fallback, &mut counter, &mut ids, &paths.messages)?;
        if let Some(id) = original
            && !converted.iter().any(|message| message.id.as_str() == id)
            && !converted.is_empty()
        {
            let mut mapped = Vec::new();
            mapped
                .try_reserve_exact(converted.len())
                .map_err(|_| limit_error(&paths.messages))?;
            mapped.extend(
                converted
                    .iter()
                    .map(|message| message.id.as_str().to_owned()),
            );
            remap.insert(id, mapped);
        }
        messages
            .try_reserve(converted.len())
            .map_err(|_| limit_error(&paths.messages))?;
        messages.extend(converted);
    }
    Ok((messages, remap))
}

fn is_legacy_ui(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("parts").is_some_and(Value::is_array)
            && (!object.contains_key("content") || object.get("content") == Some(&Value::Null))
    })
}

fn is_pi_local(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("id").is_some_and(Value::is_string)
            && matches!(
                object.get("role").and_then(Value::as_str),
                Some("user" | "assistant" | "toolResult")
            )
            && object.contains_key("content")
    })
}

fn numeric_id(id: &str) -> u64 {
    id.strip_prefix("ui-msg-")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn next_id(
    counter: &mut u64,
    ids: &mut BTreeSet<String>,
    path: &Path,
) -> Result<String, StoreError> {
    loop {
        let id = format!("ui-msg-{counter}");
        *counter = counter.checked_add(1).ok_or_else(|| limit_error(path))?;
        if ids.insert(id.clone()) {
            return Ok(id);
        }
    }
}

fn convert_ui(
    value: &Value,
    fallback: lotta_domain::Timestamp,
    counter: &mut u64,
    ids: &mut BTreeSet<String>,
    path: &Path,
) -> Result<Vec<LocalMessage>, StoreError> {
    let object = value
        .as_object()
        .ok_or_else(|| StoreError::new(StoreErrorKind::Parse, path))?;
    let id = match object.get("id").and_then(Value::as_str) {
        Some(id) => id.to_owned(),
        None => next_id(counter, ids, path)?,
    };
    let role = object.get("role").and_then(Value::as_str).unwrap_or("");
    let parts = object
        .get("parts")
        .and_then(Value::as_array)
        .map_or(&[] as &[Value], Vec::as_slice);
    let metadata = object
        .get("metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (timestamp, created) = legacy_time(object, &metadata, fallback);
    let updated = metadata
        .get("updated_at")
        .and_then(Value::as_str)
        .unwrap_or(&created)
        .to_owned();
    let metadata = normalized_metadata(metadata, &created, &updated);
    if role == "user"
        && metadata
            .get("compaction")
            .is_some_and(|value| !value.is_null())
    {
        let text = parts
            .iter()
            .filter_map(Value::as_object)
            .find_map(|part| {
                (part.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| part.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .unwrap_or("");
        return one_message(
            serde_json::json!({
                "id": id,
                "role": "user",
                "metadata": metadata,
                "content": [{"type": "text", "text": text}],
                "timestamp": timestamp
            }),
            path,
        );
    }
    if matches!(role, "user" | "system") {
        let content = user_content(parts);
        return one_message(
            serde_json::json!({
                "id": id,
                "role": "user",
                "metadata": metadata,
                "content": content,
                "timestamp": timestamp
            }),
            path,
        );
    }
    if role != "assistant" {
        return Ok(Vec::new());
    }
    convert_assistant(parts, &id, &metadata, timestamp, counter, ids, path)
}

fn legacy_time(
    object: &Map<String, Value>,
    metadata: &Map<String, Value>,
    fallback: lotta_domain::Timestamp,
) -> (f64, String) {
    let raw = metadata
        .get("created_at")
        .and_then(Value::as_str)
        .or_else(|| object.get("created_at").and_then(Value::as_str))
        .or_else(|| object.get("updated_at").and_then(Value::as_str));
    let parsed = raw
        .and_then(|value| lotta_domain::Timestamp::parse_persisted_rfc3339(value).ok())
        .unwrap_or(fallback);
    let millis = parsed
        .as_utc()
        .timestamp_millis()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    (
        millis,
        raw.map_or_else(|| parsed.as_utc().to_rfc3339(), str::to_owned),
    )
}

fn normalized_metadata(
    mut metadata: Map<String, Value>,
    created: &str,
    updated: &str,
) -> Map<String, Value> {
    metadata.insert("created_at".to_owned(), Value::String(created.to_owned()));
    metadata.insert("updated_at".to_owned(), Value::String(updated.to_owned()));
    metadata
}

fn user_content(parts: &[Value]) -> Vec<Value> {
    let mut content = Vec::new();
    for object in parts.iter().filter_map(Value::as_object) {
        if object.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                content.push(serde_json::json!({"type":"text","text":text}));
            }
        } else if let Some(image) = legacy_image(object) {
            content.push(image);
        }
    }
    if content.is_empty() {
        content.push(serde_json::json!({"type":"text","text":""}));
    }
    content
}

fn legacy_image(part: &Map<String, Value>) -> Option<Value> {
    if part.get("type").and_then(Value::as_str) == Some("image") {
        let source = part.get("source")?.as_object()?;
        if source.get("type").and_then(Value::as_str) == Some("base64") {
            return Some(serde_json::json!({
                "type": "image",
                "mimeType": source.get("media_type")?.as_str()?,
                "data": source.get("data")?.as_str()?
            }));
        }
    }
    if part.get("type").and_then(Value::as_str) == Some("file") {
        let media = part
            .get("mediaType")
            .or_else(|| part.get("mime"))?
            .as_str()?;
        let url = part.get("url")?.as_str()?;
        if media.starts_with("image/") && url.starts_with("data:") {
            return url.find(";base64,").map(|index| {
                serde_json::json!({
                    "type": "image",
                    "mimeType": media,
                    "data": &url[index + 8..]
                })
            });
        }
    }
    None
}

fn convert_assistant(
    parts: &[Value],
    id: &str,
    metadata: &Map<String, Value>,
    timestamp: f64,
    counter: &mut u64,
    ids: &mut BTreeSet<String>,
    path: &Path,
) -> Result<Vec<LocalMessage>, StoreError> {
    let mut steps: Vec<Vec<&Map<String, Value>>> = Vec::new();
    let mut current = Vec::new();
    for part in parts.iter().filter_map(Value::as_object) {
        if part.get("type").and_then(Value::as_str) == Some("step-start") {
            if !current.is_empty() {
                steps.push(std::mem::take(&mut current));
            }
        } else {
            current.push(part);
        }
    }
    if !current.is_empty() {
        steps.push(current);
    }
    let multiple = steps.len() > 1;
    let mut output = Vec::new();
    for step in steps {
        let step_id = if multiple {
            next_id(counter, ids, path)?
        } else {
            id.to_owned()
        };
        let mut content = Vec::new();
        let mut results = Vec::new();
        for part in step {
            let mut part_state = AssistantPartContext {
                content: &mut content,
                results: &mut results,
                metadata,
                timestamp,
                counter,
                ids,
                path,
            };
            convert_assistant_part(part, &mut part_state)?;
        }
        if !content.is_empty() {
            let value = serde_json::json!({
                "id": step_id,
                "role": "assistant",
                "metadata": metadata,
                "content": content,
                "api": "legacy-local",
                "provider": "legacy-local",
                "model": "legacy-local",
                "usage": {
                    "input": 0,
                    "output": 0,
                    "cacheRead": 0,
                    "cacheWrite": 0,
                    "totalTokens": 0,
                    "cost": {
                        "input": 0,
                        "output": 0,
                        "cacheRead": 0,
                        "cacheWrite": 0,
                        "total": 0
                    }
                },
                "stopReason": "stop",
                "timestamp": timestamp
            });
            output.push(parse_message(value, path)?);
        }
        output.extend(results);
    }
    Ok(output)
}

struct AssistantPartContext<'a> {
    content: &'a mut Vec<Value>,
    results: &'a mut Vec<LocalMessage>,
    metadata: &'a Map<String, Value>,
    timestamp: f64,
    counter: &'a mut u64,
    ids: &'a mut BTreeSet<String>,
    path: &'a Path,
}

fn convert_assistant_part(
    part: &Map<String, Value>,
    context: &mut AssistantPartContext<'_>,
) -> Result<(), StoreError> {
    let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
    if kind == "text" {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            context
                .content
                .push(serde_json::json!({"type":"text","text":text}));
        }
        return Ok(());
    }
    if kind == "reasoning" {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            context
                .content
                .push(serde_json::json!({"type":"thinking","thinking":text}));
        }
        return Ok(());
    }
    let Some(name) = kind.strip_prefix("tool-") else {
        return Ok(());
    };
    let Some(call) = part.get("toolCallId").and_then(Value::as_str) else {
        return Ok(());
    };
    let input = part
        .get("input")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let arguments = if input.is_object() {
        input
    } else {
        serde_json::json!({"input":input})
    };
    context
        .content
        .push(serde_json::json!({"type":"toolCall","id":call,"name":name,"arguments":arguments}));
    let state = part.get("state").and_then(Value::as_str);
    if !matches!(
        state,
        Some("output-available" | "output-error" | "output-denied")
    ) {
        return Ok(());
    }
    let value = if state == Some("output-available") {
        part.get("output")
    } else {
        part.get("errorText")
    };
    let text = text_from_unknown(value);
    let result = serde_json::json!({
        "id": next_id(context.counter, context.ids, context.path)?,
        "role": "toolResult",
        "toolCallId": call,
        "toolName": name,
        "content": [{"type": "text", "text": text}],
        "isError": state != Some("output-available"),
        "timestamp": context.timestamp,
        "metadata": context.metadata
    });
    context.results.push(parse_message(result, context.path)?);
    Ok(())
}

fn text_from_unknown(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

fn one_message(value: Value, path: &Path) -> Result<Vec<LocalMessage>, StoreError> {
    let mut output = Vec::new();
    output.try_reserve_exact(1).map_err(|_| limit_error(path))?;
    output.push(parse_message(value, path)?);
    Ok(output)
}
fn parse_message(value: Value, path: &Path) -> Result<LocalMessage, StoreError> {
    serde_json::from_value(value).map_err(|_| StoreError::new(StoreErrorKind::Parse, path))
}
fn limit_error(path: &Path) -> StoreError {
    StoreError::new(StoreErrorKind::Limit, path)
}

fn entries(
    conversation: &Conversation,
    messages: Vec<LocalMessage>,
    path: &Path,
) -> Result<Vec<TranscriptEntry>, StoreError> {
    let capacity = messages
        .len()
        .checked_add(1)
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(capacity)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    entries.push(TranscriptEntry::Session(SessionEntry {
        entry_type: SessionEntryType::Session,
        version: 3,
        id: NonEmptyString::new(conversation.id.as_str().to_owned())
            .map_err(|_| StoreError::new(StoreErrorKind::Parse, path))?,
        timestamp: conversation.created_at,
        cwd: String::new(),
    }));
    let mut parent = None;
    for message in messages {
        let id = message.id.as_str().to_owned();
        let timestamp = message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("created_at"))
            .and_then(Value::as_str)
            .and_then(|value| lotta_domain::Timestamp::parse_persisted_rfc3339(value).ok())
            .unwrap_or(conversation.created_at);
        entries.push(TranscriptEntry::Message(MessageEntry {
            entry_type: MessageEntryType::Message,
            id: NonEmptyString::new(id.clone())
                .map_err(|_| StoreError::new(StoreErrorKind::Parse, path))?,
            parent_id: parent,
            timestamp,
            message,
        }));
        parent = Some(id);
    }
    transcript::session_header::validate_sequence(&entries, path)?;
    Ok(entries)
}

fn remap_context(
    conversation: &mut Conversation,
    remap: &BTreeMap<String, Vec<String>>,
    path: &Path,
) -> Result<(), StoreError> {
    if remap.is_empty() {
        return Ok(());
    }
    let mut values = Vec::new();
    for id in conversation.in_context_message_ids.as_slice() {
        if let Some(replacements) = remap.get(id.as_str()) {
            values
                .try_reserve(replacements.len())
                .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
            for replacement in replacements {
                values.push(
                    lotta_domain::MessageId::accept(replacement)
                        .map_err(|_| StoreError::new(StoreErrorKind::Parse, path))?,
                );
            }
        } else {
            values
                .try_reserve(1)
                .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
            values.push(id.clone());
        }
    }
    conversation.in_context_message_ids = lotta_domain::BoundedVec::new(values)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    Ok(())
}

fn encode_record(value: &Conversation, path: &Path) -> Result<Vec<u8>, StoreError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| StoreError::new(StoreErrorKind::Parse, path))?;
    bytes
        .try_reserve_exact(1)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    bytes.push(b'\n');
    if bytes.len() > RECORD_BYTES_MAX {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    Ok(bytes)
}

fn validate_key(
    key: &crate::ConversationKey,
    conversation: &Conversation,
    path: &Path,
) -> Result<(), StoreError> {
    let valid = match key {
        crate::ConversationKey::Default(agent) => agent == &conversation.agent_id,
        crate::ConversationKey::Named(id) => id == &conversation.id,
    };
    if !valid {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    Ok(())
}

mod execute;
use execute::execute;

fn bounded_read(path: &Path, maximum: u64) -> Result<Vec<u8>, StoreError> {
    use std::io::Read as _;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| StoreError::from_io(path, &error))?;
    if metadata.len() > maximum {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    std::fs::File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| StoreError::from_io(path, &error))?;
    Ok(bytes)
}

#[cfg(test)]
#[path = "migration/tests/backup_rotation.rs"]
mod backup_rotation;
#[cfg(test)]
#[path = "migration/tests/converter_evidence.rs"]
mod converter_evidence;
#[cfg(test)]
#[path = "migration/tests/converts.rs"]
mod converts;
#[cfg(test)]
#[path = "migration/tests/dry_run.rs"]
mod dry_run;
#[cfg(test)]
#[path = "migration/tests/durability.rs"]
mod durability;
#[cfg(test)]
#[path = "migration/tests/negative.rs"]
mod negative;
#[cfg(test)]
#[path = "migration/tests/rollback_evidence.rs"]
mod rollback_evidence;
#[cfg(test)]
#[path = "migration/tests/support.rs"]
mod support;

fn report(
    paths: &StorePaths,
    dry_run: bool,
    plans: &[Plan],
) -> Result<MigrationReport, StoreError> {
    let mut items = Vec::new();
    items
        .try_reserve_exact(plans.len())
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, paths.root()))?;
    for plan in plans {
        items.push(execute::item(plan, None)?);
    }
    Ok(MigrationReport {
        storage_root: paths.root().to_path_buf(),
        dry_run,
        items,
    })
}
