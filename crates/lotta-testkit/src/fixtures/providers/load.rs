//! Sanitized provider-stream fixture corpus and replay support.

use super::ProviderFixtureError;
use super::convert::{convert_request, convert_trace, normalize_baseline, validate_trace};
use super::types::{
    BaselineEventRecord, CASE_COUNT, Dialect, Dimension, ERROR_KIND_COUNT, FILES,
    FixtureEventRecord, INVENTORY_COUNT, InventoryEntry, PI_AI_VERSION, ProviderCase,
    ProviderCaseRecord, ProviderErrorKind, ProviderIndex, RESPONSE_STATUS_MAX, RESPONSE_STATUS_MIN,
    RawFormat, RequestFixture, SOURCE_COMMIT, TRACE_EVENTS_MAX,
};
use crate::TestkitError;
use crate::fixtures::{FixtureLoader, sha256};
use lotta_domain::{BoundedJsonValue, BoundedVec};
use std::collections::BTreeSet;
use std::path::{Component, Path};

/// Loads and validates the complete provider index and all indexed bytes.
///
/// # Errors
/// Returns a typed fixture error for malformed metadata, tree, bytes, or hashes.
pub fn load_index(loader: &FixtureLoader) -> Result<ProviderIndex, TestkitError> {
    let index: ProviderIndex = loader.load("providers/index.json")?;
    validate_index_metadata(&index)?;
    validate_tree(loader, &index)?;
    Ok(index)
}
/// Loads and semantically validates one indexed case.
///
/// # Errors
/// Returns a typed fixture error for malformed request, raw wire, events, or trace semantics.
pub fn load_case(
    loader: &FixtureLoader,
    record: &ProviderCaseRecord,
) -> Result<ProviderCase, ProviderFixtureError> {
    validate_record(record)?;
    let root = format!("providers/{}/{}", record.dialect.path(), record.name);
    let request_fixture: RequestFixture = loader.load(format!("{root}/request.json"))?;
    let request = convert_request(request_fixture)?;
    let expected_request = loader.load(format!("{root}/expected-request.json"))?;
    let raw_stream = loader.load_text(format!("{root}/raw-stream.txt"))?;
    validate_raw(record.raw_format, &raw_stream)?;
    let baseline_text = loader.load_text(format!("{root}/baseline-events.jsonl"))?;
    let baseline_events = parse_jsonl(&baseline_text)?;
    let normalized_baseline = normalize_baseline(baseline_events.as_slice())?;
    let trace_records: BoundedVec<FixtureEventRecord, TRACE_EVENTS_MAX> =
        loader.load(format!("{root}/expected-trace.json"))?;
    let expected_trace = convert_trace(trace_records.as_slice())?;
    if normalized_baseline != expected_trace {
        return Err(ProviderFixtureError::Semantic("baseline expected equality"));
    }
    validate_trace(expected_trace.as_slice())?;
    let host_expected_trace = record
        .host_expected_trace
        .as_ref()
        .map(|path| -> Result<_, ProviderFixtureError> {
            let records: BoundedVec<FixtureEventRecord, TRACE_EVENTS_MAX> =
                loader.load(format!("providers/{path}"))?;
            let trace = convert_trace(records.as_slice())?;
            validate_trace(trace.as_slice())?;
            Ok(trace)
        })
        .transpose()?;
    Ok(ProviderCase {
        record: record.clone(),
        request,
        expected_request,
        raw_stream,
        baseline_event_count: baseline_events.len(),
        expected_trace,
        host_expected_trace,
    })
}
/// Loads every fixture case in index order.
///
/// # Errors
/// Returns the first typed fixture or semantic error.
pub fn load_all(loader: &FixtureLoader) -> Result<Vec<ProviderCase>, ProviderFixtureError> {
    let index = load_index(loader)?;
    let mut cases = Vec::with_capacity(CASE_COUNT);
    for record in index.cases {
        cases.push(load_case(loader, &record)?);
    }
    Ok(cases)
}

fn malformed() -> TestkitError {
    TestkitError::MalformedFixture {
        path: "fixtures/providers/index.json".into(),
    }
}
fn validate_index_metadata(index: &ProviderIndex) -> Result<(), TestkitError> {
    if index.schema_version != 1
        || index.source_commit != SOURCE_COMMIT
        || index.pi_ai_version != PI_AI_VERSION
        || index.generator != "tools/capture-provider-streams.mjs"
        || index.dialects != Dialect::ALL
        || index.dimensions != Dimension::ALL
        || index.error_kind_to_case.len() != ERROR_KIND_COUNT
    {
        return Err(malformed());
    }
    let case_ids: BTreeSet<_> = index.cases.iter().map(|case| case.id.as_str()).collect();
    if case_ids.len() != CASE_COUNT {
        return Err(malformed());
    }
    for (position, kind) in ProviderErrorKind::ALL.into_iter().enumerate() {
        if index.error_kind_to_case[position].0 != kind
            || !index
                .cases
                .iter()
                .any(|case| case.id == index.error_kind_to_case[position].1)
        {
            return Err(malformed());
        }
    }
    validate_inventory(&index.inventory)
}
fn validate_inventory(entries: &[InventoryEntry; INVENTORY_COUNT]) -> Result<(), TestkitError> {
    let mut previous = "";
    for entry in entries {
        confined(&entry.path)?;
        if entry.path.as_str() <= previous
            || entry.path == "index.json"
            || entry.sha256.len() != 64
            || !entry
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(malformed());
        }
        previous = &entry.path;
    }
    Ok(())
}
fn confined(path: &str) -> Result<(), TestkitError> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        Err(malformed())
    } else {
        Ok(())
    }
}
fn validate_tree(loader: &FixtureLoader, index: &ProviderIndex) -> Result<(), TestkitError> {
    let expected_top = [
        "anthropic/",
        "index.json",
        "llama-cpp/",
        "lm-studio/",
        "ollama/",
        "openai-compatible/",
    ];
    if loader.list_children("providers")? != expected_top {
        return Err(malformed());
    }
    let actual = loader.list_tree("providers")?;
    let mut expected: Vec<_> = index.inventory.iter().map(|v| v.path.clone()).collect();
    expected.push("index.json".into());
    expected.sort();
    if actual != expected {
        return Err(malformed());
    }
    for entry in &index.inventory {
        let bytes = loader.load_bytes(format!("providers/{}", entry.path))?;
        if bytes.len() != entry.bytes || sha256::lowercase_hex(&bytes) != entry.sha256 {
            return Err(malformed());
        }
    }
    Ok(())
}
fn validate_record(record: &ProviderCaseRecord) -> Result<(), ProviderFixtureError> {
    if record.id != format!("{}/{}", record.dialect.path(), record.name) {
        return Err(ProviderFixtureError::Semantic("case identifier"));
    }
    for (actual, file) in record.paths.iter().zip(FILES) {
        if actual != &format!("{}/{file}", record.id) {
            return Err(ProviderFixtureError::Semantic("case path"));
        }
    }
    validate_host_trace(record)?;
    validate_response(record)
}

