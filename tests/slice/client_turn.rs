use crate::harness;
use lotta_runtime::ports::{ProviderContentPart, ProviderMessageRole, TranscriptItem};
use lotta_testkit::fixtures::traces::{OrderingInvariant, assert_invariant, compare_semantic};

#[tokio::test]
async fn completes() {
    let capture = harness::capture().await;
    let kinds = kinds(&capture.trace);
    assert_eq!(
        kinds,
        [
            "open",
            "runtime_start",
            "runtime_start_response",
            "broadcast_begin",
            "update_device_status",
            "broadcast_begin",
            "update_loop_status",
            "broadcast_begin",
            "update_queue",
            "input",
            "input_accepted",
            "turn_started",
            "lease_acquired",
            "broadcast_begin",
            "stream_delta",
            "broadcast_begin",
            "turn_finished",
        ]
    );
    assert_eq!(capture.stats.runtime_resolutions, 1);
    assert_eq!(capture.stats.retained_inputs, 1);
    assert_eq!(capture.stats.provider_calls, 1);
    assert_eq!(capture.stats.projections, 1);
    assert_eq!(capture.stats.terminals, 1);
    assert_eq!(capture.tool_calls, 0);
    assert!(capture.checkout.join("src/app-server-client.ts").is_file());
    assert_eq!(capture.transcript.len(), 3);
    assert_eq!(
        capture.transcript.as_slice(),
        &[
            TranscriptItem::Manifest(capture.expected_manifest),
            TranscriptItem::Entry(capture.expected_session),
            TranscriptItem::Entry(capture.expected_appended.clone()),
        ]
    );
    let TranscriptItem::Entry(lotta_domain::TranscriptEntry::Message(entry)) =
        &capture.transcript.as_slice()[2]
    else {
        panic!("third transcript item must be the admitted message");
    };
    assert_eq!(entry.id.as_str(), "client-message-slice");
    assert_eq!(entry.message.id.as_str(), "client-message-slice");
    assert_eq!(
        entry.message.content.as_ref().unwrap().as_value(),
        "<sanitized-trace>"
    );
    assert_eq!(capture.provider_requests.len(), 1);
    let request = &capture.provider_requests.as_slice()[0];
    assert_eq!(request.messages.len(), 1);
    assert_eq!(
        request.messages.as_slice()[0].role,
        ProviderMessageRole::User
    );
    assert_eq!(
        request.messages.as_slice()[0].content.as_slice(),
        &[ProviderContentPart::Text(
            lotta_runtime::boundary::ProviderText::new("<sanitized-trace>".into()).unwrap()
        )]
    );
    assert_eq!(count(&kinds, "turn_finished"), 1);
    let terminal = capture
        .trace
        .frames
        .as_slice()
        .iter()
        .find(|frame| frame.wire.as_value()["type"] == "turn_finished")
        .unwrap()
        .wire
        .as_value();
    assert_eq!(terminal["turn_id"], "00000000-0000-4000-8000-000000000090");
    assert_eq!(terminal["run_id"], "00000000-0000-4000-8000-000000000091");
    assert_eq!(terminal["stop_reason"], "end_turn");
    persist(&capture.trace);
    harness::assert_failure_cleanup_and_timeout().await;
}

#[tokio::test]
async fn matches_reference_trace() {
    let source: lotta_testkit::fixtures::traces::ReferenceTrace = serde_json::from_slice(
        include_bytes!("../../fixtures/reference-traces/vertical-slice.json"),
    )
    .unwrap();
    let expected = lotta_testkit::fixtures::traces::load_trace("slice_happy_turn")
        .expect("authoritative derived trace");
    let declaration = lotta_testkit::fixtures::traces::load_derived_case("slice_happy_turn")
        .expect("validated derived declaration");
    let projected = lotta_testkit::fixtures::traces::project_derived_trace(&source, &declaration)
        .expect("declarative source projection");
    assert_eq!(expected.frames, projected.frames);
    let actual = harness::capture().await.trace;
    compare_semantic(&expected, &actual).unwrap_or_else(|error| panic!("{error}"));
    let mut value = serde_json::to_value(&expected).unwrap();
    value["frames"][1]["wire"]["request_id"] = serde_json::json!("changed");
    let mutated = serde_json::from_value(value).unwrap();
    let error = compare_semantic(&mutated, &actual).expect_err("mutation must diverge");
    let message = error.to_string();
    assert!(message.contains("frame 1"), "{message}");
    assert!(message.contains("/wire/request_id"), "{message}");
}

mod ordering {
    use super::*;
    macro_rules! invariant_test {
        ($name:ident, $variant:ident) => {
            #[tokio::test]
            async fn $name() {
                let trace = harness::capture().await.trace;
                assert!(
                    !trace.frames.is_empty(),
                    "comparator receives actual live trace"
                );
                assert_invariant(&trace, OrderingInvariant::$variant)
                    .unwrap_or_else(|error| panic!("{error}"));
            }
        };
    }
    invariant_test!(
        increasing_event_seq_per_connection,
        IncreasingEventSeqPerConnection
    );
    invariant_test!(
        input_accepted_before_caused_events,
        InputAcceptedBeforeCausedEvents
    );
    invariant_test!(
        client_tool_start_before_same_id_end,
        ToolStartBeforeMatchingToolEnd
    );
    invariant_test!(
        exactly_one_finished_after_final_delta,
        TurnFinishedExactlyOnceAfterFinalStreamDelta
    );
    invariant_test!(
        stale_lease_replacement_suppresses_event_sink,
        NoServerEventFromStaleLeaseAfterReplacement
    );
    invariant_test!(
        fanout_ordinals_ascending,
        BroadcastDeliveryStableAscendingConnectionOrdinal
    );
}

fn kinds(trace: &lotta_testkit::fixtures::traces::ReferenceTrace) -> Vec<&str> {
    trace
        .frames
        .as_slice()
        .iter()
        .filter_map(|frame| frame.wire.as_value()["type"].as_str())
        .collect()
}
fn count(values: &[&str], target: &str) -> usize {
    values.iter().filter(|value| **value == target).count()
}
fn persist(trace: &lotta_testkit::fixtures::traces::ReferenceTrace) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/slice-traces/observed.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(trace).unwrap()).unwrap();
    std::fs::rename(temporary, path).unwrap();
}
