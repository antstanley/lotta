use super::*;
use serde_json::{Value, json};

fn vertical() -> ReferenceTrace {
    load_trace("vertical-slice").expect("vertical trace")
}
fn set_wire(frame: &mut TraceFrame, path: &[&str], value: Value) {
    let mut wire = frame.wire.as_value().clone();
    let mut current = &mut wire;
    for key in &path[..path.len() - 1] {
        current = &mut current[*key];
    }
    current[path[path.len() - 1]] = value;
    frame.wire = BoundedJsonValue::new(wire).expect("bounded wire");
}
fn edit(trace: &mut ReferenceTrace, change: impl FnOnce(&mut Vec<TraceFrame>)) {
    let mut frames = trace.frames.as_slice().to_vec();
    change(&mut frames);
    trace.frames = BoundedVec::new(frames).expect("bounded frames");
}
fn renumber(frames: &mut [TraceFrame]) {
    for (index, frame) in frames.iter_mut().enumerate() {
        frame.frame_index = index;
    }
}
fn index(trace: &ReferenceTrace, kind: &str) -> usize {
    trace
        .frames
        .as_slice()
        .iter()
        .position(|frame| wire_type(frame) == kind)
        .expect("frame kind")
}
fn emission(trace: &ReferenceTrace, label: &str) -> Vec<usize> {
    trace
        .frames
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, frame)| frame.broadcast_emission.as_deref() == Some(label))
        .map(|(index, _)| index)
        .collect()
}
fn assert_class(trace: &ReferenceTrace, invariant: OrderingInvariant) -> TraceInvariantViolation {
    let error = assert_ordering(trace).expect_err("authority mutation rejected");
    assert_eq!(error.invariant, invariant);
    error
}
fn cause(trace: &ReferenceTrace) -> TraceInvariantViolation {
    assert_class(trace, OrderingInvariant::InputAcceptedBeforeCausedEvents)
}
fn lease(trace: &ReferenceTrace) -> TraceInvariantViolation {
    assert_class(
        trace,
        OrderingInvariant::NoServerEventFromStaleLeaseAfterReplacement,
    )
}
fn group(trace: &ReferenceTrace) -> TraceInvariantViolation {
    assert_class(
        trace,
        OrderingInvariant::BroadcastDeliveryStableAscendingConnectionOrdinal,
    )
}

#[test]
fn authority_all_eight_traces_pass() {
    let (_, traces) = load_all().expect("corpus");
    for trace in traces {
        assert_ordering(&trace).expect("authoritative trace");
    }
}
#[test]
fn authority_forged_input_cause_is_rejected() {
    let mut trace = vertical();
    let at = index(&trace, "input");
    edit(&mut trace, |frames| {
        frames[at].caused_by = Some("forged".into());
    });
    assert_eq!(cause(&trace).first_frame.index, at);
}
#[test]
fn authority_acceptance_request_is_rejected() {
    let mut trace = vertical();
    let at = index(&trace, "input_accepted");
    edit(&mut trace, |frames| {
        set_wire(&mut frames[at], &["request_id"], json!("missing"));
    });
    assert_eq!(cause(&trace).first_frame.index, at);
}
#[test]
fn authority_acceptance_cause_is_rejected_with_input_pair() {
    let mut trace = vertical();
    let input = index(&trace, "input");
    let accepted = index(&trace, "input_accepted");
    edit(&mut trace, |frames| {
        frames[accepted].caused_by = Some("forged".into());
    });
    let error = cause(&trace);
    assert_eq!(
        (
            error.first_frame.index,
            error.second_frame.expect("pair").index
        ),
        (input, accepted)
    );
}
#[test]
fn authority_turn_marker_cause_is_rejected() {
    let mut trace = vertical();
    let at = index(&trace, "turn_started");
    edit(&mut trace, |frames| {
        set_wire(&mut frames[at], &["caused_by"], json!("forged"));
    });
    assert_eq!(lease(&trace).first_frame.index, at);
}
#[test]
fn authority_delivery_lease_id_is_rejected_with_begin_pair() {
    let mut trace = vertical();
    let delivery = emission(&trace, "emission-4")[0];
    edit(&mut trace, |frames| {
        frames[delivery].lease_id = Some("forged".into());
    });
    let error = lease(&trace);
    assert_eq!(
        (
            error.first_frame.index,
            error.second_frame.expect("pair").index
        ),
        (delivery - 1, delivery)
    );
}
#[test]
fn authority_delivery_lease_currentness_is_rejected() {
    let mut trace = vertical();
    let delivery = emission(&trace, "emission-4")[0];
    edit(&mut trace, |frames| {
        frames[delivery].lease_current = Some(false);
    });
    assert_eq!(
        lease(&trace).second_frame.expect("delivery").index,
        delivery
    );
}
#[test]
fn authority_forged_emission_label_is_rejected() {
    let mut trace = vertical();
    let delivery = emission(&trace, "emission-4")[0];
    edit(&mut trace, |frames| {
        frames[delivery].broadcast_emission = Some("forged".into());
    });
    assert_eq!(group(&trace).first_frame.index, delivery - 1);
}
#[test]
fn authority_begin_hash_is_rejected() {
    let mut trace = vertical();
    let begin = index(&trace, "broadcast_begin");
    edit(&mut trace, |frames| {
        set_wire(
            &mut frames[begin],
            &["payload_sha256"],
            json!("0".repeat(64)),
        );
    });
    assert_eq!(group(&trace).first_frame.index, begin);
}
#[test]
fn authority_begin_message_type_is_rejected() {
    let mut trace = vertical();
    let begin = index(&trace, "broadcast_begin");
    edit(&mut trace, |frames| {
        set_wire(&mut frames[begin], &["message_type"], json!("wrong"));
    });
    assert_eq!(
        group(&trace).second_frame.expect("delivery").index,
        begin + 1
    );
}

