//! Ordering assertions and semantic comparator implementation.

use super::{
    DateTime, FrameDirection, FrameSummary, GENERATED_UUID_PATHS, OrderingInvariant,
    ReferenceTrace, ReferenceTraceError, SemanticDivergence, TIMESTAMP_PATHS,
    TRACE_STRING_BYTES_MAX, TraceComparisonError, TraceFrame, TraceInvariantViolation, Uuid, Value,
    formatting,
};
use std::collections::{BTreeMap, BTreeSet};

#[path = "traces_authority.rs"]
mod traces_authority;
#[cfg(test)]
pub(super) use traces_authority::logical_payload_sha256;

/// Runs all six ordering assertions in fixed order.
///
/// # Errors
/// Returns the first typed invariant violation.
pub fn assert_ordering(trace: &ReferenceTrace) -> Result<(), TraceInvariantViolation> {
    traces_authority::authoritative(trace)?;
    for invariant in OrderingInvariant::ALL {
        assert_invariant(trace, invariant)?;
    }
    Ok(())
}

/// Runs one independently selectable ordering assertion.
///
/// # Errors
/// Returns a typed violation with the first offending pair.
pub fn assert_invariant(
    trace: &ReferenceTrace,
    invariant: OrderingInvariant,
) -> Result<(), TraceInvariantViolation> {
    match invariant {
        OrderingInvariant::IncreasingEventSeqPerConnection => increasing_sequences(trace),
        OrderingInvariant::InputAcceptedBeforeCausedEvents => accepted_before_caused(trace),
        OrderingInvariant::ToolStartBeforeMatchingToolEnd => tool_lifecycle(trace),
        OrderingInvariant::TurnFinishedExactlyOnceAfterFinalStreamDelta => terminal_once(trace),
        OrderingInvariant::NoServerEventFromStaleLeaseAfterReplacement => stale_lease(trace),
        OrderingInvariant::BroadcastDeliveryStableAscendingConnectionOrdinal => {
            broadcast_order(trace)
        }
    }
}

fn increasing_sequences(trace: &ReferenceTrace) -> Result<(), TraceInvariantViolation> {
    let invariant = OrderingInvariant::IncreasingEventSeqPerConnection;
    let mut previous: BTreeMap<u32, &TraceFrame> = BTreeMap::new();
    for frame in trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| is_server_broadcast(frame))
    {
        if let Some(first) = previous.insert(frame.connection_ordinal, frame)
            && wire_u64(frame, "event_seq") <= wire_u64(first, "event_seq")
        {
            return Err(violation(invariant, first, Some(frame)));
        }
    }
    Ok(())
}

fn accepted_before_caused(trace: &ReferenceTrace) -> Result<(), TraceInvariantViolation> {
    let invariant = OrderingInvariant::InputAcceptedBeforeCausedEvents;
    let accepted = trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| wire_type(frame) == "input_accepted")
        .filter_map(|frame| frame.caused_by.as_ref().map(|id| (id.as_str(), frame)))
        .collect::<BTreeMap<_, _>>();
    for frame in trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| is_server_broadcast(frame))
    {
        if let Some(id) = frame.caused_by.as_deref() {
            let Some(first) = accepted.get(id) else {
                return Err(violation(invariant, frame, None));
            };
            if first.frame_index >= frame.frame_index {
                return Err(violation(invariant, first, Some(frame)));
            }
        }
    }
    Ok(())
}

fn tool_lifecycle(trace: &ReferenceTrace) -> Result<(), TraceInvariantViolation> {
    let invariant = OrderingInvariant::ToolStartBeforeMatchingToolEnd;
    let mut open: Option<(&str, &TraceFrame)> = None;
    let mut closed = BTreeMap::new();
    for frame in logical_tool_events(trace) {
        let kind = nested_string(frame, &["delta", "message_type"]);
        let call = nested_string(frame, &["delta", "tool_call_id"]);
        if kind == Some("client_tool_start") {
            let Some(id) = call else {
                return Err(violation(invariant, frame, None));
            };
            if let Some((_, first)) = open {
                return Err(violation(invariant, first, Some(frame)));
            }
            open = Some((id, frame));
        } else if kind == Some("client_tool_end") {
            let Some(id) = call else {
                return Err(violation(invariant, frame, None));
            };
            let Some((open_id, first)) = open else {
                return Err(violation(invariant, frame, closed.get(id).copied()));
            };
            if id != open_id {
                return Err(violation(invariant, first, Some(frame)));
            }
            closed.insert(id, frame);
            open = None;
        }
    }
    if let Some((_, first)) = open {
        return Err(violation(invariant, first, None));
    }
    Ok(())
}

