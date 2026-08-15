//! Bounded loader, ordering assertions, and semantic comparator for reference traces.

use crate::TestkitError;
use crate::fixtures::FixtureLoader;
use crate::fixtures::sha256::lowercase_hex;
use chrono::DateTime;
use lotta_domain::{BoundedJsonValue, BoundedVec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt as formatting;
use uuid::Uuid;

const RELIABILITY_COUNT: usize = 7;
const INVARIANT_COUNT: usize = 6;
const CASE_COUNT: usize = 8;
const INVENTORY_COUNT: usize = 8;
const SOURCE_FILES_COUNT: usize = 20;
const SOURCE_REGIONS_COUNT: usize = 35;
const SUPPORTING_MAX: usize = 12;
const SEMANTIC_RULES_COUNT: usize = 5;
/// Maximum frames retained in one reference trace.
pub const TRACE_FRAMES_MAX: usize = 64;
const TRACE_STRING_BYTES_MAX: usize = 512;
const TRACE_AGGREGATE_BYTES_MAX: usize = 524_288;
const SOURCE_COMMIT: &str = "300f923f16cc8eee50656d7da732902c1dea2b65";
const BOUNDARY: &str = "pinned AppServerClient + pinned source-test scenario boundary";
const PLACEHOLDER: &str = "<sanitized-trace>";
const TRACE_NAMES: [&str; CASE_COUNT] = [
    "queue",
    "abort",
    "disconnect",
    "stale-lease",
    "retry",
    "idempotency",
    "crash-recovery",
    "vertical-slice",
];
const GENERATED_UUID_PATHS: [&str; 11] = [
    "/wire/agent_id",
    "/wire/conversation_id",
    "/wire/runtime/agent_id",
    "/wire/runtime/conversation_id",
    "/wire/turn_id",
    "/wire/run_id",
    "/wire/delta/id",
    "/wire/delta/tool_call_id",
    "/wire/queue/*/id",
    "/wire/loop_status/active_run_ids/*",
    "/wire/loop_status/executing_tool_call_ids/*",
];
const TIMESTAMP_PATHS: [&str; 3] = [
    "/wire/emitted_at",
    "/wire/delta/date",
    "/wire/queue/*/enqueued_at",
];
const INVARIANT_PROVENANCE: [(OrderingInvariant, &str, &str); INVARIANT_COUNT] = [
    (
        OrderingInvariant::IncreasingEventSeqPerConnection,
        "src/websocket/listener/protocol-outbound.ts",
        "emitProtocolV2Message",
    ),
    (
        OrderingInvariant::InputAcceptedBeforeCausedEvents,
        "src/websocket/listener/message-router.test.ts",
        "test:preserves the acting user on a directly-owned input and deduplicates retries",
    ),
    (
        OrderingInvariant::ToolStartBeforeMatchingToolEnd,
        "src/websocket/listener/loop-state-executing-tools.test.ts",
        "test:reuses the approval request message id for tool lifecycle rows",
    ),
    (
        OrderingInvariant::TurnFinishedExactlyOnceAfterFinalStreamDelta,
        "src/websocket/listener/turn-terminal-protocol.test.ts",
        "test:finishListenerTurn emits exactly one correlated terminal event",
    ),
    (
        OrderingInvariant::NoServerEventFromStaleLeaseAfterReplacement,
        "src/websocket/listener/recovery-lease.test.ts",
        "test:stale recovered tool execution emits nothing into a replacement run",
    ),
    (
        OrderingInvariant::BroadcastDeliveryStableAscendingConnectionOrdinal,
        "src/websocket/listener/protocol-outbound.test.ts",
        "test:fans notifications out to subscribers and honors an explicit target",
    ),
];

/// Exact compatibility reliability surfaces in specification order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ReliabilitySurface {
    /// Queue transition behavior.
    Queue,
    /// Abort behavior.
    Abort,
    /// Disconnect cleanup and replay.
    Disconnect,
    /// Stale lease suppression.
    StaleLease,
    /// Retry lifecycle.
    Retry,
    /// Input admission idempotency.
    Idempotency,
    /// Crash recovery.
    CrashRecovery,
}
impl ReliabilitySurface {
    /// All seven surfaces independent of fixture metadata.
    pub const ALL: [Self; RELIABILITY_COUNT] = [
        Self::Queue,
        Self::Abort,
        Self::Disconnect,
        Self::StaleLease,
        Self::Retry,
        Self::Idempotency,
        Self::CrashRecovery,
    ];
}