fn validate_host_trace(record: &ProviderCaseRecord) -> Result<(), ProviderFixtureError> {
    match (&record.host_expected_trace, &record.host_trace_provenance) {
        (None, None) => Ok(()),
        (Some(path), Some(provenance))
            if record.id == "openai-compatible/happy-tool"
                && path == "openai-compatible/happy-tool/expected-host-trace.json"
                && provenance
                    == "pi-ai 0.82.1 omits OpenAI system_fingerprint from its public events" =>
        {
            Ok(())
        }
        _ => Err(ProviderFixtureError::Semantic("host trace provenance")),
    }
}
/// A plain JSON capture carries no framing, so its status and headers must be indexed and
/// non-2xx; every recorded response must otherwise stay a well-formed replayable response.
fn validate_response(record: &ProviderCaseRecord) -> Result<(), ProviderFixtureError> {
    let Some(response) = &record.response else {
        return if record.raw_format == RawFormat::Json {
            Err(ProviderFixtureError::Semantic("json capture response"))
        } else {
            Ok(())
        };
    };
    if response.status < RESPONSE_STATUS_MIN || response.status > RESPONSE_STATUS_MAX {
        return Err(ProviderFixtureError::Semantic("response status"));
    }
    if record.raw_format == RawFormat::Json && response.is_success() {
        return Err(ProviderFixtureError::Semantic("json capture status"));
    }
    let mut names = BTreeSet::new();
    for header in response.headers.as_slice() {
        let lowercase = header.name.bytes().all(|byte| !byte.is_ascii_uppercase());
        if header.name.is_empty() || !lowercase || !names.insert(header.name.as_str()) {
            return Err(ProviderFixtureError::Semantic("response header"));
        }
        if header.value.is_empty() || header.value.bytes().any(|byte| byte < b' ') {
            return Err(ProviderFixtureError::Semantic("response header value"));
        }
    }
    Ok(())
}
fn parse_jsonl(
    text: &str,
) -> Result<BoundedVec<BaselineEventRecord, TRACE_EVENTS_MAX>, ProviderFixtureError> {
    let mut output = Vec::new();
    for line in text.lines() {
        if output.len() == TRACE_EVENTS_MAX {
            return Err(ProviderFixtureError::TraceLimit);
        }
        let event = serde_json::from_str(line)
            .map_err(|_| ProviderFixtureError::Semantic("baseline JSONL"))?;
        output.push(event);
    }
    BoundedVec::new(output).map_err(|_| ProviderFixtureError::TraceLimit)
}
pub(super) fn validate_raw(format: RawFormat, text: &str) -> Result<(), ProviderFixtureError> {
    if !text.contains("SANITIZED_FIXTURE") {
        return Err(ProviderFixtureError::Semantic("raw sanitization marker"));
    }
    match format {
        RawFormat::Json => {
            let value: BoundedJsonValue = serde_json::from_str(text)
                .map_err(|_| ProviderFixtureError::Semantic("JSON body"))?;
            if !value.as_value().is_object() {
                return Err(ProviderFixtureError::Semantic("JSON body object"));
            }
        }
        RawFormat::Ndjson => {
            for line in text.lines() {
                serde_json::from_str::<BoundedJsonValue>(line)
                    .map_err(|_| ProviderFixtureError::Semantic("NDJSON"))?;
            }
        }
        RawFormat::Sse | RawFormat::AnthropicSse => {
            let mut data = 0;
            for line in text.lines().filter(|line| line.starts_with("data: ")) {
                if &line[6..] != "[DONE]" {
                    serde_json::from_str::<BoundedJsonValue>(&line[6..])
                        .map_err(|_| ProviderFixtureError::Semantic("SSE data"))?;
                }
                data += 1;
            }
            if data == 0 {
                serde_json::from_str::<BoundedJsonValue>(text)
                    .map_err(|_| ProviderFixtureError::Semantic("SSE or plain error body"))?;
            } else if format == RawFormat::AnthropicSse && !text.starts_with("event: ") {
                return Err(ProviderFixtureError::Semantic("SSE framing"));
            }
        }
    }
    Ok(())
}
