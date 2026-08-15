#[test]
fn runtime_entry_owns_queue_and_history() {
    let source = include_str!("../../registry.rs");
    let entry = source
        .split("pub(crate) struct RuntimeEntry")
        .nth(1)
        .unwrap_or_else(|| panic!("RuntimeEntry missing"))
        .split('}')
        .next()
        .unwrap_or_else(|| panic!("RuntimeEntry body missing"));
    assert!(entry.contains("queue: ConversationQueue"));
    assert!(entry.contains("admission_history: AdmissionHistory"));
}

#[test]
fn registry_exposes_no_mutable_queue() {
    let source = include_str!("../../registry.rs");
    assert!(!source.contains("-> &mut ConversationQueue"));
    assert!(!source.contains("-> Option<&mut ConversationQueue>"));
    assert!(source.contains("pub fn dequeue_queue("));
    assert!(source.contains("pub fn pump_queue("));
}

#[test]
fn queue_mutations_are_owned_and_no_event_accumulator_exists() {
    let source = include_str!("../../registry.rs");
    assert!(!source.contains("queue_events:"));
    assert!(!source.contains("event_accumulator"));
    assert!(!source.contains("residency: RuntimeResidency { queue"));
}

#[test]
fn residency_has_no_queue_field() {
    let source = include_str!("../../registry.rs");
    let residency = source
        .split("pub struct RuntimeResidency")
        .nth(1)
        .unwrap_or_else(|| panic!("RuntimeResidency missing"))
        .split('}')
        .next()
        .unwrap_or_else(|| panic!("RuntimeResidency body missing"));
    assert!(!residency.contains("queue"));
}
