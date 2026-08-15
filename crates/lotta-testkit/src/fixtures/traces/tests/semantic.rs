use super::*;

fn remap(trace: &mut ReferenceTrace) {
    let mut aliases = BTreeMap::<String, String>::new();
    let mut next = 500_u64;
    let mut bases = BTreeMap::<u32, u64>::new();
    let mut emitted = BTreeMap::<u32, u64>::new();
    edit_frames(trace, |frames| {
        for frame in frames.iter_mut() {
            let mut wire = frame.wire.as_value().clone();
            remap_value(&mut wire, "/wire", &mut aliases, &mut next);
            frame.wire =
                BoundedJsonValue::new(wire).unwrap_or_else(|error| panic!("wire: {error}"));
            if is_server_broadcast(frame) {
                let base = bases.entry(frame.connection_ordinal).or_insert(1000);
                let old = wire_u64(frame, "event_seq");
                let time = emitted.entry(frame.connection_ordinal).or_default();
                *time += 1;
                set_wire(
                    frame,
                    &["emitted_at"],
                    json!(format!("2010-01-01T00:00:{:02}Z", *time)),
                );
                set_wire(frame, &["event_seq"], json!(*base + old));
                let kind = wire_type(frame).to_owned();
                set_wire(
                    frame,
                    &["idempotency_key"],
                    json!(format!("{kind}:{}:{}", *base + old, uuid_for(next))),
                );
                next += 1;
            }
        }
    });
}
fn remap_value(
    value: &mut Value,
    path: &str,
    aliases: &mut BTreeMap<String, String>,
    next: &mut u64,
) {
    if is_generated_path(path) {
        if let Some(old) = value.as_str() {
            let mapped = aliases.entry(old.to_owned()).or_insert_with(|| {
                let result = uuid_for(*next);
                *next += 1;
                result
            });
            *value = json!(mapped);
        }
        return;
    }
    if is_timestamp_path(path) {
        *value = json!("2010-01-01T00:00:00.000Z");
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                remap_value(item, &format!("{path}/{key}"), aliases, next);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                remap_value(item, &format!("{path}/{index}"), aliases, next);
            }
        }
        _ => {}
    }
}
pub(super) fn uuid_for(value: u64) -> String {
    format!("00000000-0000-4000-8000-{value:012}")
}
#[test]
fn semantic_equivalence_complete_remap_passes() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    remap(&mut actual);
    refresh_broadcasts(&mut actual, None);
    assert_ordering(&actual).expect("authority-valid remap");
    compare_semantic(&expected, &actual).expect("semantic remap");
}
#[test]
fn semantic_equivalence_event_seq_ordinal_allows_gaps() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    let mut ranks = BTreeMap::<u32, usize>::new();
    let mut key_id = 700_u64;
    edit_frames(&mut actual, |frames| {
        for frame in frames.iter_mut().filter(|frame| is_server_broadcast(frame)) {
            let rank = ranks.entry(frame.connection_ordinal).or_default();
            let sequence = [10_u64, 20, 40, 80, 160, 320, 640, 1280, 2560][*rank];
            *rank += 1;
            key_id += 1;
            let kind = wire_type(frame).to_owned();
            set_wire(frame, &["event_seq"], json!(sequence));
            set_wire(
                frame,
                &["idempotency_key"],
                json!(format!("{kind}:{sequence}:{}", uuid_for(key_id))),
            );
        }
    });
    compare_semantic(&expected, &actual).expect("ordinal rank");
}
#[test]
fn semantic_equivalence_inconsistent_alias_fails() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    remap(&mut actual);
    let index = actual
        .frames
        .as_slice()
        .iter()
        .position(|x| wire_type(x) == "input")
        .unwrap();
    edit_frames(&mut actual, |frames| {
        set_wire(
            &mut frames[index],
            &["runtime", "agent_id"],
            json!(uuid_for(999)),
        );
    });
    assert!(compare_semantic(&expected, &actual).is_err());
}
#[test]
fn semantic_equivalence_invalid_identical_top_level_uuid_fails_exact_path() {
    let mut expected = trace("vertical-slice");
    edit_frames(&mut expected, |frames| {
        set_wire(&mut frames[1], &["agent_id"], json!("invalid"));
    });
    let actual = expected.clone();
    let Err(TraceComparisonError::Semantic(error)) = compare_semantic(&expected, &actual) else {
        panic!("UUID validation expected")
    };
    assert_eq!(
        (error.frame_index, error.path.as_str()),
        (1, "/wire/agent_id")
    );
}
#[test]
fn semantic_equivalence_invalid_identical_nested_uuid_fails_exact_path() {
    let mut expected = trace("queue");
    let index = expected
        .frames
        .as_slice()
        .iter()
        .position(|frame| {
            frame
                .wire
                .as_value()
                .get("queue")
                .and_then(Value::as_array)
                .is_some_and(|q| !q.is_empty())
        })
        .unwrap();
    mutate_broadcast_group(
        &mut expected,
        index,
        &["queue", "0", "id"],
        &json!("invalid"),
    );
    assert_ordering(&expected).expect("authority-valid UUID mutation");
    let actual = expected.clone();
    let Err(TraceComparisonError::Semantic(error)) = compare_semantic(&expected, &actual) else {
        panic!("nested UUID validation expected")
    };
    assert_eq!(
        (error.frame_index, error.path.as_str()),
        (index, "/wire/queue/0/id")
    );
}
#[test]
fn semantic_equivalence_invalid_identical_time_fails_exact_path() {
    let mut expected = trace("vertical-slice");
    let index = expected
        .frames
        .as_slice()
        .iter()
        .position(is_server_broadcast)
        .unwrap();
    edit_frames(&mut expected, |frames| {
        set_wire(&mut frames[index], &["emitted_at"], json!("tomorrow"));
    });
    let actual = expected.clone();
    let Err(TraceComparisonError::Semantic(error)) = compare_semantic(&expected, &actual) else {
        panic!("timestamp validation expected")
    };
    assert_eq!(
        (error.frame_index, error.path.as_str()),
        (index, "/wire/emitted_at")
    );
}
#[test]
fn semantic_equivalence_emitted_at_regression_fails_exact_path() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    let indexes = actual
        .frames
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, frame)| is_server_broadcast(frame) && frame.connection_ordinal == 1)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    edit_frames(&mut actual, |frames| {
        set_wire(
            &mut frames[indexes[1]],
            &["emitted_at"],
            json!("1999-01-01T00:00:00Z"),
        );
    });
    let Err(TraceComparisonError::Semantic(error)) = compare_semantic(&expected, &actual) else {
        panic!("temporal validation expected")
    };
    assert_eq!(
        (error.frame_index, error.path.as_str()),
        (indexes[1], "/wire/emitted_at")
    );
}
#[test]
fn semantic_equivalence_delta_date_regression_fails_exact_path() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    let indexes = actual
        .frames
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, frame)| wire_type(frame) == "stream_delta" && frame.connection_ordinal == 1)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    mutate_broadcast_group(
        &mut actual,
        indexes[1],
        &["delta", "date"],
        &json!("1999-01-01T00:00:00Z"),
    );
    assert_ordering(&actual).expect("authority-valid delta mutation");
    let Err(TraceComparisonError::Semantic(error)) = compare_semantic(&expected, &actual) else {
        panic!("delta temporal validation expected")
    };
    assert_eq!(
        (error.frame_index, error.path.as_str()),
        (indexes[1], "/wire/delta/date")
    );
}
#[test]
fn semantic_equivalence_queue_time_drift_fails_exact_path() {
    let expected = trace("queue");
    let mut actual = expected.clone();
    let source = actual
        .frames
        .as_slice()
        .iter()
        .position(|frame| {
            frame
                .wire
                .as_value()
                .get("queue")
                .and_then(Value::as_array)
                .is_some_and(|q| !q.is_empty())
        })
        .unwrap();
    edit_frames(&mut actual, |frames| {
        let begin = source - 1;
        let mut repeat_begin = frames[begin].clone();
        repeat_begin.broadcast_emission = None;
        set_wire(&mut repeat_begin, &["emission"], json!("queue-repeat"));
        let mut repeat = frames[source].clone();
        repeat.broadcast_emission = Some("queue-repeat".into());
        set_wire(&mut repeat, &["event_seq"], json!(99));
        set_wire(&mut repeat, &["emitted_at"], json!("2000-01-01T00:01:00Z"));
        set_wire(
            &mut repeat,
            &["idempotency_key"],
            json!(format!("update_queue:99:{}", uuid_for(998))),
        );
        frames.extend([repeat_begin, repeat]);
    });
    renumber(&mut actual);
    let repeated = actual.frames.len() - 1;
    mutate_broadcast_group(
        &mut actual,
        repeated,
        &["queue", "0", "enqueued_at"],
        &json!("2000-01-02T00:00:00Z"),
    );
    assert_ordering(&actual).expect("authority-valid queue mutation");
    let Err(TraceComparisonError::Semantic(error)) = compare_semantic(&expected, &actual) else {
        panic!("queue temporal validation expected")
    };
    assert_eq!(error.path, "/wire/queue/0/enqueued_at");
}
#[test]
fn semantic_equivalence_duplicate_idempotency_fails() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    let indexes = actual
        .frames
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, x)| is_server_broadcast(x))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let key = wire_string(&actual.frames.as_slice()[indexes[0]], "idempotency_key").unwrap();
    edit_frames(&mut actual, |frames| {
        set_wire(&mut frames[indexes[1]], &["idempotency_key"], json!(key));
    });
    assert!(matches!(
        compare_semantic(&expected, &actual),
        Err(TraceComparisonError::Semantic(_))
    ));
}
#[test]
fn semantic_equivalence_duplicate_recipient_key_fails_exact_path() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    let indexes = actual
        .frames
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, frame)| frame.broadcast_emission.as_deref() == Some("emission-6"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let key = wire_string(&actual.frames.as_slice()[indexes[0]], "idempotency_key").unwrap();
    edit_frames(&mut actual, |frames| {
        set_wire(&mut frames[indexes[1]], &["idempotency_key"], json!(key));
    });
    let Err(TraceComparisonError::Semantic(error)) = compare_semantic(&expected, &actual) else {
        panic!("delivery key validation expected")
    };
    assert_eq!(
        (error.frame_index, error.path.as_str()),
        (indexes[1], "/wire/idempotency_key")
    );
}
#[test]
fn semantic_equivalence_wrong_key_type_fails() {
    assert_bad_key("wrong", None, Some(uuid_for(999)));
}
#[test]
fn semantic_equivalence_wrong_key_seq_fails() {
    assert_bad_key("update_device_status", Some(99), Some(uuid_for(999)));
}
#[test]
fn semantic_equivalence_wrong_key_uuid_fails() {
    assert_bad_key("update_device_status", None, Some("invalid".into()));
}
fn assert_bad_key(kind: &str, sequence: Option<u64>, uuid: Option<String>) {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    let index = actual
        .frames
        .as_slice()
        .iter()
        .position(is_server_broadcast)
        .unwrap();
    let seq = sequence.unwrap_or_else(|| wire_u64(&actual.frames.as_slice()[index], "event_seq"));
    edit_frames(&mut actual, |frames| {
        set_wire(
            &mut frames[index],
            &["idempotency_key"],
            json!(format!("{kind}:{seq}:{}", uuid.unwrap_or_default())),
        );
    });
    assert!(matches!(
        compare_semantic(&expected, &actual),
        Err(TraceComparisonError::Semantic(_))
    ));
}
#[test]
fn semantic_equivalence_changed_client_id_fails() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    edit_frames(&mut actual, |frames| {
        frames
            .iter_mut()
            .find(|x| x.caused_by.is_some())
            .unwrap()
            .caused_by = Some("changed".into());
    });
    assert!(compare_semantic(&expected, &actual).is_err());
}
#[test]
fn semantic_equivalence_changed_discriminant_fails_exact_path() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    edit_frames(&mut actual, |frames| {
        set_wire(&mut frames[1], &["type"], json!("sync"));
    });
    let Err(TraceComparisonError::Semantic(error)) = compare_semantic(&expected, &actual) else {
        panic!("semantic divergence expected")
    };
    assert_eq!((error.frame_index, error.path.as_str()), (1, "/wire/type"));
}
#[test]
fn semantic_equivalence_changed_body_shape_fails() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    edit_frames(&mut actual, |frames| {
        set_wire(&mut frames[1], &["request_id"], json!({"wrong": true}));
    });
    assert!(matches!(
        compare_semantic(&expected, &actual),
        Err(TraceComparisonError::Semantic(_))
    ));
}
#[test]
fn semantic_equivalence_changed_order_fails() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    edit_frames(&mut actual, |frames| frames.swap(0, 1));
    renumber(&mut actual);
    assert!(compare_semantic(&expected, &actual).is_err());
}
#[test]
fn semantic_equivalence_authority_precedes_semantic() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    let index = actual
        .frames
        .as_slice()
        .iter()
        .position(|frame| wire_type(frame) == "broadcast_begin")
        .unwrap();
    edit_frames(&mut actual, |frames| {
        set_wire(
            &mut frames[index],
            &["payload_sha256"],
            json!("0000000000000000000000000000000000000000000000000000000000000000"),
        );
    });
    for invariant in OrderingInvariant::ALL {
        assert_invariant(&actual, invariant)
            .expect("wire invariant does not inspect authority hash");
    }
    assert!(matches!(
        compare_semantic(&expected, &actual),
        Err(TraceComparisonError::Ordering(_))
    ));
}
#[test]
fn semantic_equivalence_ordering_precedes_divergence() {
    let expected = trace("vertical-slice");
    let mut actual = expected.clone();
    let indexes = actual
        .frames
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, x)| is_server_broadcast(x) && x.connection_ordinal == 1)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    edit_frames(&mut actual, |frames| {
        set_wire(&mut frames[indexes[1]], &["event_seq"], json!(1));
    });
    assert!(matches!(
        compare_semantic(&expected, &actual),
        Err(TraceComparisonError::Ordering(_))
    ));
}
