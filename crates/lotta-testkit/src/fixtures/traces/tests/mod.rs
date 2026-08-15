use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
fn trace(name: &str) -> ReferenceTrace {
    load_trace(name).unwrap_or_else(|error| panic!("trace {name}: {error}"))
}
fn set_wire(frame: &mut TraceFrame, path: &[&str], value: Value) {
    let mut wire = frame.wire.as_value().clone();
    let mut current = &mut wire;
    for key in &path[..path.len() - 1] {
        current = if let Ok(index) = key.parse::<usize>() {
            &mut current[index]
        } else {
            &mut current[*key]
        };
    }
    let final_key = path[path.len() - 1];
    if let Ok(index) = final_key.parse::<usize>() {
        current[index] = value;
    } else {
        current[final_key] = value;
    }
    frame.wire = BoundedJsonValue::new(wire).unwrap_or_else(|error| panic!("wire: {error}"));
}
fn edit_frames(trace: &mut ReferenceTrace, edit: impl FnOnce(&mut Vec<TraceFrame>)) {
    let mut frames = trace.frames.as_slice().to_vec();
    edit(&mut frames);
    trace.frames = BoundedVec::new(frames).unwrap_or_else(|error| panic!("frames: {error}"));
}
fn renumber(trace: &mut ReferenceTrace) {
    edit_frames(trace, |frames| {
        for (index, frame) in frames.iter_mut().enumerate() {
            frame.frame_index = index;
        }
    });
}
fn refresh_broadcasts(trace: &mut ReferenceTrace, emissions: Option<&[String]>) {
    edit_frames(trace, |frames| {
        let mut index = 0;
        while index < frames.len() {
            if wire_type(&frames[index]) != "broadcast_begin" {
                index += 1;
                continue;
            }
            let emission = nested_string(&frames[index], &["emission"])
                .unwrap_or_else(|| panic!("broadcast emission"))
                .to_owned();
            let refresh = emissions.is_none_or(|values| values.contains(&emission));
            let delivery = frames
                .get(index + 1)
                .filter(|frame| frame.broadcast_emission.as_deref() == Some(&emission));
            if refresh {
                let hash = delivery
                    .and_then(|frame| logical_payload_sha256(frame.wire.as_value()))
                    .unwrap_or_else(|| panic!("logical payload hash"));
                set_wire(&mut frames[index], &["payload_sha256"], json!(hash));
            }
            index += 1;
        }
    });
}
fn mutate_broadcast_group(trace: &mut ReferenceTrace, source: usize, path: &[&str], value: &Value) {
    let emission = trace.frames.as_slice()[source]
        .broadcast_emission
        .clone()
        .unwrap_or_else(|| panic!("authoritative delivery"));
    edit_frames(trace, |frames| {
        for frame in frames
            .iter_mut()
            .filter(|frame| frame.broadcast_emission.as_deref() == Some(&emission))
        {
            set_wire(frame, path, value.clone());
        }
    });
    refresh_broadcasts(trace, Some(&[emission]));
}
fn violation(trace: &ReferenceTrace, invariant: OrderingInvariant) -> TraceInvariantViolation {
    assert_invariant(trace, invariant).unwrap_err()
}
#[test]
fn duplicate_json_keys_are_rejected_before_serde() {
    for bytes in [
        br#"{"a":1,"a":2}"#.as_slice(),
        br#"{"a":1,"\u0061":2}"#.as_slice(),
        br#"{"outer":{"a":1,"a":2}}"#.as_slice(),
        br#"{"a":1"#.as_slice(),
    ] {
        assert!(reject_duplicate_json_keys(bytes).is_err());
    }
    assert!(reject_duplicate_json_keys(br#"{"a":1,"nested":{"a":2}}"#).is_ok());
}
#[test]
fn sanitizer_rejects_payloads_secrets_duplicates_and_non_utf8() {
    let mutations: &[&[u8]] = &[
        br#"{"content":"real text","marker":"<sanitized-trace>"}"#,
        br#"{"tool_args":"rm -rf /","marker":"<sanitized-trace>"}"#,
        br#"{"tool_output":"result","marker":"<sanitized-trace>"}"#,
        br#"{"prompt":"body","marker":"<sanitized-trace>"}"#,
        br#"{"x":"Bearer token","marker":"<sanitized-trace>"}"#,
        br#"{"x":"eyJabc.def.ghi","marker":"<sanitized-trace>"}"#,
        br#"{"x":"-----BEGIN PRIVATE KEY-----","marker":"<sanitized-trace>"}"#,
        br#"{"OAuth":"x","marker":"<sanitized-trace>"}"#,
        br#"{"aws_access_key":"x","marker":"<sanitized-trace>"}"#,
        br#"{"content":"<sanitized-trace>","content":"bad"}"#,
        br#"{"safe":{"x":1,"x":2},"marker":"<sanitized-trace>"}"#,
    ];
    for mutation in mutations {
        assert!(scan_sanitized_json(mutation, true).is_err());
    }
    assert!(scan_sanitized_json(&[0xff, 0xfe], true).is_err());
}
#[test]
fn sanitizer_accepts_structural_values_and_exact_payload_placeholder() {
    let safe = concat!(
        r#"{"type":"input","request_id":"request-1","status":"success","#,
        r#""content":"<sanitized-trace>","tool_args":"<sanitized-trace>","#,
        r#""delta":{"message_type":"message","delta":"<sanitized-trace>"}}"#,
    );
    assert!(scan_sanitized_json(safe.as_bytes(), true).is_ok());
}
#[test]
fn case_and_invariant_provenance_is_exact_and_keyed() {
    let (index, traces) = load_all().expect("corpus");
    for (case, trace) in index.cases.iter().zip(traces.iter()) {
        assert_eq!(case.supporting_provenance, trace.supporting_provenance);
        assert!(!case.supporting_provenance.is_empty());
    }
    for (actual, expected) in index.invariant_provenance.iter().zip(INVARIANT_PROVENANCE) {
        assert_eq!(
            (
                actual.invariant,
                actual.path.as_str(),
                actual.symbol.as_str()
            ),
            expected
        );
    }
}
#[test]
fn index_is_complete() {
    let (index, traces) = load_all().expect("complete corpus");
    assert_eq!(index.reliability_surfaces, ReliabilitySurface::ALL);
    assert_eq!(index.ordering_invariants, OrderingInvariant::ALL);
    assert_eq!(index.inventory.len(), INVENTORY_COUNT);
    assert_eq!(traces.len(), CASE_COUNT);
}
macro_rules! surface_test {
    ($name:ident, $trace:literal, $check:expr) => {
        #[test]
        fn $name() {
            let trace = trace($trace);
            assert!($check(&trace));
        }
    };
}
fn has(trace: &ReferenceTrace, kind: &str) -> bool {
    trace
        .frames
        .as_slice()
        .iter()
        .any(|frame| wire_type(frame) == kind)
}
surface_test!(
    queue_surface_is_semantic,
    "queue",
    |trace: &ReferenceTrace| has(trace, "update_queue")
        && trace
            .frames
            .as_slice()
            .iter()
            .filter(|x| wire_type(x) == "turn_finished")
            .count()
            == 2
);
surface_test!(
    abort_surface_is_semantic,
    "abort",
    |trace: &ReferenceTrace| has(trace, "abort_message") && has(trace, "abort_message_response")
);
surface_test!(
    disconnect_surface_is_semantic,
    "disconnect",
    |trace: &ReferenceTrace| has(trace, "connection_cleanup") && has(trace, "sync_response")
);
surface_test!(
    stale_lease_surface_is_semantic,
    "stale-lease",
    |trace: &ReferenceTrace| has(trace, "lease_replaced")
        && !trace
            .frames
            .as_slice()
            .iter()
            .any(|x| x.lease_current == Some(false))
);
surface_test!(
    retry_surface_is_semantic,
    "retry",
    |trace: &ReferenceTrace| trace
        .frames
        .as_slice()
        .iter()
        .any(|x| nested_string(x, &["delta", "message_type"]) == Some("retry"))
);
surface_test!(
    idempotency_surface_is_semantic,
    "idempotency",
    |trace: &ReferenceTrace| trace
        .frames
        .as_slice()
        .iter()
        .filter(|x| wire_type(x) == "input_accepted")
        .count()
        == 2
        && trace
            .frames
            .as_slice()
            .iter()
            .filter(|x| wire_type(x) == "turn_finished")
            .count()
            == 1
);
surface_test!(
    crash_recovery_surface_is_semantic,
    "crash-recovery",
    |trace: &ReferenceTrace| has(trace, "process_crash")
        && has(trace, "restart")
        && has(trace, "sync_response")
);
#[test]
fn ordering_invariants_increasing_event_seq() {
    assert_invariant(&trace("vertical-slice"), OrderingInvariant::ALL[0]).expect("sequence");
}
#[test]
fn ordering_invariants_input_accepted_before_caused_events() {
    assert_invariant(&trace("vertical-slice"), OrderingInvariant::ALL[1]).expect("admission");
}
#[test]
fn ordering_invariants_tool_start_before_matching_tool_end() {
    assert_invariant(&trace("vertical-slice"), OrderingInvariant::ALL[2]).expect("tools");
}
#[test]
fn ordering_invariants_turn_finished_once_after_final_delta() {
    assert_invariant(&trace("vertical-slice"), OrderingInvariant::ALL[3]).expect("terminal");
}
#[test]
fn ordering_invariants_no_stale_lease_event() {
    assert_invariant(&trace("stale-lease"), OrderingInvariant::ALL[4]).expect("lease");
}
#[test]
fn ordering_invariants_stable_broadcast_ordinals() {
    assert_invariant(&trace("vertical-slice"), OrderingInvariant::ALL[5]).expect("broadcast");
}
#[test]
fn ordering_invariants_aggregate() {
    assert_ordering(&trace("vertical-slice")).expect("all invariants");
}
#[test]
fn ordering_mutation_event_seq_reports_exact_pair() {
    let mut value = trace("vertical-slice");
    let indexes = value
        .frames
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, x)| is_server_broadcast(x) && x.connection_ordinal == 1)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let previous = wire_u64(&value.frames.as_slice()[indexes[0]], "event_seq");
    edit_frames(&mut value, |frames| {
        set_wire(&mut frames[indexes[1]], &["event_seq"], json!(previous));
    });
    let error = violation(&value, OrderingInvariant::ALL[0]);
    assert_eq!(
        (error.first_frame.index, error.second_frame.unwrap().index),
        (indexes[0], indexes[1])
    );
}
#[test]
fn ordering_mutation_admission_reports_exact_pair() {
    let mut value = trace("retry");
    let accepted = value
        .frames
        .as_slice()
        .iter()
        .position(|x| wire_type(x) == "input_accepted")
        .unwrap();
    let caused = value
        .frames
        .as_slice()
        .iter()
        .position(|x| wire_type(x) == "stream_delta")
        .unwrap();
    edit_frames(&mut value, |frames| frames.swap(accepted, caused));
    renumber(&mut value);
    let error = violation(&value, OrderingInvariant::ALL[1]);
    assert_eq!(error.invariant, OrderingInvariant::ALL[1]);
}
fn tool_indexes(value: &ReferenceTrace) -> (usize, usize) {
    let find = |kind| {
        value
            .frames
            .as_slice()
            .iter()
            .position(|frame| nested_string(frame, &["delta", "message_type"]) == Some(kind))
            .unwrap()
    };
    (find("client_tool_start"), find("client_tool_end"))
}
fn tool_error(value: &ReferenceTrace) -> TraceInvariantViolation {
    violation(value, OrderingInvariant::ALL[2])
}
#[test]
fn tool_lifecycle_vertical_multisubscriber_is_logical_once() {
    assert_invariant(&trace("vertical-slice"), OrderingInvariant::ALL[2]).expect("logical pair");
}
#[test]
fn tool_lifecycle_end_without_start_reports_end_only() {
    let mut value = trace("vertical-slice");
    let (start, end) = tool_indexes(&value);
    edit_frames(&mut value, |frames| {
        set_wire(
            &mut frames[start],
            &["delta", "message_type"],
            json!("message"),
        );
    });
    let error = tool_error(&value);
    assert_eq!(error.first_frame.index, end);
    assert!(error.second_frame.is_none());
}
#[test]
fn tool_lifecycle_start_without_end_reports_start_only() {
    let mut value = trace("vertical-slice");
    let (start, end) = tool_indexes(&value);
    edit_frames(&mut value, |frames| {
        set_wire(
            &mut frames[end],
            &["delta", "message_type"],
            json!("message"),
        );
    });
    let error = tool_error(&value);
    assert_eq!(error.first_frame.index, start);
    assert!(error.second_frame.is_none());
}
#[test]
fn tool_lifecycle_duplicate_start_reports_exact_pair() {
    let mut value = trace("vertical-slice");
    let (start, end) = tool_indexes(&value);
    edit_frames(&mut value, |frames| {
        set_wire(
            &mut frames[end],
            &["delta", "message_type"],
            json!("client_tool_start"),
        );
    });
    let error = tool_error(&value);
    assert_eq!(
        (error.first_frame.index, error.second_frame.unwrap().index),
        (start, end)
    );
}
#[test]
fn tool_lifecycle_duplicate_end_reports_exact_pair() {
    let mut value = trace("vertical-slice");
    let (_, end) = tool_indexes(&value);
    edit_frames(&mut value, |frames| {
        let mut duplicate = frames[end].clone();
        duplicate.broadcast_emission = Some("duplicate-end".into());
        frames.push(duplicate);
    });
    renumber(&mut value);
    let error = tool_error(&value);
    assert_eq!(
        (error.first_frame.index, error.second_frame.unwrap().index),
        (value.frames.len() - 1, end)
    );
}
#[test]
fn tool_lifecycle_wrong_end_id_reports_start_end_pair() {
    let mut value = trace("vertical-slice");
    let (start, end) = tool_indexes(&value);
    edit_frames(&mut value, |frames| {
        set_wire(
            &mut frames[end],
            &["delta", "tool_call_id"],
            json!(uuid_for(777)),
        );
    });
    let error = tool_error(&value);
    assert_eq!(
        (error.first_frame.index, error.second_frame.unwrap().index),
        (start, end)
    );
}
#[test]
fn tool_lifecycle_reordered_end_reports_end_only() {
    let mut value = trace("vertical-slice");
    let (start, end) = tool_indexes(&value);
    edit_frames(&mut value, |frames| frames.swap(start, end));
    renumber(&mut value);
    let error = tool_error(&value);
    assert_eq!(error.first_frame.index, start);
    assert!(error.second_frame.is_none());
}
#[test]
fn ordering_mutation_terminal_reports_exact_pair() {
    let mut value = trace("retry");
    let terminal = value
        .frames
        .as_slice()
        .iter()
        .position(|x| wire_type(x) == "turn_finished")
        .unwrap();
    edit_frames(&mut value, |frames| {
        let mut duplicate = frames[terminal].clone();
        duplicate.broadcast_emission = Some("duplicate-terminal".into());
        frames.push(duplicate);
    });
    renumber(&mut value);
    let error = violation(&value, OrderingInvariant::ALL[3]);
    assert!(error.second_frame.is_some());
}
#[test]
fn ordering_mutation_stale_lease_reports_exact_pair() {
    let mut value = trace("stale-lease");
    let replaced = value
        .frames
        .as_slice()
        .iter()
        .position(|x| wire_type(x) == "lease_replaced")
        .unwrap();
    let later = value
        .frames
        .as_slice()
        .iter()
        .enumerate()
        .skip(replaced + 1)
        .find(|(_, frame)| is_server_broadcast(frame))
        .map(|(index, _)| index)
        .unwrap();
    edit_frames(&mut value, |frames| {
        frames[later].lease_id = Some("lease-old".into());
    });
    let error = violation(&value, OrderingInvariant::ALL[4]);
    assert_eq!(
        (error.first_frame.index, error.second_frame.unwrap().index),
        (replaced, later)
    );
}
#[test]
fn ordering_mutation_broadcast_reports_exact_pair() {
    let mut value = trace("vertical-slice");
    let indexes = value
        .frames
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, x)| x.broadcast_emission.as_deref() == Some("emission-6"))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    edit_frames(&mut value, |frames| {
        frames[indexes[1]].connection_ordinal = 1;
    });
    let error = violation(&value, OrderingInvariant::ALL[5]);
    assert_eq!(
        (error.first_frame.index, error.second_frame.unwrap().index),
        (indexes[0], indexes[1])
    );
}
mod semantic;
use semantic::uuid_for;