fn logical_tool_events(trace: &ReferenceTrace) -> Vec<&TraceFrame> {
    let mut labels = BTreeSet::new();
    trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| wire_type(frame) == "stream_delta")
        .filter(|frame| {
            frame
                .broadcast_emission
                .as_deref()
                .is_some_and(|label| labels.insert(label))
        })
        .filter(|frame| {
            matches!(
                nested_string(frame, &["delta", "message_type"]),
                Some("client_tool_start" | "client_tool_end")
            )
        })
        .collect()
}

fn terminal_once(trace: &ReferenceTrace) -> Result<(), TraceInvariantViolation> {
    let invariant = OrderingInvariant::TurnFinishedExactlyOnceAfterFinalStreamDelta;
    let starts = trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| matches!(wire_type(frame), "turn_started" | "turn_dequeued"));
    for start in starts {
        let Some(id) = start.caused_by.as_deref() else {
            return Err(violation(invariant, start, None));
        };
        let terminals = logical_groups(trace, id, "turn_finished");
        let deltas = logical_groups(trace, id, "stream_delta");
        if terminals.len() != 1 {
            return Err(violation(invariant, start, terminals.get(1).copied()));
        }
        if deltas
            .last()
            .is_some_and(|delta| delta.frame_index >= terminals[0].frame_index)
        {
            return Err(violation(
                invariant,
                deltas[deltas.len() - 1],
                Some(terminals[0]),
            ));
        }
    }
    Ok(())
}

fn logical_groups<'a>(
    trace: &'a ReferenceTrace,
    id: &str,
    frame_type: &str,
) -> Vec<&'a TraceFrame> {
    let mut labels = BTreeSet::new();
    trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| {
            frame.caused_by.as_deref() == Some(id)
                && wire_type(frame) == frame_type
                && frame
                    .broadcast_emission
                    .as_deref()
                    .is_some_and(|label| labels.insert(label))
        })
        .collect()
}

fn stale_lease(trace: &ReferenceTrace) -> Result<(), TraceInvariantViolation> {
    let invariant = OrderingInvariant::NoServerEventFromStaleLeaseAfterReplacement;
    let mut stale = BTreeMap::new();
    for frame in trace.frames.as_slice() {
        if wire_type(frame) == "lease_replaced" {
            if let Some(value) = nested_string(frame, &["old_lease_id"]) {
                stale.insert(value, frame);
            }
        } else if is_server_broadcast(frame)
            && let Some(first) = frame.lease_id.as_deref().and_then(|value| stale.get(value))
        {
            return Err(violation(invariant, first, Some(frame)));
        }
    }
    Ok(())
}

fn broadcast_order(trace: &ReferenceTrace) -> Result<(), TraceInvariantViolation> {
    let invariant = OrderingInvariant::BroadcastDeliveryStableAscendingConnectionOrdinal;
    let mut previous: BTreeMap<&str, &TraceFrame> = BTreeMap::new();
    let mut multi = false;
    for frame in trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| is_server_broadcast(frame))
    {
        let label = frame.broadcast_emission.as_deref().unwrap_or_default();
        if let Some(first) = previous.insert(label, frame) {
            multi = true;
            if first.connection_ordinal >= frame.connection_ordinal {
                return Err(violation(invariant, first, Some(frame)));
            }
        }
    }
    if trace.name == "vertical-slice" && !multi {
        return Err(violation(invariant, &trace.frames.as_slice()[0], None));
    }
    Ok(())
}

