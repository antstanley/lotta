#[cfg(test)]
use super::INVENTORY_COUNT;
use super::{
    BOUNDARY, BTreeSet, BoundedJsonValue, BoundedVec, CASE_COUNT, DERIVED_COUNT, DERIVED_KIND,
    DERIVED_NAME, DERIVED_PATH, DERIVED_PROJECTION, DERIVED_SOURCE, DERIVED_SOURCE_INDICES,
    DateTime, DerivedTraceCase, FixtureLoader, FrameDirection, FrameSummary, GENERATED_UUID_PATHS,
    INVARIANT_PROVENANCE, OrderingInvariant, PLACEHOLDER, ReferenceTrace, ReferenceTraceError,
    ReferenceTraceIndex, ReliabilitySurface, SOURCE_COMMIT, SOURCE_FILES_COUNT,
    SOURCE_REGIONS_COUNT, SemanticDivergence, SourceFile, SourceRegion, TIMESTAMP_PATHS,
    TRACE_AGGREGATE_BYTES_MAX, TRACE_FRAMES_MAX, TRACE_NAMES, TRACE_STRING_BYTES_MAX, TraceCase,
    TraceComparisonError, TraceDriverProof, TraceFrame, TraceInvariantViolation, Uuid, Value,
    formatting, lowercase_hex,
};

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

/// Loads and validates the corpus index and returns one derived declaration.
///
/// # Errors
/// Returns a bounded typed error when the corpus or derived name is invalid.
pub fn load_derived_case(name: &str) -> Result<DerivedTraceCase, ReferenceTraceError> {
    let (index, _) = load_all()?;
    index
        .derived_cases
        .iter()
        .find(|case| case.name == name)
        .cloned()
        .ok_or(ReferenceTraceError::Invalid("unknown derived trace"))
}

/// Applies a validated declarative derived case to its canonical source trace.
///
/// # Errors
/// Returns a bounded typed error if source identity or declared transforms do not resolve.
pub fn project_derived_trace(
    source: &ReferenceTrace,
    case: &DerivedTraceCase,
) -> Result<ReferenceTrace, ReferenceTraceError> {
    validate_projection(case, source.frames.len())?;
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(case.projection_declaration.source_frame_indices.len())
        .map_err(|_| ReferenceTraceError::Invalid("projection allocation"))?;
    for source_index in case.projection_declaration.source_frame_indices.as_slice() {
        let mut frame = source
            .frames
            .as_slice()
            .get(*source_index)
            .cloned()
            .ok_or(ReferenceTraceError::Invalid("projection source index"))?;
        apply_projection_frame(case, *source_index, &mut frame)?;
        frames.push(frame);
    }
    if case.projection_declaration.renumber_frame_indices {
        for (index, frame) in frames.iter_mut().enumerate() {
            frame.frame_index = index;
        }
    }
    let mut trace = source.clone();
    trace.name.clone_from(&case.name);
    trace.kind.clone_from(&case.kind);
    trace.frames = BoundedVec::new(frames)
        .map_err(|_| ReferenceTraceError::Invalid("projection frame bound"))?;
    trace.driver_proof = driver_proof(&trace.frames)?;
    assert_ordering(&trace)?;
    Ok(trace)
}

/// Loads and validates one named authoritative or derived trace.
///
/// # Errors
/// Returns a bounded typed error when the corpus, name, or trace is invalid.
pub fn load_trace(name: &str) -> Result<ReferenceTrace, ReferenceTraceError> {
    let (index, traces) = load_all()?;
    if let Some(trace) = traces.into_iter().find(|trace| trace.name == name) {
        return Ok(trace);
    }
    let loader = FixtureLoader::new();
    let record = index
        .derived_cases
        .iter()
        .find(|case| case.name == name)
        .ok_or(ReferenceTraceError::Invalid("unknown trace"))?;
    load_validated_trace(&loader, &record.path)
}