/// Six executable ordering requirements in specification order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderingInvariant {
    /// Event sequence increases per connection.
    IncreasingEventSeqPerConnection,
    /// Admission acknowledgement precedes caused events.
    InputAcceptedBeforeCausedEvents,
    /// Tool start precedes matching end.
    ToolStartBeforeMatchingToolEnd,
    /// One terminal follows the final delta.
    TurnFinishedExactlyOnceAfterFinalStreamDelta,
    /// A replaced lease emits no later event.
    NoServerEventFromStaleLeaseAfterReplacement,
    /// Broadcast delivery follows connection ordinal.
    BroadcastDeliveryStableAscendingConnectionOrdinal,
}
impl OrderingInvariant {
    /// Exact invariant list independent of index contents.
    pub const ALL: [Self; INVARIANT_COUNT] = [
        Self::IncreasingEventSeqPerConnection,
        Self::InputAcceptedBeforeCausedEvents,
        Self::ToolStartBeforeMatchingToolEnd,
        Self::TurnFinishedExactlyOnceAfterFinalStreamDelta,
        Self::NoServerEventFromStaleLeaseAfterReplacement,
        Self::BroadcastDeliveryStableAscendingConnectionOrdinal,
    ];
}

/// Direction of a trace observation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameDirection {
    /// Actual client command.
    ClientToServer,
    /// Actual server event or response.
    ServerToClient,
    /// Non-protocol lifecycle observation.
    Lifecycle,
}

/// One bounded trace frame with protocol JSON isolated in `wire`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TraceFrame {
    /// Exact zero-based index.
    pub frame_index: usize,
    /// Observation direction.
    pub direction: FrameDirection,
    /// Stable connection ordinal.
    pub connection_ordinal: u32,
    /// Sanitized client message causal identity.
    pub caused_by: Option<String>,
    /// Lease identity observation.
    pub lease_id: Option<String>,
    /// Whether the observed lease is current.
    pub lease_current: Option<bool>,
    /// Same-emission broadcast label.
    pub broadcast_emission: Option<String>,
    /// Actual protocol or lifecycle JSON.
    pub wire: BoundedJsonValue,
}

/// One complete bounded reference trace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReferenceTrace {
    /// Trace schema version.
    pub schema_version: u8,
    /// Stable case name.
    pub name: String,
    /// Semantic case kind.
    pub kind: String,
    /// Source provenance.
    pub provenance: Provenance,
    /// Exact bounded supporting source provenance.
    pub supporting_provenance: BoundedVec<Provenance, SUPPORTING_MAX>,
    /// Actual command/message projection proving the client driver.
    pub driver_proof: TraceDriverProof,
    /// Ordered frames.
    pub frames: BoundedVec<TraceFrame, TRACE_FRAMES_MAX>,
}

/// Indexed source-test provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Provenance {
    /// Repository-relative source path.
    pub path: String,
    /// Test or operative symbol.
    pub symbol: String,
    /// Honest capture boundary.
    pub capture_boundary: String,
}