pub(super) fn violation(
    invariant: OrderingInvariant,
    first: &TraceFrame,
    second: Option<&TraceFrame>,
) -> TraceInvariantViolation {
    TraceInvariantViolation {
        invariant,
        first_frame: summary(first),
        second_frame: second.map(summary),
    }
}
pub(super) fn summary(frame: &TraceFrame) -> FrameSummary {
    FrameSummary {
        index: frame.frame_index,
        frame_type: wire_type(frame).to_owned(),
        connection_ordinal: frame.connection_ordinal,
    }
}

/// Compares two valid traces under the fixed semantic path rules.
///
/// # Errors
/// Returns ordering validation first, otherwise the first exact semantic divergence.
pub fn compare_semantic(
    expected: &ReferenceTrace,
    actual: &ReferenceTrace,
) -> Result<(), TraceComparisonError> {
    assert_ordering(expected).map_err(TraceComparisonError::Ordering)?;
    assert_ordering(actual).map_err(TraceComparisonError::Ordering)?;
    validate_comparison_trace(expected).map_err(TraceComparisonError::Semantic)?;
    validate_comparison_trace(actual).map_err(TraceComparisonError::Semantic)?;
    if expected.frames.len() != actual.frames.len() {
        return Err(divergence(
            expected.frames.len().min(actual.frames.len()),
            "/frames/length",
            expected.frames.len(),
            actual.frames.len(),
        ));
    }
    let mut state = CompareState {
        aliases: AliasState::default(),
        left_ranks: BTreeMap::new(),
        right_ranks: BTreeMap::new(),
    };
    for (index, (left, right)) in expected
        .frames
        .as_slice()
        .iter()
        .zip(actual.frames.as_slice().iter())
        .enumerate()
    {
        compare_frame(index, left, right, &mut state)?;
    }
    Ok(())
}

#[derive(Default)]
struct AliasState {
    forward: BTreeMap<Uuid, Uuid>,
    reverse: BTreeMap<Uuid, Uuid>,
}
struct CompareState {
    aliases: AliasState,
    left_ranks: BTreeMap<u32, usize>,
    right_ranks: BTreeMap<u32, usize>,
}

fn compare_frame(
    index: usize,
    left: &TraceFrame,
    right: &TraceFrame,
    state: &mut CompareState,
) -> Result<(), TraceComparisonError> {
    compare_exact(index, "/direction", &left.direction, &right.direction)?;
    compare_exact(
        index,
        "/connection_ordinal",
        &left.connection_ordinal,
        &right.connection_ordinal,
    )?;
    compare_exact(index, "/caused_by", &left.caused_by, &right.caused_by)?;
    compare_exact(index, "/lease_id", &left.lease_id, &right.lease_id)?;
    compare_exact(
        index,
        "/lease_current",
        &left.lease_current,
        &right.lease_current,
    )?;
    compare_exact(
        index,
        "/broadcast_emission",
        &left.broadcast_emission,
        &right.broadcast_emission,
    )?;
    let left_value = left.wire.as_value();
    let right_value = right.wire.as_value();
    compare_value(
        index,
        "/wire",
        left_value,
        right_value,
        state,
        left.connection_ordinal,
    )
}

fn compare_value(
    index: usize,
    path: &str,
    left: &Value,
    right: &Value,
    state: &mut CompareState,
    connection: u32,
) -> Result<(), TraceComparisonError> {
    if is_generated_path(path) {
        return compare_alias(index, path, left, right, &mut state.aliases);
    }
    if is_timestamp_path(path) {
        return compare_timestamp(index, path, left, right);
    }
    if path == "/wire/event_seq" {
        return compare_rank(
            index,
            path,
            left,
            right,
            connection,
            &mut state.left_ranks,
            &mut state.right_ranks,
        );
    }
    if matches!(path, "/wire/idempotency_key" | "/wire/payload_sha256") {
        return Ok(());
    }
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            compare_objects(index, path, a, b, state, connection)
        }
        (Value::Array(a), Value::Array(b)) => compare_arrays(index, path, a, b, state, connection),
        _ if left == right => Ok(()),
        _ => Err(TraceComparisonError::Semantic(divergence_value(
            index, path, left, right,
        ))),
    }
}

