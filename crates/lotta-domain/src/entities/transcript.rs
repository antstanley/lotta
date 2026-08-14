use super::{BoundedMap, UNBOUNDED_MAP_FIELDS_MAX};
use crate::{LocalMessage, NonEmptyString, Timestamp};
use serde::{Deserialize, Serialize};

/// Transcript manifest schema version.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TranscriptSchemaVersion {
    /// Legacy transcript manifest.
    #[serde(rename = "1")]
    One,
    /// Current transcript manifest.
    #[serde(rename = "2")]
    Two,
}

/// Transcript message storage format.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TranscriptMessageFormat {
    /// Legacy raw pi-ai message JSONL.
    #[serde(rename = "pi-ai-message-jsonl")]
    PiAiMessageJsonl,
    /// Parent-linked pi session-entry JSONL.
    #[serde(rename = "pi-session-entry-jsonl")]
    PiSessionEntryJsonl,
}

/// Provider stack constant used by transcript manifests.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ProviderStack {
    /// pi-ai provider stack.
    #[serde(rename = "pi-ai")]
    PiAi,
}

/// Transcript version and format declaration.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TranscriptManifest {
    /// Manifest schema version.
    pub schema_version: u8,
    /// Stored message format.
    pub message_format: TranscriptMessageFormat,
    /// Provider stack.
    pub provider_stack: ProviderStack,
    /// Creation timestamp.
    pub created_at: Timestamp,
    /// Optional migration source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_from: Option<String>,
    /// Optional migration timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_at: Option<Timestamp>,
    /// Optional backup path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

/// Version-three transcript session header.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionEntry {
    /// Fixed session discriminant.
    #[serde(rename = "type")]
    pub entry_type: SessionEntryType,
    /// Fixed session version.
    pub version: u8,
    /// Session identifier.
    pub id: NonEmptyString,
    /// Entry timestamp.
    pub timestamp: Timestamp,
    /// Session working directory.
    pub cwd: String,
}

/// Session entry discriminant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SessionEntryType {
    /// Session header.
    #[serde(rename = "session")]
    Session,
}

/// Parent-linked local message entry.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MessageEntry {
    /// Fixed message discriminant.
    #[serde(rename = "type")]
    pub entry_type: MessageEntryType,
    /// Entry identifier.
    pub id: NonEmptyString,
    /// Parent entry identifier; null marks the root.
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    /// Entry timestamp.
    pub timestamp: Timestamp,
    /// Embedded local message.
    pub message: LocalMessage,
}

/// Message entry discriminant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MessageEntryType {
    /// Message snapshot.
    #[serde(rename = "message")]
    Message,
}

/// Transcript compaction boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CompactionEntry {
    /// Fixed compaction discriminant.
    #[serde(rename = "type")]
    pub entry_type: CompactionEntryType,
    /// Entry identifier.
    pub id: NonEmptyString,
    /// Parent entry identifier; null marks the root.
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    /// Entry timestamp.
    pub timestamp: Timestamp,
    /// Compaction summary.
    pub summary: String,
    /// First retained entry after compaction.
    #[serde(rename = "firstKeptEntryId")]
    pub first_kept_entry_id: Option<String>,
    /// Token count before compaction.
    #[serde(rename = "tokensBefore")]
    pub tokens_before: u64,
    /// Embedded summary message.
    pub message: LocalMessage,
    /// Optional compaction details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<BoundedMap<UNBOUNDED_MAP_FIELDS_MAX>>,
}

/// Compaction entry discriminant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CompactionEntryType {
    /// Compaction boundary.
    #[serde(rename = "compaction")]
    Compaction,
}

/// Any canonical transcript JSONL entry.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TranscriptEntry {
    /// Session header.
    Session(SessionEntry),
    /// Local message snapshot.
    Message(MessageEntry),
    /// Compaction boundary.
    Compaction(CompactionEntry),
}