/// One pinned source file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct SourceFile {
    /// Relative path.
    pub path: String,
    /// Whole-file SHA-256.
    pub sha256: String,
}
/// One pinned operative source region.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct SourceRegion {
    /// Relative path.
    pub path: String,
    /// Unique symbol.
    pub symbol: String,
    /// Balanced lexical region SHA-256.
    pub sha256: String,
}
/// Source provenance for one invariant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct InvariantProvenance {
    /// Ordering invariant justified by this source.
    pub invariant: OrderingInvariant,
    /// Source file path.
    pub path: String,
    /// Exact operative symbol.
    pub symbol: String,
}
/// One fixed semantic path rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct SemanticRule {
    /// Rule identifier.
    pub rule: String,
    /// Exact paths covered by the rule.
    pub paths: BoundedVec<String, 12>,
}
/// One indexed trace case.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct TraceCase {
    /// Case name.
    pub name: String,
    /// Fixture path.
    pub path: String,
    /// Semantic kind.
    pub kind: String,
    /// Source provenance.
    pub provenance: Provenance,
    /// Exact bounded supporting provenance copied from the trace.
    pub supporting_provenance: BoundedVec<Provenance, SUPPORTING_MAX>,
    /// Exact projection copied from the trace.
    pub driver_proof: TraceDriverProof,
}
/// One corpus inventory entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct InventoryEntry {
    /// Relative path.
    pub path: String,
    /// Parser kind.
    pub kind: String,
    /// Exact bytes.
    pub bytes: usize,
    /// Exact SHA-256.
    pub sha256: String,
}
/// Bounded proof projected from actual trace directions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TraceDriverProof {
    /// Actual client-to-server wire discriminants in frame order.
    pub command_types: BoundedVec<String, TRACE_FRAMES_MAX>,
    /// Actual server-to-client wire discriminants in frame order.
    pub message_types: BoundedVec<String, TRACE_FRAMES_MAX>,
}
/// Complete fixed-shape corpus index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ReferenceTraceIndex {
    /// Schema version.
    pub schema_version: u8,
    /// Pinned source commit.
    pub source_commit: String,
    /// Generator path.
    pub generator: String,
    /// Capture boundary.
    pub capture_boundary: String,
    /// Explicit non-live boundary detail.
    pub boundary_detail: String,
    /// Exact pinned files.
    pub source_files: [SourceFile; SOURCE_FILES_COUNT],
    /// Exact operative regions.
    pub source_regions: BoundedVec<SourceRegion, SOURCE_REGIONS_COUNT>,
    /// Exact surfaces.
    pub reliability_surfaces: [ReliabilitySurface; RELIABILITY_COUNT],
    /// Exact invariants.
    pub ordering_invariants: [OrderingInvariant; INVARIANT_COUNT],
    /// Exact source provenance for each invariant.
    pub invariant_provenance: [InvariantProvenance; INVARIANT_COUNT],
    /// Exact semantic path table.
    pub semantic_rules: [SemanticRule; SEMANTIC_RULES_COUNT],
    /// Exact cases.
    pub cases: [TraceCase; CASE_COUNT],
    /// Exact trace inventory.
    pub inventory: [InventoryEntry; INVENTORY_COUNT],
}

/// Bounded frame summary in a typed invariant report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameSummary {
    /// Zero-based frame index.
    pub index: usize,
    /// Wire discriminant.
    pub frame_type: String,
    /// Connection ordinal.
    pub connection_ordinal: u32,
}
/// One typed ordering invariant failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceInvariantViolation {
    /// Failed invariant.
    pub invariant: OrderingInvariant,
    /// First offending frame.
    pub first_frame: FrameSummary,
    /// Second offending frame, absent only when a counterpart is missing.
    pub second_frame: Option<FrameSummary>,
}
impl formatting::Display for TraceInvariantViolation {
    fn fmt(&self, formatter: &mut formatting::Formatter<'_>) -> formatting::Result {
        std::write!(
            formatter,
            "trace invariant {:?} failed at {}:{}:{}",
            self.invariant,
            self.first_frame.index,
            self.first_frame.frame_type,
            self.first_frame.connection_ordinal
        )?;
        if let Some(second) = &self.second_frame {
            std::write!(
                formatter,
                " and {}:{}:{}",
                second.index,
                second.frame_type,
                second.connection_ordinal
            )?;
        }
        Ok(())
    }
}
impl std::error::Error for TraceInvariantViolation {}

/// First semantic mismatch after both traces pass validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticDivergence {
    /// Exact frame index.
    pub frame_index: usize,
    /// JSON pointer-like path.
    pub path: String,
    /// Bounded expected summary.
    pub expected: String,
    /// Bounded actual summary.
    pub actual: String,
}
/// Comparator failure precedence: ordering before semantic divergence.
#[derive(Debug)]
pub enum TraceComparisonError {
    /// Ordering or metadata validation failed.
    Ordering(TraceInvariantViolation),
    /// First semantic divergence.
    Semantic(SemanticDivergence),
}
impl formatting::Display for TraceComparisonError {
    fn fmt(&self, formatter: &mut formatting::Formatter<'_>) -> formatting::Result {
        match self {
            Self::Ordering(value) => value.fmt(formatter),
            Self::Semantic(value) => std::write!(
                formatter,
                "semantic divergence at frame {} {}",
                value.frame_index,
                value.path
            ),
        }
    }
}
impl std::error::Error for TraceComparisonError {}

