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
const DERIVED_COUNT: usize = 1;
const DERIVED_NAME: &str = "slice_happy_turn";
const DERIVED_PATH: &str = "slice_happy_turn.json";
const DERIVED_KIND: &str = "vertical_slice";
const DERIVED_SOURCE: &str = "vertical-slice.json";
const DERIVED_PROJECTION: &str = concat!(
    "source frame indices 0..14 plus 30,31; frame_index renumbered, terminal subscriber ",
    "projected to 1, and final emission-9 relabeled emission-5 only"
);
const DERIVED_SOURCE_INDICES: [usize; 17] =
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 30, 31];
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
/// One exact subscriber projection applied to a source lifecycle frame.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct SubscriberProjection {
    /// Source frame index containing `broadcast_begin`.
    pub source_frame_index: usize,
    /// Exact sorted nonempty recipient ordinals.
    pub subscriber_ordinals: BoundedVec<u64, 16>,
}
/// One exact emission-label rewrite.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct EmissionRelabel {
    /// Existing source emission ordinal.
    pub from: u64,
    /// Projected emission ordinal.
    pub to: u64,
}
/// Bounded declarative frame projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct DerivedTraceProjection {
    /// Exact ordered source frame indices.
    pub source_frame_indices: BoundedVec<usize, TRACE_FRAMES_MAX>,
    /// Whether output frame indexes are renumbered from zero.
    pub renumber_frame_indices: bool,
    /// Exact bounded subscriber rewrites.
    pub subscriber_projections: BoundedVec<SubscriberProjection, 4>,
    /// Exact bounded emission rewrites.
    pub emission_relabels: BoundedVec<EmissionRelabel, 4>,
}
/// One derived trace case whose bytes are generated from an authoritative source case.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct DerivedTraceCase {
    /// Stable derived case name.
    pub name: String,
    /// Fixture path.
    pub path: String,
    /// Semantic kind.
    pub kind: String,
    /// Canonical source case path.
    pub derived_from: String,
    /// Human-readable generated projection description.
    pub projection: String,
    /// Typed declarative projection.
    pub projection_declaration: DerivedTraceProjection,
    /// Exact bytes.
    pub bytes: usize,
    /// Exact SHA-256.
    pub sha256: String,
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
    /// Exact authoritative source cases.
    pub cases: [TraceCase; CASE_COUNT],
    /// Exact derived cases.
    pub derived_cases: [DerivedTraceCase; DERIVED_COUNT],
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

mod implementation;
pub use implementation::*;
