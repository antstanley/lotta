use crate::{comparator, harness::Harness, sanitize, schema::*};
use serde_json::{Value, json};
use std::collections::BTreeSet;

async fn run_named_case(name: &str) {
    let index = sanitize::load_index();
    let selected = index
        .cases
        .iter()
        .find(|case| case.name == name)
        .expect("indexed case");
    let order = dependency_order(&index, selected);
    let mut harness = Harness::start(name).await;
    for entry in order {
        let fixture = sanitize::load_fixture(entry);
        let (actual, observable, relationships) = harness.execute(&fixture).await;
        comparator::compare(&fixture.name, &fixture.mode, &fixture.expected, &actual);
        assert_eq!(
            observable, fixture.observable,
            "case {} observable delta",
            fixture.name
        );
        assert_eq!(
            relationships, fixture.relationships,
            "case {} relationship map",
            fixture.name
        );
    }
    harness.shutdown().await;
}

fn dependency_order<'a>(index: &'a FixtureIndex, selected: &'a IndexCase) -> Vec<&'a IndexCase> {
    let mut order = Vec::new();
    for dependency in &selected.dependencies {
        let entry = index
            .cases
            .iter()
            .find(|case| &case.name == dependency)
            .expect("dependency indexed");
        assert!(entry.dependencies.is_empty(), "dependency depth bound");
        order.push(entry);
    }
    order.push(selected);
    order
}

#[test]
fn index_is_complete_strict_and_sanitized() {
    let index = sanitize::load_index();
    assert_eq!(index.schema_version, 2);
    assert_eq!(index.baseline_commit, BASELINE_COMMIT);
    assert_eq!(index.baseline_tree, BASELINE_TREE);
    assert_eq!(
        index.capture_command,
        "LOTTA_BUN=bun node tools/capture-openai-fixtures.mjs"
    );
    assert_eq!(
        (index.runtime.name.as_str(), index.runtime.version.as_str()),
        ("bun", "1.3.14")
    );
    assert_eq!(index.bounds.cases_max, FIXTURE_CASES_MAX);
    assert_eq!(index.bounds.fixture_bytes_max, FIXTURE_BYTES_MAX);
    assert_eq!(index.bounds.events_per_case_max, SSE_EVENTS_MAX);
    assert_eq!(index.bounds.json_depth_max, JSON_DEPTH_MAX);
    assert_eq!(index.cases.len(), 11);
    for required in [
        "handler",
        "turn_bridge",
        "common",
        "lockfile",
        "capture_runner",
    ] {
        let pin = index
            .sources
            .get(required)
            .unwrap_or_else(|| panic!("missing source {required}"));
        assert_eq!(pin.sha256.len(), 64, "source hash length");
    }
    sanitize::assert_corpus_layout(&index);
    validate_cases(&index);
}

fn validate_cases(index: &FixtureIndex) {
    let mut names = BTreeSet::new();
    for entry in &index.cases {
        assert!(names.insert(&entry.name), "duplicate case name");
        let fixture = sanitize::load_fixture(entry);
        assert_eq!(fixture.schema_version, 2);
        assert_eq!(fixture.name, entry.name);
        assert_eq!(fixture.route, entry.route);
        assert_eq!(fixture.mode, entry.mode);
        assert_eq!(fixture.dependencies, entry.dependencies);
        assert_eq!(fixture.request.mode, fixture.mode);
        let value = serde_json::to_value(&fixture).expect("fixture value");
        assert!(json_depth(&value) <= JSON_DEPTH_MAX, "fixture depth bound");
    }
}

fn json_depth(value: &Value) -> usize {
    let mut maximum = 1;
    let mut stack = vec![(value, 1)];
    while let Some((current, depth)) = stack.pop() {
        maximum = maximum.max(depth);
        match current {
            Value::Array(values) => stack.extend(values.iter().map(|child| (child, depth + 1))),
            Value::Object(values) => stack.extend(values.values().map(|child| (child, depth + 1))),
            _ => {}
        }
    }
    maximum
}

#[test]
fn strict_schema_rejects_missing_and_extra_observables() {
    let index = sanitize::load_index();
    let mut index_value = serde_json::to_value(&index).expect("index JSON");
    index_value["extra"] = json!(true);
    assert!(serde_json::from_value::<FixtureIndex>(index_value).is_err());
    let mut index_value = serde_json::to_value(&index).expect("index JSON");
    index_value
        .as_object_mut()
        .expect("index")
        .remove("baseline_tree");
    assert!(serde_json::from_value::<FixtureIndex>(index_value).is_err());
    let entry = index.cases.into_iter().next().expect("fixture entry");
    let fixture = sanitize::load_fixture(&entry);
    let mut value = serde_json::to_value(fixture).expect("fixture JSON");
    value["observable"]
        .as_object_mut()
        .expect("observable")
        .remove("cleanup");
    assert!(serde_json::from_value::<Fixture>(value).is_err());
    let fixture = sanitize::load_fixture(&entry);
    let mut value = serde_json::to_value(fixture).expect("fixture JSON");
    value["observable"]["extra"] = json!(true);
    assert!(serde_json::from_value::<Fixture>(value).is_err());
}

macro_rules! golden_case {
    ($name:ident, $case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() {
            run_named_case($case).await;
        }
    };
}

golden_case!(fixture_models_json, "models_json");
golden_case!(fixture_chat_headerless_json, "chat_headerless_json");
golden_case!(fixture_chat_stream_sse, "chat_stream_sse");
golden_case!(fixture_chat_stateful_first, "chat_stateful_first");
golden_case!(fixture_chat_stateful_newest, "chat_stateful_newest");
golden_case!(fixture_chat_idempotent_retry, "chat_idempotent_retry");
golden_case!(fixture_responses_nonstored_json, "responses_nonstored_json");
golden_case!(fixture_responses_stream_sse, "responses_stream_sse");
golden_case!(fixture_responses_stored_json, "responses_stored_json");
golden_case!(fixture_responses_previous_json, "responses_previous_json");
golden_case!(fixture_responses_no_idempotency, "responses_no_idempotency");