/// Corpus loading and validation error.
#[derive(Debug, thiserror::Error)]
pub enum ReferenceTraceError {
    /// Generic fixture loader error.
    #[error("reference trace fixture load failed")]
    Fixture(#[from] TestkitError),
    /// Corpus metadata, integrity, or sanitization failure.
    #[error("reference trace corpus validation failed: {0}")]
    Invalid(&'static str),
    /// Ordering validation failure.
    #[error(transparent)]
    Ordering(#[from] TraceInvariantViolation),
}

fn reject_duplicate_json_keys(bytes: &[u8]) -> Result<(), ReferenceTraceError> {
    struct Seed;
    impl<'de> serde::de::DeserializeSeed<'de> for Seed {
        type Value = ();
        fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_any(Visitor)
        }
    }
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = ();
        fn expecting(&self, formatter: &mut formatting::Formatter<'_>) -> formatting::Result {
            formatter.write_str("bounded JSON value")
        }
        fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut keys = BTreeSet::new();
            while let Some(key) = map.next_key::<String>()? {
                if !keys.insert(key) {
                    return Err(serde::de::Error::custom("duplicate object key"));
                }
                map.next_value_seed(Seed)?;
            }
            Ok(())
        }
        fn visit_seq<A>(self, mut sequence: A) -> Result<(), A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            while sequence.next_element_seed(Seed)?.is_some() {}
            Ok(())
        }
        fn visit_bool<E>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
        fn visit_none<E>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E>(self) -> Result<(), E> {
            Ok(())
        }
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    serde::de::DeserializeSeed::deserialize(Seed, &mut deserializer)
        .map_err(|_| ReferenceTraceError::Invalid("duplicate or malformed JSON"))?;
    deserializer
        .end()
        .map_err(|_| ReferenceTraceError::Invalid("duplicate or malformed JSON"))
}

/// Loads and validates the complete index and all eight traces.
///
/// # Errors
/// Returns a bounded typed error for tree, hash, schema, or trace validation failure.
pub fn load_all() -> Result<(ReferenceTraceIndex, [ReferenceTrace; 8]), ReferenceTraceError> {
    let loader = FixtureLoader::new();
    let index: ReferenceTraceIndex = loader.load("reference-traces/index.json")?;
    validate_index(&loader, &index)?;
    let mut traces = Vec::with_capacity(CASE_COUNT);
    let mut aggregate = 0_usize;
    for record in &index.cases {
        let bytes = loader.load_bytes(format!("reference-traces/{}", record.path))?;
        aggregate = aggregate.saturating_add(bytes.len());
        if aggregate > TRACE_AGGREGATE_BYTES_MAX {
            return Err(ReferenceTraceError::Invalid("aggregate bytes"));
        }
        reject_duplicate_json_keys(&bytes)?;
        let trace: ReferenceTrace = serde_json::from_slice(&bytes)
            .map_err(|_| ReferenceTraceError::Invalid("trace JSON"))?;
        validate_trace(&trace)?;
        validate_case(record, &trace, &index.source_regions, &index.source_files)?;
        traces.push(trace);
    }
    traces
        .try_into()
        .map_err(|_| ReferenceTraceError::Invalid("trace count"))
        .map(|traces| (index, traces))
}

/// Loads and validates one named trace.
///
/// # Errors
/// Returns a bounded typed error when the name or trace is invalid.
pub fn load_trace(name: &str) -> Result<ReferenceTrace, ReferenceTraceError> {
    let (_, traces) = load_all()?;
    traces
        .into_iter()
        .find(|trace| trace.name == name)
        .ok_or(ReferenceTraceError::Invalid("unknown trace"))
}

