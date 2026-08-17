//! Fixture replay for the native OpenAI-compatible and Anthropic adapters.
//!
//! Each test drives the public [`ProviderPort`] of a production adapter against the deterministic
//! loopback in [`super::loopback`], which serves the case's captured bytes under its captured
//! status and headers. The emitted trace is compared by the Task 12 comparator
//! ([`replay_provider`]), which reports the first diverging event and its index; nothing here
//! projects, filters, or reorders either side. The request the adapter actually sent is then
//! compared against `expected-request.json`, exactly for stable headers and presence-only for the
//! transport- and credential-derived ones.

use super::anthropic::Anthropic;
use super::loopback::{Loopback, RecordedRequest, has_multibyte, splits_multibyte};
use super::openai_compatible::OpenAiCompatible;
use lotta_runtime::ports::ProviderPort;
use lotta_testkit::fixtures::FixtureLoader;
use lotta_testkit::fixtures::providers::{
    Dialect, ProviderCase, ProviderCaseRecord, load_case, load_index, replay_provider,
};
use serde_json::Value;
use std::collections::BTreeSet;

/// Corpus token standing for whichever header carries the dialect's credential.
const CREDENTIAL_ROLE: &str = "credential";
/// Credential handed to the adapters; only its presence on the wire is ever asserted.
const CREDENTIAL: &str = "fixture-credential";

pub(super) fn record(id: &str) -> ProviderCaseRecord {
    load_index(&FixtureLoader::new())
        .expect("provider index")
        .cases
        .into_iter()
        .find(|case| case.id == id)
        .unwrap_or_else(|| panic!("{id} is indexed"))
}

fn case(id: &str) -> ProviderCase {
    load_case(&FixtureLoader::new(), &record(id)).unwrap_or_else(|error| panic!("{id}: {error}"))
}

fn adapter(dialect: Dialect, base: &reqwest::Url) -> Box<dyn ProviderPort> {
    match dialect {
        Dialect::Anthropic => Box::new(Anthropic::new(base, CREDENTIAL).expect("anthropic port")),
        _ => Box::new(OpenAiCompatible::new(base, CREDENTIAL).expect("openai port")),
    }
}

fn credential_header(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Anthropic => "x-api-key",
        _ => "authorization",
    }
}

/// Replays one case end to end and asserts its trace, request, and chunk boundaries.
pub(super) async fn replay_case(id: &str) {
    let case = case(id);
    let dialect = case.record.dialect;
    let raw = case.raw_stream.clone();
    let expected = case.expected_request.as_value().clone();
    let loopback = Loopback::start(&case).await;
    let provider = adapter(dialect, loopback.base());
    replay_provider(provider.as_ref(), case)
        .await
        .unwrap_or_else(|error| panic!("{id}: {error}"));
    assert_request(id, dialect, &loopback.recorded().await, &expected);
    if has_multibyte(&raw) {
        assert!(
            splits_multibyte(&raw),
            "{id}: no chunk boundary split a sequence"
        );
    }
}

fn assert_request(id: &str, dialect: Dialect, actual: &RecordedRequest, expected: &Value) {
    let method = expected["method"].as_str().expect("fixture method");
    let target = expected["endpoint"].as_str().expect("fixture endpoint");
    assert_eq!(actual.method, method, "{id}: request method");
    assert_eq!(actual.target, target, "{id}: request target");
    let body: Value = serde_json::from_slice(&actual.body).expect("request body is JSON");
    assert_eq!(body, expected["body"], "{id}: request body");
    assert_headers(id, dialect, actual, expected);
}

/// Compares stable headers exactly and dynamic ones by presence, then forbids any extra header.
fn assert_headers(id: &str, dialect: Dialect, actual: &RecordedRequest, expected: &Value) {
    let mut allowed = BTreeSet::new();
    for (name, value) in expected["headers"].as_object().expect("fixture headers") {
        allowed.insert(name.clone());
        let found = actual
            .headers
            .iter()
            .find(|(header, _)| header == name)
            .unwrap_or_else(|| panic!("{id}: missing header {name}"));
        assert_eq!(
            found.1.as_str(),
            value.as_str().expect("fixture header value"),
            "{id}: header {name}"
        );
    }
    for entry in expected["presence_only_headers"]
        .as_array()
        .expect("fixture dynamic headers")
    {
        let token = entry.as_str().expect("fixture dynamic header");
        let name = if token == CREDENTIAL_ROLE {
            credential_header(dialect).to_owned()
        } else {
            token.to_owned()
        };
        let present = actual
            .headers
            .iter()
            .any(|(header, value)| *header == name && !value.is_empty());
        assert!(present, "{id}: missing dynamic header {name}");
        allowed.insert(name);
    }
    let seen: BTreeSet<String> = actual
        .headers
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    assert_eq!(seen, allowed, "{id}: unexpected request headers");
}

#[tokio::test]
async fn openai_compatible_happy_tool() {
    replay_case("openai-compatible/happy-tool").await;
}

#[tokio::test]
async fn openai_compatible_authorization() {
    replay_case("openai-compatible/authorization").await;
}

#[tokio::test]
async fn openai_compatible_protocol_error() {
    replay_case("openai-compatible/protocol-error").await;
}

#[tokio::test]
async fn openai_compatible_retry_after() {
    replay_case("openai-compatible/retry-after").await;
}

#[tokio::test]
async fn anthropic_reasoning_redacted() {
    replay_case("anthropic/reasoning-redacted").await;
}

#[tokio::test]
async fn anthropic_authentication() {
    replay_case("anthropic/authentication").await;
}

#[tokio::test]
async fn anthropic_quota() {
    replay_case("anthropic/quota").await;
}