macro_rules! ordinal_mutation {
    ($name:ident, $value:expr) => {
        #[test]
        fn $name() {
            let mut trace = vertical();
            let begin = emission(&trace, "emission-6")[0] - 1;
            edit(&mut trace, |frames| {
                set_wire(&mut frames[begin], &["subscriber_ordinals"], json!($value));
            });
            assert_eq!(group(&trace).first_frame.index, begin);
        }
    };
}
ordinal_mutation!(authority_subscriber_ordinal_missing_is_rejected, [1]);
ordinal_mutation!(authority_subscriber_ordinal_extra_is_rejected, [1, 2, 3]);
ordinal_mutation!(authority_subscriber_ordinal_duplicate_is_rejected, [1, 1]);
ordinal_mutation!(authority_subscriber_ordinal_unsorted_is_rejected, [2, 1]);

#[test]
fn authority_missing_delivery_is_rejected_after_safe_renumber() {
    let mut trace = vertical();
    let deliveries = emission(&trace, "emission-6");
    edit(&mut trace, |frames| {
        frames.remove(deliveries[1]);
        renumber(frames);
    });
    assert_eq!(group(&trace).first_frame.index, deliveries[0] - 1);
}
#[test]
fn authority_extra_duplicate_delivery_is_rejected() {
    let mut trace = vertical();
    let deliveries = emission(&trace, "emission-6");
    edit(&mut trace, |frames| {
        frames.insert(deliveries[1] + 1, frames[deliveries[1]].clone());
        renumber(frames);
    });
    assert_eq!(group(&trace).first_frame.index, deliveries[0] - 1);
}
#[test]
fn authority_recipient_payload_change_is_rejected() {
    let mut trace = vertical();
    let delivery = emission(&trace, "emission-6")[1];
    edit(&mut trace, |frames| {
        set_wire(
            &mut frames[delivery],
            &["delta", "content"],
            json!("changed"),
        );
    });
    assert_eq!(
        group(&trace).second_frame.expect("delivery").index,
        delivery
    );
}
#[test]
fn authority_server_delivery_after_disconnect_is_rejected() {
    let mut trace = vertical();
    let delivery = emission(&trace, "emission-6")[1];
    edit(&mut trace, |frames| {
        let mut disconnect = frames[15].clone();
        set_wire(&mut disconnect, &["type"], json!("disconnect"));
        disconnect.frame_index = delivery;
        frames.insert(delivery, disconnect);
        renumber(frames);
    });
    let error = group(&trace);
    assert_eq!(error.first_frame.index, delivery - 2);
    assert_eq!(error.second_frame.expect("delivery").index, delivery - 1);
}