fn validate_index(
    loader: &FixtureLoader,
    index: &ReferenceTraceIndex,
) -> Result<(), ReferenceTraceError> {
    if index.schema_version != 1
        || index.source_commit != SOURCE_COMMIT
        || index.generator != "tools/capture-reference-traces.mjs"
        || index.capture_boundary != BOUNDARY
        || index.reliability_surfaces != ReliabilitySurface::ALL
        || index.ordering_invariants != OrderingInvariant::ALL
    {
        return Err(ReferenceTraceError::Invalid("index identity"));
    }
    let tree = loader.list_tree("reference-traces")?;
    let mut expected = std::iter::once("index.json".to_owned())
        .chain(index.inventory.iter().map(|entry| entry.path.clone()))
        .collect::<Vec<_>>();
    expected.sort();
    if tree != expected || tree.len() != CASE_COUNT + 1 {
        return Err(ReferenceTraceError::Invalid("exact tree"));
    }
    validate_inventory(loader, index)?;
    validate_provenance(index)?;
    validate_rule_table(index)?;
    validate_sanitization(loader, &tree)
}

fn validate_provenance(index: &ReferenceTraceIndex) -> Result<(), ReferenceTraceError> {
    let files = index
        .source_files
        .iter()
        .map(|x| x.path.as_str())
        .collect::<BTreeSet<_>>();
    let regions = index
        .source_regions
        .as_slice()
        .iter()
        .map(|x| (x.path.as_str(), x.symbol.as_str()))
        .collect::<BTreeSet<_>>();
    if files.len() != SOURCE_FILES_COUNT || regions.len() != SOURCE_REGIONS_COUNT {
        return Err(ReferenceTraceError::Invalid("source metadata uniqueness"));
    }
    if index
        .source_regions
        .as_slice()
        .iter()
        .any(|x| !files.contains(x.path.as_str()))
    {
        return Err(ReferenceTraceError::Invalid("source region path"));
    }
    for (source, expected) in index.invariant_provenance.iter().zip(INVARIANT_PROVENANCE) {
        if (
            source.invariant,
            source.path.as_str(),
            source.symbol.as_str(),
        ) != expected
            || !regions.contains(&(source.path.as_str(), source.symbol.as_str()))
        {
            return Err(ReferenceTraceError::Invalid("invariant provenance"));
        }
    }
    Ok(())
}

fn validate_case(
    case: &TraceCase,
    trace: &ReferenceTrace,
    regions: &BoundedVec<SourceRegion, SOURCE_REGIONS_COUNT>,
    files: &[SourceFile; SOURCE_FILES_COUNT],
) -> Result<(), ReferenceTraceError> {
    let resolves = regions
        .as_slice()
        .iter()
        .any(|x| x.path == case.provenance.path && x.symbol == case.provenance.symbol);
    let source_path = files.iter().any(|x| x.path == case.provenance.path);
    let supporting_resolves =
        case.supporting_provenance
            .as_slice()
            .iter()
            .all(|reference| {
                regions.as_slice().iter().any(|region| {
                    region.path == reference.path && region.symbol == reference.symbol
                }) && files.iter().any(|file| file.path == reference.path)
                    && reference.capture_boundary == BOUNDARY
            });
    if case.name != trace.name
        || case.kind != trace.kind
        || case.provenance != trace.provenance
        || case.supporting_provenance != trace.supporting_provenance
        || case.driver_proof != trace.driver_proof
        || !resolves
        || !source_path
        || !supporting_resolves
        || case.supporting_provenance.is_empty()
    {
        return Err(ReferenceTraceError::Invalid("case identity or provenance"));
    }
    Ok(())
}

fn validate_inventory(
    loader: &FixtureLoader,
    index: &ReferenceTraceIndex,
) -> Result<(), ReferenceTraceError> {
    let mut previous = "";
    for entry in &index.inventory {
        if entry.path.as_str() <= previous
            || entry.kind != "json"
            || !TRACE_NAMES
                .iter()
                .any(|name| entry.path == format!("{name}.json"))
        {
            return Err(ReferenceTraceError::Invalid("inventory metadata"));
        }
        let bytes = loader.load_bytes(format!("reference-traces/{}", entry.path))?;
        if bytes.len() != entry.bytes || lowercase_hex(&bytes) != entry.sha256 {
            return Err(ReferenceTraceError::Invalid("inventory integrity"));
        }
        previous = &entry.path;
    }
    Ok(())
}

