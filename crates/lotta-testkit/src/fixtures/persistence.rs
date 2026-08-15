//! Typed, bounded access to the source-derived persistence compatibility corpus.

use super::{FixtureLoader, sha256};
use crate::{TESTKIT_ITEMS_MAX, TestkitError};
use serde::Deserialize;
use std::path::{Component, Path};

/// Pinned TypeScript source revision represented by this corpus.
pub const SOURCE_COMMIT: &str = "300f923f16cc8eee50656d7da732902c1dea2b65";
/// Exact conceptual case count.
pub const PERSISTENCE_CASES: usize = 9;
/// Exact indexed side-store path count.
pub const SIDE_STORES: usize = 12;
/// Exact generated inventory entry count, excluding the self-referential index.
pub const INVENTORY_ENTRIES: usize = 84;

pub(super) const CASE_ROOTS: [&str; PERSISTENCE_CASES] = [
    "current_typescript_state",
    "rust_target_state",
    "unversioned_legacy_transcript",
    "versioned_legacy_transcript",
    "baseline_tolerated_versioned_rows",
    "orphan_result_repair_input",
    "interrupted_append",
    "interrupted_replacement",
    "corrupt_unsupported_manifests",
];

/// Exact persistence compatibility fixture classes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistenceCase {
    /// Current TypeScript state read by Rust.
    CurrentTypescriptState,
    /// Rust target wire state read by TypeScript.
    RustTargetState,
    /// Unversioned legacy transcript migration input.
    UnversionedLegacyTranscript,
    /// Versioned schema-one legacy transcript.
    VersionedLegacyTranscript,
    /// Schema-two rows tolerated by the baseline.
    BaselineToleratedVersionedRows,
    /// Orphan tool-result active projection repair input.
    OrphanResultRepairInput,
    /// Interrupted append recovery input.
    InterruptedAppend,
    /// Interrupted replacement recovery input.
    InterruptedReplacement,
    /// Corrupt and unsupported manifest inputs.
    CorruptUnsupportedManifests,
}

impl PersistenceCase {
    /// Returns every case in fixed conceptual order.
    #[must_use]
    pub const fn all() -> [Self; PERSISTENCE_CASES] {
        [
            Self::CurrentTypescriptState,
            Self::RustTargetState,
            Self::UnversionedLegacyTranscript,
            Self::VersionedLegacyTranscript,
            Self::BaselineToleratedVersionedRows,
            Self::OrphanResultRepairInput,
            Self::InterruptedAppend,
            Self::InterruptedReplacement,
            Self::CorruptUnsupportedManifests,
        ]
    }

    /// Returns the exact fixture directory.
    #[must_use]
    pub const fn directory(self) -> &'static str {
        match self {
            Self::CurrentTypescriptState => CASE_ROOTS[0],
            Self::RustTargetState => CASE_ROOTS[1],
            Self::UnversionedLegacyTranscript => CASE_ROOTS[2],
            Self::VersionedLegacyTranscript => CASE_ROOTS[3],
            Self::BaselineToleratedVersionedRows => CASE_ROOTS[4],
            Self::OrphanResultRepairInput => CASE_ROOTS[5],
            Self::InterruptedAppend => CASE_ROOTS[6],
            Self::InterruptedReplacement => CASE_ROOTS[7],
            Self::CorruptUnsupportedManifests => CASE_ROOTS[8],
        }
    }

    /// Returns the expected compatibility or recovery outcome.
    #[must_use]
    pub const fn expectation(self) -> &'static str {
        match self {
            Self::CurrentTypescriptState => "rust reads",
            Self::RustTargetState => "typescript reads",
            Self::UnversionedLegacyTranscript => "migration required",
            Self::VersionedLegacyTranscript => "typescript upgrades on persistence",
            Self::BaselineToleratedVersionedRows => "baseline loads",
            Self::OrphanResultRepairInput => "active projection repairs",
            Self::InterruptedAppend => "complete prefix recovers",
            Self::InterruptedReplacement => "active original remains",
            Self::CorruptUnsupportedManifests => "reject without mutation",
        }
    }
}

/// Source and encoded form of one baseline storage key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct PersistenceKeyRecord {
    /// Exact text passed to Node's base64url encoder.
    pub source: String,
    /// Exact unpadded URL-safe encoded path segment.
    pub encoded: String,
}

