//! End-to-end replay of the nine local transport captures.

use super::{
    llama_cpp::LlamaCpp,
    lmstudio::LmStudio,
    ollama::{Ollama, OllamaCloud},
};
use crate::native::loopback::{Loopback, RecordedRequest};
use lotta_runtime::{RuntimeError, ports::ProviderPort};
use lotta_testkit::fixtures::{
    FixtureLoader,
    providers::{Dialect, ProviderCase, load_case, load_index, replay_provider},
};
use serde_json::Value;
use std::collections::BTreeSet;

const LOCAL_CREDENTIAL: &str = "";
const CLOUD_CREDENTIAL: &str = "fixture-credential";

fn case(id: &str) -> ProviderCase {
    let loader = FixtureLoader::new();
    let record = load_index(&loader)
        .expect("provider index")
        .cases
        .into_iter()
        .find(|record| record.id == id)
        .unwrap_or_else(|| panic!("{id}: indexed local fixture"));
    load_case(&loader, &record).unwrap_or_else(|error| panic!("{id}: {error}"))
}

fn adapter(dialect: Dialect, base: &reqwest::Url) -> Box<dyn ProviderPort> {
    match dialect {
        Dialect::Ollama => Box::new(Ollama::new(base, LOCAL_CREDENTIAL).expect("Ollama")),
        Dialect::LmStudio => Box::new(LmStudio::new(base, LOCAL_CREDENTIAL).expect("LM Studio")),
        Dialect::LlamaCpp => Box::new(LlamaCpp::new(base, LOCAL_CREDENTIAL).expect("llama.cpp")),
        _ => panic!("not a local fixture dialect"),
    }
}

async fn replay_case(id: &str) {
    let case = case(id);
    if case.expected_request.as_value()["outcome"] == "preflight_error" {
        replay_preflight(case).await;
        return;
    }
    let dialect = case.record.dialect;
    let expected = case.expected_request.as_value().clone();
    let server = Loopback::start(&case).await;
    replay_provider(adapter(dialect, server.base()).as_ref(), case)
        .await
        .unwrap_or_else(|error| panic!("{id}: {error}"));
    assert_request(id, &server.recorded().await, &expected, false);
}

async fn replay_preflight(case: ProviderCase) {
    let server =
        Loopback::scripted(crate::native::loopback::ResponseScript::Complete(Vec::new())).await;
    let provider = Ollama::new(server.base(), LOCAL_CREDENTIAL).expect("Ollama");
    let (sink, _receiver) =
        lotta_runtime::ports::provider_event_channel(8, &case.request.cancellation)
            .expect("event channel");
    let error = provider
        .stream(case.request, sink)
        .await
        .expect_err("strict unsupported image is a typed preflight error");
    assert!(matches!(error, RuntimeError::InvalidData { .. }));
    server.assert_no_request();
}

async fn replay_cloud() {
    let id = "ollama/image-drop";
    let case = case(id);
    let expected = case.expected_request.as_value().clone();
    let server = Loopback::start(&case).await;
    let provider = OllamaCloud::with_test_core(server.base(), CLOUD_CREDENTIAL, false)
        .expect("Cloud injected core");
    replay_provider(&provider, case)
        .await
        .unwrap_or_else(|error| panic!("ollama-cloud: {error}"));
    let recorded = server.recorded().await;
    assert_request(id, &recorded, &expected, true);
    let authorization = recorded
        .headers
        .iter()
        .find(|(name, _)| name == "authorization")
        .map(|(_, value)| value.as_str());
    assert_eq!(authorization, Some("Bearer fixture-credential"));
}

fn assert_request(id: &str, actual: &RecordedRequest, expected: &Value, credential: bool) {
    assert_eq!(actual.method, expected["method"], "{id}: method");
    assert_eq!(actual.target, expected["endpoint"], "{id}: endpoint");
    assert_eq!(
        serde_json::from_slice::<Value>(&actual.body).expect("request JSON"),
        expected["body"],
        "{id}: request body"
    );
    let mut allowed = BTreeSet::new();
    for (name, value) in expected["headers"].as_object().expect("stable headers") {
        allowed.insert(name.clone());
        assert_eq!(
            actual
                .headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|v| v.1.as_str()),
            value.as_str(),
            "{id}: header {name}"
        );
    }
    for value in expected["presence_only_headers"]
        .as_array()
        .expect("presence-only headers")
    {
        let name = value.as_str().expect("header name");
        allowed.insert(name.to_owned());
        assert!(
            actual
                .headers
                .iter()
                .any(|(key, value)| key == name && !value.is_empty())
        );
    }
    if credential {
        allowed.insert("authorization".to_owned());
    }
    assert_eq!(
        actual
            .headers
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>(),
        allowed,
        "{id}: unexpected headers"
    );
}

macro_rules! replay_test {
    ($name:ident, $id:literal) => {
        #[tokio::test]
        async fn $name() {
            replay_case($id).await;
        }
    };
}

replay_test!(ollama_cancelled, "ollama/cancelled");
replay_test!(ollama_image_drop, "ollama/image-drop");
replay_test!(ollama_image_strict, "ollama/image-strict");
replay_test!(ollama_unavailable, "ollama/unavailable");
replay_test!(lm_studio_timeout, "lm-studio/timeout");
replay_test!(lm_studio_invalid_request, "lm-studio/invalid-request");
replay_test!(lm_studio_unknown_error, "lm-studio/unknown-error");
replay_test!(llama_cpp_context_overflow, "llama-cpp/context-overflow");
replay_test!(llama_cpp_overloaded, "llama-cpp/overloaded");

#[tokio::test]
async fn ollama_cloud_reuses_ollama_dialect() {
    replay_cloud().await;
}
