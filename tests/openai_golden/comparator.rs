use serde_json::Value;

pub fn compare(case: &str, mode: &str, expected: &Value, actual: &Value) {
    if mode != "sse" || expected.get("attempts").is_some() {
        assert_eq!(
            expected, actual,
            "case {case}: expected={expected} actual={actual}"
        );
        return;
    }
    assert_eq!(expected["status"], actual["status"], "case {case} status");
    assert_eq!(
        expected["headers"], actual["headers"],
        "case {case} headers"
    );
    assert_eq!(
        expected["dynamic_map"], actual["dynamic_map"],
        "case {case} dynamic map"
    );
    let expected_events = expected["events"].as_array().expect("fixture events");
    let actual_events = actual["events"].as_array().expect("actual events");
    assert_events(case, expected_events, actual_events);
}

pub fn assert_events(case: &str, expected: &[Value], actual: &[Value]) {
    let length = expected.len().max(actual.len());
    for index in 0..length {
        let left = expected.get(index);
        let right = actual.get(index);
        assert_eq!(
            left, right,
            "case {case} event index {index}: expected={left:?} actual={right:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_mutations_are_rejected_with_index() {
        let expected = vec![json!("a"), json!("b"), json!("c")];
        for actual in [
            vec![json!("a"), json!("b")],
            vec![json!("a"), json!("b"), json!("c"), json!("d")],
            vec![json!("b"), json!("a"), json!("c")],
            vec![json!("a"), json!("b"), json!("b")],
        ] {
            let panic = std::panic::catch_unwind(|| assert_events("mutation", &expected, &actual))
                .expect_err("event mutation must fail");
            assert!(panic_message(panic).contains("event index"));
        }
    }

    fn panic_message(value: Box<dyn std::any::Any + Send>) -> String {
        value
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| value.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_else(|| "non-string panic".to_owned())
    }
}
