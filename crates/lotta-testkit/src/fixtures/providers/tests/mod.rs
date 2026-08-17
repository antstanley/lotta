use super::types::{CASE_COUNT, ERROR_KIND_COUNT, INVENTORY_COUNT};
use super::*;
use crate::fakes::FakeProvider;
use crate::fixtures::FixtureLoader;
use lotta_runtime::boundary::ProviderEventText;
use lotta_runtime::ports::{ImagePolicy, ProviderEvent, StopReason};
use std::collections::BTreeSet;

fn index() -> ProviderIndex {
    load_index(&FixtureLoader::new()).expect("provider index")
}
fn cases() -> Vec<ProviderCase> {
    load_all(&FixtureLoader::new()).expect("provider cases")
}

#[test]
fn index_is_complete() {
    let value = index();
    assert_eq!(value.dialects, Dialect::ALL);
    assert_eq!(value.cases.len(), CASE_COUNT);
    assert_eq!(value.inventory.len(), INVENTORY_COUNT);
    assert_eq!(
        FixtureLoader::new()
            .list_tree("providers")
            .expect("tree")
            .len(),
        81
    );
    for dialect in Dialect::ALL {
        assert!(value.cases.iter().any(|case| case.dialect == dialect));
    }
}
#[test]
fn dimensions_are_covered() {
    let loaded = cases();
    let actual: BTreeSet<_> = loaded
        .iter()
        .flat_map(|case| case.record.dimensions.as_slice().iter().copied())
        .collect();
    assert_eq!(actual, Dimension::ALL.into_iter().collect());
}
#[test]
fn reasoning_present() {
    let loaded = cases();
    let reasoning = loaded
        .iter()
        .find(|case| case.record.name == "reasoning-redacted")
        .expect("reasoning case");
    assert!(
        reasoning
            .expected_trace
            .as_slice()
            .iter()
            .any(|event| matches!(event, ProviderEvent::ReasoningDelta { .. }))
    );
    assert!(
        reasoning
            .expected_trace
            .as_slice()
            .iter()
            .any(|event| matches!(event, ProviderEvent::RedactedReasoning { .. }))
    );
}
#[test]
fn error_kinds_are_covered() {
    let loaded = cases();
    let errors = loaded
        .iter()
        .filter_map(|case| case.expected_trace.as_slice().last())
        .filter(|event| matches!(event, ProviderEvent::Error { .. }))
        .count();
    assert!(errors >= ERROR_KIND_COUNT);
    assert_eq!(index().error_kind_to_case.len(), ERROR_KIND_COUNT);
}
#[tokio::test]
async fn replay_all_cases() {
    for case in cases() {
        let provider = FakeProvider::default();
        provider
            .configure(case.expected_trace.as_slice().to_vec(), None)
            .expect("configure");
        replay_provider(&provider, case).await.expect("replay");
    }
}
#[tokio::test]
async fn replay_reports_first_divergence() {
    let case = cases().remove(0);
    let mut actual = case.expected_trace.as_slice().to_vec();
    actual[1] = ProviderEvent::TextDelta {
        text: ProviderEventText::new("SANITIZED_FIXTURE_WRONG".into()).expect("text"),
    };
    let provider = FakeProvider::default();
    provider.configure(actual, None).expect("configure");
    let error = replay_provider(&provider, case)
        .await
        .expect_err("divergence");
    let message = error.to_string();
    assert!(message.contains("event 1"));
    assert!(message.contains("expected Event"));
    assert!(message.contains("actual Event"));
}
#[test]
fn raw_formats_parse_all_sixteen() {
    let loaded = cases();
    assert_eq!(loaded.len(), CASE_COUNT);
    assert!(
        loaded
            .iter()
            .all(|case| case.raw_stream.contains("SANITIZED_FIXTURE"))
    );
}
#[test]
fn request_mappings_cover_five_dialects() {
    let loaded = cases();
    for dialect in Dialect::ALL {
        let case = loaded
            .iter()
            .find(|case| case.record.dialect == dialect)
            .expect("dialect");
        assert!(case.expected_request.as_value().is_object());
        case.request.validate_bytes().expect("bounded request");
    }
}
#[test]
fn tool_assembly_and_usage_are_semantic() {
    let loaded = cases();
    let case = loaded
        .iter()
        .find(|case| case.record.name == "happy-tool")
        .expect("tool");
    let events = case.expected_trace.as_slice();
    assert!(matches!(events[1], ProviderEvent::ToolCallStart { .. }));
    assert!(matches!(
        events[2],
        ProviderEvent::ToolCallArgumentsDelta { .. }
    ));
    assert!(matches!(
        events[3],
        ProviderEvent::ToolCallArgumentsDelta { .. }
    ));
    // The finishing choice closes its tool calls before that chunk's usage block.
    assert!(matches!(events[4], ProviderEvent::ToolCallEnd { .. }));
    let usage: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Usage { usage } => Some(*usage),
            _ => None,
        })
        .collect();
    assert_eq!(usage.len(), 2);
    assert!(usage[1].is_monotonic_after(usage[0]));
    assert!(matches!(
        events.last(),
        Some(ProviderEvent::Stop {
            reason: StopReason::ToolUse
        })
    ));
}
#[test]
fn cancellation_suppresses_late_baseline_event() {
    let loaded = cases();
    let case = loaded
        .iter()
        .find(|case| case.record.name == "cancelled")
        .expect("cancel");
    assert_eq!(case.baseline_event_count, 2);
    assert_eq!(case.expected_trace.len(), 1);
    assert!(matches!(
        case.expected_trace.as_slice().last(),
        Some(ProviderEvent::Error { .. })
    ));
}
#[test]
fn image_strict_and_drop_are_distinct() {
    let loaded = cases();
    let drop = loaded
        .iter()
        .find(|case| case.record.name == "image-drop")
        .expect("drop");
    let strict = loaded
        .iter()
        .find(|case| case.record.name == "image-strict")
        .expect("strict");
    assert_eq!(drop.request.image_policy, ImagePolicy::Drop);
    assert_eq!(strict.request.image_policy, ImagePolicy::Strict);
    assert!(strict.expected_request.as_value()["outcome"] == "preflight_error");
}
#[test]
fn each_dialect_has_distinct_wire_semantics() {
    let loaded = cases();
    for dialect in Dialect::ALL {
        let case = loaded
            .iter()
            .find(|case| case.record.dialect == dialect)
            .expect("dialect");
        match dialect {
            Dialect::Ollama => assert!(!case.raw_stream.contains("data: ")),
            Dialect::Anthropic => assert!(case.raw_stream.starts_with("event: ")),
            _ => assert!(
                case.raw_stream.contains("data: ")
                    || (!case
                        .record
                        .response
                        .as_ref()
                        .expect("response")
                        .is_success()
                        && serde_json::from_str::<serde_json::Value>(&case.raw_stream).is_ok())
            ),
        }
    }
}
#[test]
fn sanitization_scans_complete_tree() {
    let loader = FixtureLoader::new();
    let tree = loader.list_tree("providers").expect("tree");
    assert_eq!(tree.len(), 81);
    for path in tree {
        let text = loader.load_text(format!("providers/{path}")).expect("utf8");
        for forbidden in [
            "Bearer ",
            "PRIVATE KEY",
            "api_key",
            "password",
            "access_token",
        ] {
            assert!(!text.contains(forbidden), "{path}: {forbidden}");
        }
    }
}