#[test]
fn slice_trace_shape() {
    let trace = trace("vertical-slice");
    let types = trace
        .frames
        .as_slice()
        .iter()
        .map(wire_type)
        .collect::<Vec<_>>();
    for required in [
        "runtime_start",
        "runtime_start_response",
        "update_device_status",
        "update_loop_status",
        "update_queue",
        "input",
        "input_accepted",
        "stream_delta",
        "turn_finished",
    ] {
        assert!(types.contains(&required), "missing {required}");
    }
    let position = |kind| types.iter().position(|value| *value == kind).unwrap();
    assert!(position("runtime_start") < position("runtime_start_response"));
    assert!(position("input") < position("input_accepted"));
    assert!(position("input_accepted") < position("stream_delta"));
    assert!(position("stream_delta") < position("turn_finished"));
}
#[test]
fn per_trace_driver_proof_is_exact() {
    let (index, traces) = load_all().expect("corpus");
    for (case, trace) in index.cases.iter().zip(traces.iter()) {
        assert_eq!(case.driver_proof, trace.driver_proof);
        let commands = trace
            .frames
            .as_slice()
            .iter()
            .filter(|x| x.direction == FrameDirection::ClientToServer)
            .map(wire_type)
            .collect::<Vec<_>>();
        let messages = trace
            .frames
            .as_slice()
            .iter()
            .filter(|x| x.direction == FrameDirection::ServerToClient)
            .map(wire_type)
            .collect::<Vec<_>>();
        assert_eq!(commands, trace.driver_proof.command_types.as_slice());
        assert_eq!(messages, trace.driver_proof.message_types.as_slice());
        assert!(!commands.is_empty() && !messages.is_empty());
    }
}
#[test]
fn source_provenance_is_pinned_and_honest() {
    let (index, traces) = load_all().expect("corpus");
    assert_eq!(index.source_commit, SOURCE_COMMIT);
    assert_eq!(index.source_files.len(), SOURCE_FILES_COUNT);
    assert_eq!(index.source_regions.len(), SOURCE_REGIONS_COUNT);
    assert!(
        traces
            .iter()
            .all(|x| x.provenance.capture_boundary == BOUNDARY)
    );
}
#[test]
fn tree_sha_and_all_traces_validate() {
    load_all().expect("integrity");
}

