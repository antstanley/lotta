//! Corpus coverage assertions for the native adapters.
//!
//! Kept out of the `native::replay` path so that selector resolves to exactly one replay test per
//! native fixture.

use super::replay::record;
use lotta_testkit::fixtures::FixtureLoader;
use lotta_testkit::fixtures::providers::{Dialect, load_index};

/// Every corpus case the native adapters own, in index order.
const NATIVE_CASES: [&str; 7] = [
    "openai-compatible/happy-tool",
    "anthropic/reasoning-redacted",
    "openai-compatible/retry-after",
    "anthropic/authentication",
    "openai-compatible/authorization",
    "anthropic/quota",
    "openai-compatible/protocol-error",
];
/// Exact OpenAI-compatible case count in the corpus.
const OPENAI_CASE_COUNT: usize = 4;
/// Exact Anthropic case count in the corpus.
const ANTHROPIC_CASE_COUNT: usize = 3;

/// One replay test exists per native fixture, and the corpus holds exactly those fixtures.
#[test]
fn case_count_matches_corpus() {
    let index = load_index(&FixtureLoader::new()).expect("provider index");
    let native: Vec<&str> = index
        .cases
        .iter()
        .filter(|case| matches!(case.dialect, Dialect::OpenaiCompatible | Dialect::Anthropic))
        .map(|case| case.id.as_str())
        .collect();
    assert_eq!(native, NATIVE_CASES.to_vec());
    assert_eq!(native.len(), OPENAI_CASE_COUNT + ANTHROPIC_CASE_COUNT);
    let openai = native.iter().filter(|id| id.starts_with("openai-")).count();
    assert_eq!(openai, OPENAI_CASE_COUNT);
    assert_eq!(native.len() - openai, ANTHROPIC_CASE_COUNT);
    for id in NATIVE_CASES {
        assert!(record(id).response.is_some(), "{id}: response metadata");
    }
}