/// Fixed baseline key records.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct PersistenceKeys {
    /// Agent filename key.
    pub agent: PersistenceKeyRecord,
    /// Default conversation directory key.
    pub default_conversation: PersistenceKeyRecord,
    /// Named conversation directory key.
    pub named_conversation: PersistenceKeyRecord,
}

/// Fixed synthetic identifiers shared by the corpus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct PersistenceIds {
    /// Synthetic local agent ID.
    pub agent: String,
    /// Synthetic conversation ID.
    pub conversation: String,
}

/// Exact source transcript or recovery format.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceFormat {
    /// Exact `Schema2PiSessionEntryJsonl` wire classification.
    Schema2PiSessionEntryJsonl,
    /// Exact `UnversionedLegacyUiJsonl` wire classification.
    UnversionedLegacyUiJsonl,
    /// Exact `Schema1PiAiMessageJsonl` wire classification.
    Schema1PiAiMessageJsonl,
    /// Exact `Schema2ToleratedRows` wire classification.
    Schema2ToleratedRows,
    /// Exact `Schema2OrphanToolResult` wire classification.
    Schema2OrphanToolResult,
    /// Exact `Schema2TruncatedJsonl` wire classification.
    Schema2TruncatedJsonl,
    /// Exact `Schema2ReplacementEvidence` wire classification.
    Schema2ReplacementEvidence,
    /// Exact `CorruptAndUnsupportedManifests` wire classification.
    CorruptAndUnsupportedManifests,
}
/// Exact Rust fixture behavior.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RustBehavior {
    /// Exact `ReadsCurrent` wire classification.
    ReadsCurrent,
    /// Exact `WritesExactWire` wire classification.
    WritesExactWire,
    /// Exact `RequiresExplicitMigration` wire classification.
    RequiresExplicitMigration,
    /// Exact `UpgradesOnNonemptyPersistence` wire classification.
    UpgradesOnNonemptyPersistence,
    /// Exact `LoadsMessageEntries` wire classification.
    LoadsMessageEntries,
    /// Exact `RepairsActiveProjection` wire classification.
    RepairsActiveProjection,
    /// Exact `RecoversCompletePrefix` wire classification.
    RecoversCompletePrefix,
    /// Exact `PreservesOriginal` wire classification.
    PreservesOriginal,
    /// Exact `RejectsWithoutMutation` wire classification.
    RejectsWithoutMutation,
}
/// Exact pinned TypeScript behavior.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TypescriptBehavior {
    /// Exact `WritesCurrent` wire classification.
    WritesCurrent,
    /// Reads the exact Rust-produced wire format.
    ReadsExactWire,
    /// Exact `RequiresExplicitMigration` wire classification.
    RequiresExplicitMigration,
    /// Exact `UpgradesOnNonemptyPersistence` wire classification.
    UpgradesOnNonemptyPersistence,
    /// Exact `IgnoresSessionHeader` wire classification.
    IgnoresSessionHeader,
    /// Exact `RepairsActiveProjection` wire classification.
    RepairsActiveProjection,
    /// Exact `NotApplicableRustHardening` wire classification.
    NotApplicableRustHardening,
    /// Exact `RejectsWithoutMutation` wire classification.
    RejectsWithoutMutation,
}
/// Exact mutation or recovery disposition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MutationDisposition {
    /// Exact `None` wire classification.
    None,
    /// Exact `BackupThenConversion` wire classification.
    BackupThenConversion,
    /// Exact `Schema2Rewrite` wire classification.
    Schema2Rewrite,
    /// Exact `ConversationOnly` wire classification.
    ConversationOnly,
    /// Exact `DiscardTruncatedTail` wire classification.
    DiscardTruncatedTail,
    /// Exact `CandidateNotCommitted` wire classification.
    CandidateNotCommitted,
}
/// Semantic class assigned to an inventory file.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SemanticKind {
    /// Exact `Json` wire classification.
    Json,
    /// Exact `Jsonl` wire classification.
    Jsonl,
    /// Exact `YamlOrJson` wire classification.
    YamlOrJson,
    /// Exact `Bytes` wire classification.
    Bytes,
}
/// One complete corpus inventory entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct InventoryEntry {
    /// Sorted corpus-relative path.
    pub path: String,
    /// Semantic parser class.
    pub semantic_kind: SemanticKind,
    /// Exact byte length.
    pub byte_count: usize,
    /// Exact generator-computed SHA-256.
    pub sha256: String,
}
/// One conceptual compatibility case record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct PersistenceCaseRecord {
    /// One-based conceptual case ID.
    pub case_id: u8,
    /// Numbered persistence specification obligation.
    pub obligation: u8,
    /// Confined case root relative to `fixtures/persistence`.
    pub relative_root: String,
    /// Baseline source format label.
    pub source_format: SourceFormat,
    /// Expected Rust behavior.
    pub expected_rust_behavior: RustBehavior,
    /// Expected TypeScript behavior.
    pub expected_typescript_behavior: TypescriptBehavior,
    /// Expected recovery or mutation behavior.
    pub expected_recovery_or_mutation: MutationDisposition,
}