struct TraceCorpus(PathBuf);
impl TraceCorpus {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let ordinal = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("lotta-traces-{}-{ordinal}", std::process::id()));
        let destination = root.join("reference-traces");
        fs::create_dir_all(&destination).expect("create trace corpus");
        let loader = FixtureLoader::new();
        for relative in loader.list_tree("reference-traces").expect("source tree") {
            let bytes = loader
                .load_bytes(format!("reference-traces/{relative}"))
                .expect("source bytes");
            fs::write(destination.join(relative), bytes).expect("copy trace corpus");
        }
        Self(root)
    }
    fn loader(&self) -> FixtureLoader {
        FixtureLoader::from_root(&self.0).expect("custom trace loader")
    }
    fn index_path(&self) -> PathBuf {
        self.0.join("reference-traces/index.json")
    }
}
impl Drop for TraceCorpus {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove trace corpus");
    }
}

fn mutate_derived_index(edit: impl FnOnce(&mut Value)) -> TraceCorpus {
    let corpus = TraceCorpus::new();
    let path = corpus.index_path();
    let mut index: Value =
        serde_json::from_slice(&fs::read(&path).expect("read index")).expect("parse index");
    edit(&mut index["derived_cases"][0]);
    fs::write(
        path,
        serde_json::to_vec_pretty(&index).expect("serialize index"),
    )
    .expect("write index");
    corpus
}