fn compare_objects(
    index: usize,
    path: &str,
    left: &serde_json::Map<String, Value>,
    right: &serde_json::Map<String, Value>,
    state: &mut CompareState,
    connection: u32,
) -> Result<(), TraceComparisonError> {
    let left_keys = left.keys().collect::<Vec<_>>();
    let right_keys = right.keys().collect::<Vec<_>>();
    if left_keys != right_keys {
        return Err(divergence(
            index,
            path,
            format!("keys:{left_keys:?}"),
            format!("keys:{right_keys:?}"),
        ));
    }
    for key in left.keys() {
        compare_value(
            index,
            &format!("{path}/{key}"),
            &left[key],
            &right[key],
            state,
            connection,
        )?;
    }
    Ok(())
}

fn compare_arrays(
    index: usize,
    path: &str,
    left: &[Value],
    right: &[Value],
    state: &mut CompareState,
    connection: u32,
) -> Result<(), TraceComparisonError> {
    if left.len() != right.len() {
        return Err(divergence(
            index,
            &format!("{path}/length"),
            left.len(),
            right.len(),
        ));
    }
    for (offset, (a, b)) in left.iter().zip(right).enumerate() {
        compare_value(index, &format!("{path}/{offset}"), a, b, state, connection)?;
    }
    Ok(())
}

fn compare_alias(
    index: usize,
    path: &str,
    left: &Value,
    right: &Value,
    aliases: &mut AliasState,
) -> Result<(), TraceComparisonError> {
    let a = parse_uuid(left.as_str()).map_err(|_| semantic(index, path, "valid UUID", left))?;
    let b = parse_uuid(right.as_str()).map_err(|_| semantic(index, path, "valid UUID", right))?;
    if aliases.forward.get(&a).is_some_and(|value| value != &b)
        || aliases.reverse.get(&b).is_some_and(|value| value != &a)
    {
        return Err(semantic(index, path, "consistent UUID alias", right));
    }
    aliases.forward.insert(a, b);
    aliases.reverse.insert(b, a);
    Ok(())
}
fn compare_timestamp(
    index: usize,
    path: &str,
    left: &Value,
    right: &Value,
) -> Result<(), TraceComparisonError> {
    parse_time(left.as_str().unwrap_or_default())
        .map_err(|_| semantic(index, path, "RFC3339", left))?;
    parse_time(right.as_str().unwrap_or_default())
        .map_err(|_| semantic(index, path, "RFC3339", right))?;
    Ok(())
}
fn compare_rank(
    index: usize,
    path: &str,
    left: &Value,
    right: &Value,
    connection: u32,
    left_ranks: &mut BTreeMap<u32, usize>,
    right_ranks: &mut BTreeMap<u32, usize>,
) -> Result<(), TraceComparisonError> {
    left.as_u64()
        .ok_or_else(|| semantic(index, path, "integer", left))?;
    right
        .as_u64()
        .ok_or_else(|| semantic(index, path, "integer", right))?;
    let left_rank = next_rank(left_ranks, connection);
    let right_rank = next_rank(right_ranks, connection);
    if left_rank == right_rank {
        Ok(())
    } else {
        Err(divergence(index, path, left_rank, right_rank))
    }
}
fn next_rank(ranks: &mut BTreeMap<u32, usize>, connection: u32) -> usize {
    let rank = ranks.entry(connection).or_default();
    let result = *rank;
    *rank = rank.saturating_add(1);
    result
}

fn validate_comparison_trace(trace: &ReferenceTrace) -> Result<(), SemanticDivergence> {
    validate_generated_values(trace)?;
    validate_timestamps(trace)?;
    validate_temporal_order(trace)?;
    validate_queue_times(trace)?;
    validate_delivery_keys(trace)
}

fn validate_generated_values(trace: &ReferenceTrace) -> Result<(), SemanticDivergence> {
    for frame in trace.frames.as_slice() {
        validate_generated_value(frame, "/wire", frame.wire.as_value())?;
    }
    Ok(())
}