fn load_validated_trace(
    loader: &FixtureLoader,
    path: &str,
) -> Result<ReferenceTrace, ReferenceTraceError> {
    let bytes = loader.load_bytes(format!("reference-traces/{path}"))?;
    reject_duplicate_json_keys(&bytes)?;
    let trace =
        serde_json::from_slice(&bytes).map_err(|_| ReferenceTraceError::Invalid("trace JSON"))?;
    validate_trace(&trace)?;
    Ok(trace)
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
        .chain(index.derived_cases.iter().map(|entry| entry.path.clone()))
        .collect::<Vec<_>>();
    expected.sort();
    if tree != expected || tree.len() != CASE_COUNT + DERIVED_COUNT + 1 {
        return Err(ReferenceTraceError::Invalid("exact tree"));
    }
    validate_inventory(loader, index)?;
    validate_derived_cases(loader, index)?;
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

fn apply_projection_frame(
    case: &DerivedTraceCase,
    source_index: usize,
    frame: &mut TraceFrame,
) -> Result<(), ReferenceTraceError> {
    for projection in case
        .projection_declaration
        .subscriber_projections
        .as_slice()
    {
        if projection.source_frame_index == source_index {
            let mut wire = frame.wire.as_value().clone();
            wire["subscriber_ordinals"] = serde_json::to_value(&projection.subscriber_ordinals)
                .map_err(|_| ReferenceTraceError::Invalid("subscriber projection"))?;
            frame.wire = BoundedJsonValue::new(wire)
                .map_err(|_| ReferenceTraceError::Invalid("subscriber projection"))?;
        }
    }
    for relabel in case.projection_declaration.emission_relabels.as_slice() {
        let from = format!("emission-{}", relabel.from);
        let to = format!("emission-{}", relabel.to);
        if frame.broadcast_emission.as_deref() == Some(from.as_str()) {
            frame.broadcast_emission = Some(to.clone());
        }
        if frame.wire.as_value()["type"] == "broadcast_begin"
            && frame.wire.as_value()["emission"] == from
        {
            let mut wire = frame.wire.as_value().clone();
            wire["emission"] = Value::String(to);
            frame.wire = BoundedJsonValue::new(wire)
                .map_err(|_| ReferenceTraceError::Invalid("emission projection"))?;
        }
    }
    Ok(())
}

fn driver_proof(
    frames: &BoundedVec<TraceFrame, TRACE_FRAMES_MAX>,
) -> Result<TraceDriverProof, ReferenceTraceError> {
    let types = |direction| {
        frames
            .as_slice()
            .iter()
            .filter_map(|frame| {
                (frame.direction == direction)
                    .then(|| frame.wire.as_value()["type"].as_str().map(str::to_owned))
                    .flatten()
            })
            .collect()
    };
    Ok(TraceDriverProof {
        command_types: BoundedVec::new(types(FrameDirection::ClientToServer))
            .map_err(|_| ReferenceTraceError::Invalid("projection command proof"))?,
        message_types: BoundedVec::new(types(FrameDirection::ServerToClient))
            .map_err(|_| ReferenceTraceError::Invalid("projection message proof"))?,
    })
}

fn validate_projection(
    case: &DerivedTraceCase,
    source_len: usize,
) -> Result<(), ReferenceTraceError> {
    let declaration = &case.projection_declaration;
    if declaration.source_frame_indices.as_slice() != DERIVED_SOURCE_INDICES
        || !declaration.renumber_frame_indices
        || declaration.subscriber_projections.len() != 1
        || declaration.emission_relabels.len() != 1
    {
        return Err(ReferenceTraceError::Invalid(
            "derived projection declaration",
        ));
    }
    let subscriber = &declaration.subscriber_projections.as_slice()[0];
    let relabel = &declaration.emission_relabels.as_slice()[0];
    if subscriber.source_frame_index != 30
        || subscriber.subscriber_ordinals.as_slice() != [1]
        || relabel.from != 9
        || relabel.to != 5
        || declaration
            .source_frame_indices
            .as_slice()
            .iter()
            .any(|index| *index >= source_len)
    {
        return Err(ReferenceTraceError::Invalid("derived projection mapping"));
    }
    Ok(())
}

fn validate_derived_cases(
    loader: &FixtureLoader,
    index: &ReferenceTraceIndex,
) -> Result<(), ReferenceTraceError> {
    let mut previous = "";
    for case in &index.derived_cases {
        let source = index
            .cases
            .iter()
            .find(|source| source.path == case.derived_from);
        if case.path.as_str() <= previous
            || case.name != DERIVED_NAME
            || case.path != DERIVED_PATH
            || case.kind != DERIVED_KIND
            || case.derived_from != DERIVED_SOURCE
            || case.projection != DERIVED_PROJECTION
            || case.projection.is_empty()
            || source.is_none()
        {
            return Err(ReferenceTraceError::Invalid("derived metadata"));
        }
        let source_record = source.ok_or(ReferenceTraceError::Invalid("derived source"))?;
        let source_trace = load_validated_trace(loader, &source_record.path)?;
        validate_projection(case, source_trace.frames.len())?;
        let bytes = loader.load_bytes(format!("reference-traces/{}", case.path))?;
        if bytes.len() != case.bytes || lowercase_hex(&bytes) != case.sha256 {
            return Err(ReferenceTraceError::Invalid("derived integrity"));
        }
        let trace = load_validated_trace(loader, &case.path)?;
        let source = source_record;
        if trace.name != case.name
            || trace.kind != case.kind
            || trace.provenance.path != source.provenance.path
            || trace.provenance.capture_boundary != source.provenance.capture_boundary
            || trace.supporting_provenance != source.supporting_provenance
        {
            return Err(ReferenceTraceError::Invalid(
                "derived identity or authority",
            ));
        }
        assert_ordering(&trace)?;
        let projected = project_derived_trace(&source_trace, case)?;
        if projected.frames != trace.frames || projected.driver_proof != trace.driver_proof {
            return Err(ReferenceTraceError::Invalid("derived projection output"));
        }
        previous = &case.path;
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
        || (!TRACE_NAMES.contains(&trace.name.as_str()) && trace.name != DERIVED_NAME)
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

#[path = "../traces_ordering.rs"]
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
#[path = "tests/mod.rs"]
mod tests;
#[cfg(test)]
#[path = "../traces_authority_tests.rs"]
mod traces_authority_tests;