#[test]
fn derived_metadata_tamper_bytes_hash_and_path_are_rejected() {
    for corpus in [
        mutate_derived_index(|case| case["bytes"] = json!(1)),
        mutate_derived_index(|case| case["sha256"] = json!("00")),
        mutate_derived_index(|case| case["path"] = json!("vertical-slice.json")),
    ] {
        let loader = corpus.loader();
        let index: ReferenceTraceIndex = loader
            .load("reference-traces/index.json")
            .expect("mutated index parses");
        assert!(validate_index(&loader, &index).is_err());
    }
}

#[test]
fn derived_projection_declaration_tamper_is_rejected() {
    let mutations: [fn(&mut Value); 7] = [
        |case| case["projection_declaration"]["source_frame_indices"][0] = json!(1),
        |case| case["projection_declaration"]["source_frame_indices"][1] = json!(0),
        |case| case["projection_declaration"]["renumber_frame_indices"] = json!(false),
        |case| {
            case["projection_declaration"]["subscriber_projections"][0]["source_frame_index"] =
                json!(29);
        },
        |case| {
            case["projection_declaration"]["subscriber_projections"][0]["subscriber_ordinals"] =
                json!([2]);
        },
        |case| case["projection_declaration"]["emission_relabels"][0]["from"] = json!(8),
        |case| case["projection_declaration"]["emission_relabels"][0]["to"] = json!(6),
    ];
    for mutation in mutations {
        let corpus = mutate_derived_index(mutation);
        let loader = corpus.loader();
        let index: ReferenceTraceIndex = loader
            .load("reference-traces/index.json")
            .expect("mutated index parses");
        assert!(validate_index(&loader, &index).is_err());
    }
}

#[test]
fn derived_trace_loads_authoritatively_by_public_name() {
    let trace = load_trace(DERIVED_NAME).expect("derived trace");
    assert_eq!(trace.name, DERIVED_NAME);
    assert_eq!(trace.kind, DERIVED_KIND);
    assert_ordering(&trace).expect("derived ordering authority");
}
#[test]
fn sanitization_covers_all_ten_files() {
    let loader = FixtureLoader::new();
    let tree = loader.list_tree("reference-traces").expect("tree");
    assert_eq!(tree.len(), 10);
    validate_sanitization(&loader, &tree).expect("sanitized");
}
#[test]
fn metadata_bounds_and_frame_indexes_are_exact() {
    let (_, traces) = load_all().expect("corpus");
    for trace in traces {
        assert!(trace.frames.len() <= TRACE_FRAMES_MAX);
        for (index, frame) in trace.frames.as_slice().iter().enumerate() {
            assert_eq!(frame.frame_index, index);
        }
    }
}