fn validate_generated_value(
    frame: &TraceFrame,
    path: &str,
    value: &Value,
) -> Result<(), SemanticDivergence> {
    if is_generated_path(path) && parse_uuid(value.as_str()).is_err() {
        return Err(validation_error(
            frame.frame_index,
            path,
            "valid UUID",
            value,
        ));
    }
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                validate_generated_value(frame, &format!("{path}/{key}"), item)?;
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_generated_value(frame, &format!("{path}/{index}"), item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_timestamps(trace: &ReferenceTrace) -> Result<(), SemanticDivergence> {
    for frame in trace.frames.as_slice() {
        validate_timestamp_value(frame.frame_index, "/wire", frame.wire.as_value())?;
    }
    Ok(())
}

fn validate_timestamp_value(
    index: usize,
    path: &str,
    value: &Value,
) -> Result<(), SemanticDivergence> {
    if is_timestamp_path(path) && parse_time(value.as_str().unwrap_or_default()).is_err() {
        return Err(validation_error(index, path, "RFC3339", value));
    }
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                validate_timestamp_value(index, &format!("{path}/{key}"), item)?;
            }
        }
        Value::Array(items) => {
            for (offset, item) in items.iter().enumerate() {
                validate_timestamp_value(index, &format!("{path}/{offset}"), item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_temporal_order(trace: &ReferenceTrace) -> Result<(), SemanticDivergence> {
    let mut emitted = BTreeMap::new();
    let mut delta_dates = BTreeMap::new();
    let mut labels = BTreeSet::new();
    for frame in trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| is_server_broadcast(frame))
    {
        let time = wire_string(frame, "emitted_at").unwrap_or_default();
        if let Some(previous) = emitted.insert(frame.connection_ordinal, time.clone())
            && previous >= time
        {
            return Err(validation_error(
                frame.frame_index,
                "/wire/emitted_at",
                "strictly increasing",
                &json_value(time),
            ));
        }
        let label = frame.broadcast_emission.as_deref().unwrap_or_default();
        if labels.insert(label) && wire_type(frame) == "stream_delta" {
            validate_delta_date(frame, &mut delta_dates)?;
        }
    }
    Ok(())
}

fn validate_delta_date(
    frame: &TraceFrame,
    dates: &mut BTreeMap<(u32, String), String>,
) -> Result<(), SemanticDivergence> {
    let Some(date) = nested_string(frame, &["delta", "date"]) else {
        return Ok(());
    };
    let key = (
        frame.connection_ordinal,
        frame.caused_by.clone().unwrap_or_default(),
    );
    if let Some(previous) = dates.insert(key, date.to_owned())
        && previous.as_str() > date
    {
        return Err(validation_error(
            frame.frame_index,
            "/wire/delta/date",
            "nondecreasing",
            &json_value(date),
        ));
    }
    Ok(())
}

fn validate_queue_times(trace: &ReferenceTrace) -> Result<(), SemanticDivergence> {
    let mut times = BTreeMap::new();
    for frame in trace.frames.as_slice() {
        let Some(queue) = frame.wire.as_value().get("queue").and_then(Value::as_array) else {
            continue;
        };
        for (offset, item) in queue.iter().enumerate() {
            let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
            let time = item
                .get("enqueued_at")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(previous) = times.insert(id.to_owned(), time.to_owned())
                && previous != time
            {
                return Err(validation_error(
                    frame.frame_index,
                    &format!("/wire/queue/{offset}/enqueued_at"),
                    "stable queue time",
                    &json_value(time),
                ));
            }
        }
    }
    Ok(())
}

fn validate_delivery_keys(trace: &ReferenceTrace) -> Result<(), SemanticDivergence> {
    let mut unique = BTreeSet::new();
    for frame in trace
        .frames
        .as_slice()
        .iter()
        .filter(|frame| is_server_broadcast(frame))
    {
        let key = wire_string(frame, "idempotency_key").unwrap_or_default();
        let expected = format!("{}:{}:", wire_type(frame), wire_u64(frame, "event_seq"));
        let uuid = key
            .strip_prefix(&expected)
            .and_then(|value| Uuid::parse_str(value).ok());
        if uuid.is_none_or(|value| value.is_nil()) || !unique.insert(key.clone()) {
            return Err(validation_error(
                frame.frame_index,
                "/wire/idempotency_key",
                "unique type:event_seq:UUID",
                &json_value(key),
            ));
        }
    }
    Ok(())
}

fn compare_exact<T: formatting::Debug + PartialEq>(
    index: usize,
    path: &str,
    left: &T,
    right: &T,
) -> Result<(), TraceComparisonError> {
    if left == right {
        Ok(())
    } else {
        Err(divergence(
            index,
            path,
            format!("{left:?}"),
            format!("{right:?}"),
        ))
    }
}
fn divergence(
    index: usize,
    path: &str,
    expected: impl formatting::Debug,
    actual: impl formatting::Debug,
) -> TraceComparisonError {
    TraceComparisonError::Semantic(SemanticDivergence {
        frame_index: index,
        path: path.to_owned(),
        expected: bounded_summary(format!("{expected:?}")),
        actual: bounded_summary(format!("{actual:?}")),
    })
}
fn json_value(value: impl Into<String>) -> Value {
    Value::String(value.into())
}
fn validation_error(
    index: usize,
    path: &str,
    expected: &str,
    actual: &Value,
) -> SemanticDivergence {
    SemanticDivergence {
        frame_index: index,
        path: path.to_owned(),
        expected: expected.to_owned(),
        actual: bounded_summary(actual.to_string()),
    }
}
fn divergence_value(
    index: usize,
    path: &str,
    expected: &Value,
    actual: &Value,
) -> SemanticDivergence {
    SemanticDivergence {
        frame_index: index,
        path: path.to_owned(),
        expected: bounded_summary(expected.to_string()),
        actual: bounded_summary(actual.to_string()),
    }
}
fn semantic(index: usize, path: &str, expected: &str, actual: &Value) -> TraceComparisonError {
    TraceComparisonError::Semantic(SemanticDivergence {
        frame_index: index,
        path: path.to_owned(),
        expected: expected.to_owned(),
        actual: bounded_summary(actual.to_string()),
    })
}
fn bounded_summary(mut value: String) -> String {
    value.truncate(TRACE_STRING_BYTES_MAX);
    value
}

pub(super) fn is_generated_path(path: &str) -> bool {
    GENERATED_UUID_PATHS
        .iter()
        .any(|rule| path_match(rule, path))
}
pub(super) fn is_timestamp_path(path: &str) -> bool {
    TIMESTAMP_PATHS.iter().any(|rule| path_match(rule, path))
}
fn path_match(rule: &str, path: &str) -> bool {
    let a = rule.split('/').collect::<Vec<_>>();
    let b = path.split('/').collect::<Vec<_>>();
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| *x == "*" || *x == y)
}
pub(super) fn parse_uuid(value: Option<&str>) -> Result<Uuid, ReferenceTraceError> {
    Uuid::parse_str(value.unwrap_or_default()).map_err(|_| ReferenceTraceError::Invalid("UUID"))
}
pub(super) fn parse_time(value: &str) -> Result<(), ReferenceTraceError> {
    DateTime::parse_from_rfc3339(value)
        .map(|_| ())
        .map_err(|_| ReferenceTraceError::Invalid("RFC3339"))
}
pub(super) fn safe_string(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= TRACE_STRING_BYTES_MAX
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
}
pub(super) fn wire_type(frame: &TraceFrame) -> &str {
    frame
        .wire
        .as_value()
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
}
pub(super) fn wire_string(frame: &TraceFrame, key: &str) -> Option<String> {
    frame
        .wire
        .as_value()
        .get(key)?
        .as_str()
        .map(ToOwned::to_owned)
}
pub(super) fn wire_u64(frame: &TraceFrame, key: &str) -> u64 {
    frame
        .wire
        .as_value()
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or_default()
}
pub(super) fn nested_string<'a>(frame: &'a TraceFrame, path: &[&str]) -> Option<&'a str> {
    let mut value = frame.wire.as_value();
    for key in path {
        value = value.get(key)?;
    }
    value.as_str()
}
pub(super) fn is_broadcast(frame_type: &str) -> bool {
    matches!(
        frame_type,
        "control_request"
            | "update_device_status"
            | "update_loop_status"
            | "update_queue"
            | "stream_delta"
            | "turn_finished"
            | "update_subagent_state"
    )
}
pub(super) fn is_server_broadcast(frame: &TraceFrame) -> bool {
    frame.direction == FrameDirection::ServerToClient && is_broadcast(wire_type(frame))
}