fn validate_rule_table(index: &ReferenceTraceIndex) -> Result<(), ReferenceTraceError> {
    let rules = &index.semantic_rules;
    if rules[0].rule != "generated_uuid_alias"
        || rules[0].paths.as_slice() != GENERATED_UUID_PATHS
        || rules[1].rule != "rfc3339_timestamp"
        || rules[1].paths.as_slice() != TIMESTAMP_PATHS
        || rules[2].rule != "event_seq_rank"
        || rules[2].paths.as_slice() != ["/wire/event_seq"]
        || rules[3].rule != "idempotency_key_unique_emission"
        || rules[3].paths.as_slice() != ["/wire/idempotency_key"]
        || rules[4].rule != "exact"
        || rules[4].paths.as_slice() != ["*"]
    {
        return Err(ReferenceTraceError::Invalid("semantic rule table"));
    }
    Ok(())
}

fn validate_sanitization(
    loader: &FixtureLoader,
    tree: &[String],
) -> Result<(), ReferenceTraceError> {
    for path in tree {
        let bytes = loader.load_bytes(format!("reference-traces/{path}"))?;
        scan_sanitized_json(&bytes, path != "index.json")?;
    }
    Ok(())
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn forbidden_text(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let compact = normalized(value);
    lower.contains("bearer ")
        || lower.contains("private key")
        || lower.contains("-----begin")
        || compact.contains("apikey")
        || compact.contains("password")
        || compact.contains("accesstoken")
        || compact.contains("refreshtoken")
        || compact.contains("clientsecret")
        || compact.contains("oauth")
        || compact.contains("secretkey")
        || compact.contains("awsaccesskey")
        || compact.contains("googleapplicationcredentials")
        || looks_like_jwt(value)
}

fn looks_like_jwt(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!((parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(a), Some(b), Some(c), None) if a.starts_with("eyJ") && !b.is_empty() && !c.is_empty())
}

fn payload_key(key: &str, parent: Option<&str>) -> bool {
    matches!(
        normalized(key).as_str(),
        "content" | "toolargs" | "toolinput" | "tooloutput" | "prompt" | "input" | "output"
    ) || (key == "delta" && parent == Some("message"))
}

fn scan_value(value: &Value, parent: Option<&str>) -> Result<(), ReferenceTraceError> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if forbidden_text(key) {
                    return Err(ReferenceTraceError::Invalid("sanitization"));
                }
                if payload_key(key, parent)
                    && child.as_str().is_some_and(|text| text != PLACEHOLDER)
                {
                    return Err(ReferenceTraceError::Invalid("payload sanitization"));
                }
                scan_value(child, Some(key))?;
            }
        }
        Value::Array(values) => {
            for child in values {
                scan_value(child, parent)?;
            }
        }
        Value::String(text) if forbidden_text(text) => {
            return Err(ReferenceTraceError::Invalid("sanitization"));
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn scan_sanitized_json(
    bytes: &[u8],
    require_placeholder: bool,
) -> Result<(), ReferenceTraceError> {
    let raw = std::str::from_utf8(bytes).map_err(|_| ReferenceTraceError::Invalid("non-UTF8"))?;
    if forbidden_text(raw) {
        return Err(ReferenceTraceError::Invalid("raw sanitization"));
    }
    reject_duplicate_json_keys(bytes)?;
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| ReferenceTraceError::Invalid("sanitization JSON"))?;
    scan_value(&value, None)?;
    if require_placeholder && !raw.contains(PLACEHOLDER) {
        return Err(ReferenceTraceError::Invalid("placeholder"));
    }
    Ok(())
}

fn validate_trace(trace: &ReferenceTrace) -> Result<(), ReferenceTraceError> {
    if trace.schema_version != 1
        || !TRACE_NAMES.contains(&trace.name.as_str())
        || trace.provenance.capture_boundary != BOUNDARY
        || trace.frames.is_empty()
    {
        return Err(ReferenceTraceError::Invalid("trace metadata"));
    }
    for (index, frame) in trace.frames.as_slice().iter().enumerate() {
        if frame.frame_index != index
            || frame.connection_ordinal == 0
            || frame
                .caused_by
                .as_ref()
                .is_some_and(|value| !safe_string(value))
            || frame
                .lease_id
                .as_ref()
                .is_some_and(|value| !safe_string(value))
        {
            return Err(ReferenceTraceError::Invalid("frame metadata"));
        }
        validate_wire(frame)?;
    }
    validate_driver_proof(trace)?;
    validate_idempotency_keys(trace)?;
    assert_ordering(trace)?;
    Ok(())
}