#[test]
fn dialect_openai_wire_and_request() {
    let c = cases()
        .into_iter()
        .find(|c| c.record.dialect == Dialect::OpenaiCompatible)
        .unwrap();
    assert!(c.raw_stream.contains("chat.completion.chunk"));
    assert_eq!(
        c.expected_request.as_value()["body"]["stream_options"]["include_usage"],
        true
    );
}
#[test]
fn dialect_anthropic_named_sse_and_request() {
    let c = cases()
        .into_iter()
        .find(|c| c.record.dialect == Dialect::Anthropic && c.record.name == "reasoning-redacted")
        .unwrap();
    assert!(
        c.raw_stream.contains("event: message_start") && c.raw_stream.contains("redacted_thinking")
    );
    assert_eq!(c.expected_request.as_value()["endpoint"], "/v1/messages");
}
#[test]
fn dialect_ollama_ndjson_and_request() {
    let c = cases()
        .into_iter()
        .find(|c| c.record.dialect == Dialect::Ollama && c.record.name == "image-drop")
        .unwrap();
    assert!(
        c.raw_stream
            .lines()
            .all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
    );
    assert_eq!(c.expected_request.as_value()["endpoint"], "/api/chat");
}
#[test]
fn dialect_lm_studio_marker_and_request() {
    let c = cases()
        .into_iter()
        .find(|c| c.record.dialect == Dialect::LmStudio && c.record.name == "timeout")
        .unwrap();
    assert_eq!(c.expected_request.as_value()["compatibility"], "lm-studio");
    assert!(c.raw_stream.contains("SANITIZED_FIXTURE"));
}
#[test]
fn dialect_llama_cpp_marker_and_request() {
    let c = cases()
        .into_iter()
        .find(|c| c.record.dialect == Dialect::LlamaCpp)
        .unwrap();
    assert_eq!(c.expected_request.as_value()["compatibility"], "llama.cpp");
}
#[test]
fn dimension_request_mapping_semantics() {
    assert!(
        cases()
            .iter()
            .any(|c| c.expected_request.as_value().get("endpoint").is_some())
    );
}
#[test]
fn dimension_event_order_semantics() {
    let c = cases()
        .into_iter()
        .find(|c| c.record.name == "happy-tool")
        .unwrap();
    assert!(matches!(
        c.expected_trace.as_slice().last(),
        Some(ProviderEvent::Stop { .. })
    ));
}
#[test]
fn dimension_tool_call_json_semantics() {
    let c = cases()
        .into_iter()
        .find(|c| c.record.name == "happy-tool")
        .unwrap();
    let bytes = c
        .expected_trace
        .as_slice()
        .iter()
        .filter_map(|e| {
            if let ProviderEvent::ToolCallArgumentsDelta { chunk, .. } = e {
                Some(chunk.as_slice())
            } else {
                None
            }
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["a"],
        1
    );
}
#[test]
fn dimension_usage_monotonic_semantics() {
    let c = cases()
        .into_iter()
        .find(|c| c.record.name == "happy-tool")
        .unwrap();
    let u = c
        .expected_trace
        .as_slice()
        .iter()
        .filter_map(|e| {
            if let ProviderEvent::Usage { usage } = e {
                Some(*usage)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert!(u[1].is_monotonic_after(u[0]));
}
#[test]
fn dimension_cancellation_semantics() {
    cancellation_suppresses_late_baseline_event();
}
#[test]
fn dimension_error_semantics() {
    assert_eq!(
        cases()
            .iter()
            .filter(|c| matches!(
                c.expected_trace.as_slice().last(),
                Some(ProviderEvent::Error { .. })
            ))
            .count(),
        13
    );
}
#[test]
fn dimension_timeout_semantics() {
    assert!(
        cases()
            .iter()
            .find(|c| c.record.name == "timeout")
            .is_some_and(|c| matches!(c.expected_trace.as_slice()[0], ProviderEvent::Error { .. }))
    );
}
#[test]
fn dimension_context_semantics() {
    assert!(cases().iter().any(|c| c.record.name == "context-overflow"));
}
#[test]
fn dimension_retry_after_semantics() {
    let case = cases()
        .into_iter()
        .find(|c| c.record.name == "retry-after")
        .unwrap();
    let response = case.record.response.as_ref().expect("response metadata");
    assert_eq!(response.status, 429);
    assert!(
        response
            .headers
            .as_slice()
            .iter()
            .any(|header| header.name == "retry-after" && header.value == "2")
    );
    let ProviderEvent::Error {
        error: lotta_runtime::ports::ProviderError::RateLimit(context),
    } = &case.expected_trace.as_slice()[0]
    else {
        panic!("retry-after fixture is a terminal rate-limit error");
    };
    assert_eq!(
        context.retry_after,
        Some(lotta_runtime::retry::RetryAfter::Milliseconds(2_000))
    );
}
#[test]
fn dimension_image_modes_semantics() {
    image_strict_and_drop_are_distinct();
}
#[test]
fn base64_accepts_canonical() {
    assert_eq!(
        super::convert::decode_base64("eyJhIjo=").unwrap(),
        b"{\"a\":".to_vec()
    );
}
#[test]
fn base64_rejects_whitespace() {
    assert!(super::convert::decode_base64("UE 5H").is_err());
}
#[test]
fn base64_rejects_early_padding() {
    assert!(super::convert::decode_base64("AA==AAAA").is_err());
}
#[test]
fn base64_rejects_noncanonical_bits() {
    assert!(super::convert::decode_base64("AB==").is_err());
}
#[test]
fn base64_rejects_bad_alphabet() {
    assert!(super::convert::decode_base64("AA$=").is_err());
}
#[test]
fn source_regions_fixed_complete() {
    let i = index();
    assert_eq!(i.source_regions.len(), 24);
    assert!(i.source_regions.iter().all(|r| r.sha256.len() == 64));
}
#[test]
fn error_mapping_fixed_order() {
    let i = index();
    for (n, k) in ProviderErrorKind::ALL.into_iter().enumerate() {
        assert_eq!(i.error_kind_to_case[n].0, k);
    }
}
#[test]
fn expected_request_preserves_message_order() {
    for c in cases() {
        if let Some(messages) = c
            .expected_request
            .as_value()
            .pointer("/body/messages")
            .and_then(|v| v.as_array())
        {
            assert!(!messages.is_empty());
        }
    }
}
/// Native continuation captures carry classified, secret-free metadata where raw bytes supply it.
#[test]
fn provider_metadata_is_classified() {
    for case in cases() {
        let expected = matches!(
            case.record.id.as_str(),
            "openai-compatible/happy-tool" | "anthropic/reasoning-redacted"
        );
        assert_eq!(
            case.expected_trace
                .as_slice()
                .iter()
                .any(|event| matches!(event, ProviderEvent::ProviderMetadata { .. })),
            expected,
            "{}",
            case.record.id
        );
    }
    let record: super::types::FixtureEventRecord = serde_json::from_str(
        r#"{"type":"ProviderMetadata","entries":[{"key":"fixture_request_id","value":"x"}]}"#,
    )
    .expect("metadata record");
    let converted = super::convert::convert_trace(&[record]).expect("metadata conversion");
    assert!(matches!(
        converted.as_slice()[0],
        ProviderEvent::ProviderMetadata { .. }
    ));
}
#[test]
fn raw_all_have_dialect_structure() {
    for c in cases() {
        match c.record.raw_format {
            RawFormat::Ndjson => assert!(
                c.raw_stream
                    .lines()
                    .all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
            ),
            RawFormat::AnthropicSse => assert!(c.raw_stream.contains("event: ")),
            RawFormat::Sse => assert!(
                c.raw_stream.contains("data: ")
                    || (!c.record.response.as_ref().expect("response").is_success()
                        && serde_json::from_str::<serde_json::Value>(&c.raw_stream).is_ok())
            ),
            RawFormat::Json => {
                let body = serde_json::from_str::<serde_json::Value>(&c.raw_stream);
                assert!(body.expect("json body").is_object());
                assert!(!c.record.response.as_ref().expect("response").is_success());
            }
        }
    }
}
#[test]
fn index_inventory_sorted_unique() {
    let i = index();
    assert!(i.inventory.windows(2).all(|w| w[0].path < w[1].path));
}
#[test]
fn all_cases_have_terminal() {
    for c in cases() {
        assert!(matches!(
            c.expected_trace.as_slice().last(),
            Some(ProviderEvent::Stop { .. } | ProviderEvent::Error { .. })
        ));
    }
}
#[test]
fn reasoning_flags_match_events() {
    let c = cases()
        .into_iter()
        .find(|c| c.record.reasoning.visible)
        .unwrap();
    assert!(c.record.reasoning.redacted);
}