/// One indexed baseline side-store path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct SideStoreRecord {
    /// Confined path relative to `fixtures`.
    pub path: String,
    /// Persisted text format.
    pub format: String,
}

/// Complete fixed-shape persistence fixture index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct PersistenceIndex {
    /// Index schema version.
    pub schema_version: u8,
    /// Exact pinned TypeScript revision.
    pub source_commit: String,
    /// Deterministic extractor path.
    pub generator: String,
    /// Source derivation explanation.
    pub derivation: String,
    /// Approved placeholder policy.
    pub placeholder_policy: String,
    /// Fixed synthetic identifiers.
    pub ids: PersistenceIds,
    /// Fixed baseline key forms.
    pub keys: PersistenceKeys,
    /// Exact nine conceptual cases.
    pub cases: [PersistenceCaseRecord; PERSISTENCE_CASES],
    /// Intentionally malformed or truncated fixture paths.
    pub intentional_invalid_files: [String; 3],
    /// Exact baseline side-store paths.
    pub side_stores: [SideStoreRecord; SIDE_STORES],
    /// Explicit complete-inventory contract, including the index scan rule.
    pub inventory_schema: String,
    /// Every corpus file except self-referential `index.json`.
    #[serde(deserialize_with = "deserialize_inventory")]
    pub inventory: [InventoryEntry; INVENTORY_ENTRIES],
}

/// Loads and validates the fixed persistence index and every indexed fixture path.
///
/// # Errors
/// Returns a typed fixture error when the index, metadata, case order, path confinement, or an
/// indexed fixture is invalid.
pub fn load_index(loader: &FixtureLoader) -> Result<PersistenceIndex, TestkitError> {
    let index: PersistenceIndex = loader.load("persistence/index.json")?;
    validate_metadata(&index)?;
    validate_cases(&index)?;
    validate_inventory(&index)?;
    validate_paths(loader, &index)?;
    Ok(index)
}

fn deserialize_inventory<'de, D>(
    deserializer: D,
) -> Result<[InventoryEntry; INVENTORY_ENTRIES], D::Error>
where
    D: serde::Deserializer<'de>,
{
    let entries = Vec::<InventoryEntry>::deserialize(deserializer)?;
    entries.try_into().map_err(|values: Vec<InventoryEntry>| {
        serde::de::Error::invalid_length(values.len(), &"exact persistence inventory")
    })
}

fn malformed() -> TestkitError {
    TestkitError::MalformedFixture {
        path: "fixtures/persistence/index.json".into(),
    }
}

fn confined(path: &str) -> Result<(), TestkitError> {
    let value = Path::new(path);
    if value.as_os_str().is_empty() || value.is_absolute() {
        return Err(malformed());
    }
    if value.components().count() > TESTKIT_ITEMS_MAX
        || value
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(malformed());
    }
    Ok(())
}

fn validate_metadata(index: &PersistenceIndex) -> Result<(), TestkitError> {
    if index.schema_version != 1
        || index.source_commit != SOURCE_COMMIT
        || index.generator != "tools/extract-persistence-fixtures.mjs"
        || index.ids.agent != "agent-local-fixture"
        || index.ids.conversation != "conversation-fixture"
    {
        return Err(malformed());
    }
    Ok(())
}

fn validate_cases(index: &PersistenceIndex) -> Result<(), TestkitError> {
    for (position, record) in index.cases.iter().enumerate() {
        if *record != expected_case(position) {
            return Err(malformed());
        }
        confined(&record.relative_root)?;
    }
    Ok(())
}