fn validate_wire(frame: &TraceFrame) -> Result<(), ReferenceTraceError> {
    let wire = frame.wire.as_value();
    let object = wire
        .as_object()
        .ok_or(ReferenceTraceError::Invalid("wire object"))?;
    let frame_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or(ReferenceTraceError::Invalid("wire type"))?;
    if !safe_string(frame_type) {
        return Err(ReferenceTraceError::Invalid("wire type bound"));
    }
    let broadcast = is_broadcast(frame_type);
    if frame.direction == FrameDirection::ServerToClient && broadcast {
        validate_envelope(object)?;
    } else if object.contains_key("event_seq")
        || object.contains_key("idempotency_key")
        || object.contains_key("emitted_at")
    {
        return Err(ReferenceTraceError::Invalid("invented envelope"));
    }
    if frame.direction == FrameDirection::Lifecycle && broadcast {
        return Err(ReferenceTraceError::Invalid("protocol lifecycle direction"));
    }
    Ok(())
}

fn validate_envelope(object: &serde_json::Map<String, Value>) -> Result<(), ReferenceTraceError> {
    let runtime = object
        .get("runtime")
        .and_then(Value::as_object)
        .ok_or(ReferenceTraceError::Invalid("runtime envelope"))?;
    for key in ["agent_id", "conversation_id"] {
        parse_uuid(runtime.get(key).and_then(Value::as_str))?;
    }
    let sequence = object
        .get("event_seq")
        .and_then(Value::as_u64)
        .ok_or(ReferenceTraceError::Invalid("event sequence"))?;
    let emitted = object
        .get("emitted_at")
        .and_then(Value::as_str)
        .ok_or(ReferenceTraceError::Invalid("emitted time"))?;
    parse_time(emitted)?;
    let key = object
        .get("idempotency_key")
        .and_then(Value::as_str)
        .ok_or(ReferenceTraceError::Invalid("idempotency key"))?;
    let expected = format!(
        "{}:{sequence}:",
        object["type"].as_str().unwrap_or_default()
    );
    if !key.starts_with(&expected) || parse_uuid(key.strip_prefix(&expected))?.is_nil() {
        return Err(ReferenceTraceError::Invalid("idempotency key consistency"));
    }
    Ok(())
}

fn validate_driver_proof(trace: &ReferenceTrace) -> Result<(), ReferenceTraceError> {
    let commands = trace
        .frames
        .as_slice()
        .iter()
        .filter(|x| x.direction == FrameDirection::ClientToServer)
        .map(|x| wire_type(x).to_owned())
        .collect::<Vec<_>>();
    let messages = trace
        .frames
        .as_slice()
        .iter()
        .filter(|x| x.direction == FrameDirection::ServerToClient)
        .map(|x| wire_type(x).to_owned())
        .collect::<Vec<_>>();
    if commands.is_empty()
        || messages.is_empty()
        || commands.as_slice() != trace.driver_proof.command_types.as_slice()
        || messages.as_slice() != trace.driver_proof.message_types.as_slice()
    {
        return Err(ReferenceTraceError::Invalid("driver proof projection"));
    }
    Ok(())
}

fn validate_idempotency_keys(trace: &ReferenceTrace) -> Result<(), ReferenceTraceError> {
    let mut keys = BTreeSet::new();
    for frame in trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| is_server_broadcast(frame))
    {
        let key = wire_string(frame, "idempotency_key").unwrap_or_default();
        let emission = frame.broadcast_emission.as_deref().unwrap_or_default();
        if emission.is_empty() || !safe_string(emission) {
            return Err(ReferenceTraceError::Invalid("broadcast emission"));
        }
        if !keys.insert(key) {
            return Err(ReferenceTraceError::Invalid("duplicate idempotency key"));
        }
    }
    Ok(())
}

#[path = "traces_ordering.rs"]
mod traces_ordering;
pub use traces_ordering::{assert_invariant, assert_ordering, compare_semantic};
use traces_ordering::{
    is_broadcast, is_server_broadcast, parse_time, parse_uuid, safe_string, wire_string, wire_type,
};
#[cfg(test)]
use traces_ordering::{
    is_generated_path, is_timestamp_path, logical_payload_sha256, nested_string, wire_u64,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
#[path = "traces_authority_tests.rs"]
mod traces_authority_tests;