fn expected_case(position: usize) -> PersistenceCaseRecord {
    let source_format = [
        SourceFormat::Schema2PiSessionEntryJsonl,
        SourceFormat::Schema2PiSessionEntryJsonl,
        SourceFormat::UnversionedLegacyUiJsonl,
        SourceFormat::Schema1PiAiMessageJsonl,
        SourceFormat::Schema2ToleratedRows,
        SourceFormat::Schema2OrphanToolResult,
        SourceFormat::Schema2TruncatedJsonl,
        SourceFormat::Schema2ReplacementEvidence,
        SourceFormat::CorruptAndUnsupportedManifests,
    ][position];
    PersistenceCaseRecord {
        case_id: [1, 2, 3, 4, 5, 6, 7, 8, 9][position],
        obligation: [1, 2, 3, 3, 4, 5, 6, 6, 7][position],
        relative_root: CASE_ROOTS[position].into(),
        source_format,
        expected_rust_behavior: expected_rust(position),
        expected_typescript_behavior: expected_typescript(position),
        expected_recovery_or_mutation: expected_mutation(position),
    }
}

fn expected_rust(position: usize) -> RustBehavior {
    [
        RustBehavior::ReadsCurrent,
        RustBehavior::WritesExactWire,
        RustBehavior::RequiresExplicitMigration,
        RustBehavior::UpgradesOnNonemptyPersistence,
        RustBehavior::LoadsMessageEntries,
        RustBehavior::RepairsActiveProjection,
        RustBehavior::RecoversCompletePrefix,
        RustBehavior::PreservesOriginal,
        RustBehavior::RejectsWithoutMutation,
    ][position]
}

fn expected_typescript(position: usize) -> TypescriptBehavior {
    [
        TypescriptBehavior::WritesCurrent,
        TypescriptBehavior::ReadsExactWire,
        TypescriptBehavior::RequiresExplicitMigration,
        TypescriptBehavior::UpgradesOnNonemptyPersistence,
        TypescriptBehavior::IgnoresSessionHeader,
        TypescriptBehavior::RepairsActiveProjection,
        TypescriptBehavior::NotApplicableRustHardening,
        TypescriptBehavior::NotApplicableRustHardening,
        TypescriptBehavior::RejectsWithoutMutation,
    ][position]
}

fn expected_mutation(position: usize) -> MutationDisposition {
    [
        MutationDisposition::None,
        MutationDisposition::None,
        MutationDisposition::BackupThenConversion,
        MutationDisposition::Schema2Rewrite,
        MutationDisposition::None,
        MutationDisposition::ConversationOnly,
        MutationDisposition::DiscardTruncatedTail,
        MutationDisposition::CandidateNotCommitted,
        MutationDisposition::None,
    ][position]
}

fn validate_inventory(index: &PersistenceIndex) -> Result<(), TestkitError> {
    let mut previous = "";
    for entry in &index.inventory {
        confined(&entry.path)?;
        if entry.path.as_str() <= previous
            || !valid_sha256(&entry.sha256)
            || entry.path == "index.json"
        {
            return Err(malformed());
        }
        previous = &entry.path;
    }
    if !index
        .inventory_schema
        .contains("index.json is additionally included")
    {
        return Err(malformed());
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_paths(loader: &FixtureLoader, index: &PersistenceIndex) -> Result<(), TestkitError> {
    validate_top_level(loader)?;
    let actual = loader.list_tree("persistence")?;
    let mut expected: Vec<String> = index
        .inventory
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    expected.push("index.json".into());
    expected.sort();
    if actual != expected {
        return Err(malformed());
    }
    for entry in &index.inventory {
        let bytes = loader.load_bytes(format!("persistence/{}", entry.path))?;
        if bytes.len() != entry.byte_count
            || !constant_time_equal(&sha256::lowercase_hex(&bytes), &entry.sha256)
        {
            return Err(malformed());
        }
    }
    loader.load_bytes("persistence/index.json")?;
    Ok(())
}

fn validate_top_level(loader: &FixtureLoader) -> Result<(), TestkitError> {
    let mut expected = CASE_ROOTS
        .iter()
        .map(|root| format!("{root}/"))
        .collect::<Vec<_>>();
    expected.extend(["index.json".into(), "side_stores/".into()]);
    expected.sort();
    if loader.list_children("persistence")? != expected {
        return Err(malformed());
    }
    Ok(())
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests;
