use super::super::*;
use super::harness::*;
use crate::boundary::ProviderImageBytes;
use crate::bounds::TOOL_ARGUMENT_BYTES_MAX;
use crate::ports::{ProviderEvent, ProviderUsage};
use lotta_domain::BoundedJsonValue;
use std::collections::BTreeMap;
use std::future::Future;
use std::task::{Context, Poll, Waker};
use tokio_util::sync::CancellationToken;

pub(crate) fn text_reasoning_arrival_order_preserved() {
    let source = vec![
        SourceEvent::Text("a".into()),
        SourceEvent::Reasoning("b".into()),
        SourceEvent::Redacted("hidden".into()),
        SourceEvent::Text("c".into()),
        SourceEvent::Stop,
    ];
    let expected: Vec<_> = source.iter().map(normalize_source).collect();
    for (fault, compliant) in [
        (FaultMode::None, true),
        (FaultMode::Reverse, false),
        (FaultMode::DropReasoning, false),
        (FaultMode::CollapseReasoning, false),
    ] {
        let output = run_port(
            &SourceProvider {
                source: source.clone(),
                fault,
            },
            request(ImagePolicy::Strict),
        )
        .unwrap();
        assert_eq!(output == expected, compliant);
    }
}

pub(crate) fn stable_tool_call_id_across_start_deltas_end() {
    assert!(run_validated(&source_provider(tool_source())).is_ok());
    for fault in [
        FaultMode::RewriteStartId,
        FaultMode::RewriteDeltaId,
        FaultMode::RewriteEndId,
        FaultMode::RewriteToolName,
        FaultMode::DuplicateStart,
        FaultMode::UnknownEnd,
        FaultMode::ReplayEndedDelta,
        FaultMode::ReplayEndedEnd,
    ] {
        let output = run_port(
            &faulty_source_provider(tool_source(), fault),
            request(ImagePolicy::Strict),
        )
        .unwrap();
        let rejected = validate_trace(&output).is_err();
        assert!(rejected || matches!(fault, FaultMode::RewriteToolName));
        if matches!(fault, FaultMode::RewriteToolName) {
            assert_ne!(
                output,
                run_port(
                    &source_provider(tool_source()),
                    request(ImagePolicy::Strict)
                )
                .unwrap()
            );
        }
    }
    let good = vec![
        SourceEvent::ToolStart("stable".into(), "tool".into()),
        SourceEvent::ToolArguments("stable".into(), b"{}".to_vec()),
        SourceEvent::ToolEnd("stable".into()),
        SourceEvent::Stop,
    ];
    assert!(run_validated(&source_provider(good)).is_ok());
    let bad = vec![
        SourceEvent::ToolStart("a".into(), "tool".into()),
        SourceEvent::ToolArguments("b".into(), b"{}".to_vec()),
        SourceEvent::Stop,
    ];
    assert!(run_validated(&source_provider(bad)).is_err());
}

pub(crate) fn partial_json_bounded_incrementally_and_parsed_only_at_end() {
    assert!(run_validated(&source_provider(tool_source())).is_ok());
    for fault in [
        FaultMode::MergeArguments,
        FaultMode::SplitArguments,
        FaultMode::CorruptArguments,
        FaultMode::OverflowArguments,
    ] {
        let output = run_port(
            &faulty_source_provider(tool_source(), fault),
            request(ImagePolicy::Strict),
        );
        match fault {
            FaultMode::MergeArguments | FaultMode::SplitArguments => assert!(output.is_ok()),
            _ => assert!(output.is_err() || validate_trace(&output.unwrap()).is_err()),
        }
    }
    let good = vec![
        SourceEvent::ToolStart("port-json".into(), "tool".into()),
        SourceEvent::ToolArguments("port-json".into(), b"{\"value\":\"\xc3".to_vec()),
        SourceEvent::ToolArguments("port-json".into(), b"\xa9\"}".to_vec()),
        SourceEvent::ToolEnd("port-json".into()),
        SourceEvent::Stop,
    ];
    assert!(run_validated(&source_provider(good)).is_ok());
    let bad = vec![
        SourceEvent::ToolStart("port-json".into(), "tool".into()),
        SourceEvent::ToolArguments("port-json".into(), b"{".to_vec()),
        SourceEvent::ToolEnd("port-json".into()),
        SourceEvent::Stop,
    ];
    assert!(run_validated(&source_provider(bad)).is_err());

    let id = call("json");
    let mut buffer = ToolArgumentBuffer::default();
    buffer.start(id.clone()).unwrap();
    buffer.append(&id, b"{\"value\":\"").unwrap();
    buffer.append(&id, br#"ok"}"#).unwrap();
    assert!(buffer.end(&id).is_ok());
    let split = call("split");
    let mut utf8 = ToolArgumentBuffer::default();
    utf8.start(split.clone()).unwrap();
    let json = "{\"value\":\"é\"}".as_bytes();
    let split_at = json.iter().position(|byte| *byte == 0xc3).unwrap() + 1;
    utf8.append(&split, &json[..split_at]).unwrap();
    utf8.append(&split, &json[split_at..]).unwrap();
    assert!(utf8.end(&split).is_ok());
    let mut malformed = ToolArgumentBuffer::default();
    malformed.start(id.clone()).unwrap();
    malformed.append(&id, b"{").unwrap();
    assert!(malformed.end(&id).is_err());
    let over_id = call("over-fresh");
    let mut fresh = ToolArgumentBuffer::default();
    fresh.start(over_id.clone()).unwrap();
    assert_eq!(fresh.active_capacity_for_test(&over_id), 0);
    assert!(
        fresh
            .append(&over_id, &vec![b'x'; TOOL_ARGUMENT_BYTES_MAX.value + 1])
            .is_err()
    );
    assert_eq!(fresh.active_capacity_for_test(&over_id), 0);

    let mut bounded = ToolArgumentBuffer::default();
    bounded.start(id.clone()).unwrap();
    bounded
        .append(&id, &vec![b'x'; TOOL_ARGUMENT_BYTES_MAX.value])
        .unwrap();
    let capacity = bounded.active_capacity_for_test(&id);
    assert!(bounded.append(&id, b"x").is_err());
    assert!(capacity >= TOOL_ARGUMENT_BYTES_MAX.value);
    assert_eq!(bounded.active_capacity_for_test(&id), 0);
}

pub(crate) fn usage_monotonic_and_final_snapshot_retained() {
    assert_eq!(
        run_validated(&source_provider(usage_source()))
            .unwrap()
            .usage,
        Some(ProviderUsage {
            input_tokens: 5,
            output_tokens: 5,
            cached_input_tokens: 3,
            reasoning_tokens: 3
        })
    );
    for fault in [
        FaultMode::DecreaseInput,
        FaultMode::DecreaseOutput,
        FaultMode::DecreaseCached,
        FaultMode::DecreaseReasoning,
        FaultMode::ImpossibleUsage,
        FaultMode::OverflowUsage,
    ] {
        assert!(run_validated(&faulty_source_provider(usage_source(), fault)).is_err());
    }
    assert_ne!(
        run_validated(&faulty_source_provider(
            usage_source(),
            FaultMode::DropFinalUsage
        ))
        .unwrap()
        .usage,
        Some(ProviderUsage {
            input_tokens: 5,
            output_tokens: 5,
            cached_input_tokens: 3,
            reasoning_tokens: 3
        })
    );
    let good = vec![
        SourceEvent::Usage(usage(1, 1)),
        SourceEvent::Usage(usage(2, 3)),
        SourceEvent::Stop,
    ];
    assert_eq!(
        run_validated(&source_provider(good)).unwrap().usage,
        Some(usage(2, 3))
    );
    let base = ProviderUsage {
        input_tokens: 4,
        output_tokens: 4,
        cached_input_tokens: 2,
        reasoning_tokens: 2,
    };
    for decreased in [
        ProviderUsage {
            input_tokens: 3,
            ..base
        },
        ProviderUsage {
            output_tokens: 3,
            ..base
        },
        ProviderUsage {
            cached_input_tokens: 1,
            ..base
        },
        ProviderUsage {
            reasoning_tokens: 1,
            ..base
        },
    ] {
        assert!(
            run_validated(&source_provider(vec![
                SourceEvent::Usage(base),
                SourceEvent::Usage(decreased),
                SourceEvent::Stop,
            ]))
            .is_err()
        );
    }
    assert!(
        ProviderUsage {
            input_tokens: u64::MAX,
            output_tokens: 1,
            cached_input_tokens: 0,
            reasoning_tokens: 0
        }
        .checked_total()
        .is_err()
    );
    assert!(
        ProviderUsage {
            input_tokens: 1,
            output_tokens: 1,
            cached_input_tokens: 2,
            reasoning_tokens: 0
        }
        .checked_total()
        .is_err()
    );
}

pub(crate) fn exactly_one_stop() {
    let source = vec![SourceEvent::Stop];
    assert!(run_validated(&source_provider(source.clone())).is_ok());
    for fault in [
        FaultMode::OmitStop,
        FaultMode::DuplicateStop,
        FaultMode::EmitAfterStop,
        FaultMode::EmitAfterError,
    ] {
        assert!(run_validated(&faulty_source_provider(source.clone(), fault)).is_err());
    }
    assert!(run_validated(&faulty_source_provider(source, FaultMode::EmitError)).is_ok());
}

pub(crate) fn cancellation_closes_and_suppresses_late_adapter_sends() {
    struct CancellationPort;
    impl ProviderPort for CancellationPort {
        fn stream(
            &self,
            request: ProviderRequest,
            events: ProviderEventSink,
        ) -> PortFuture<'_, ()> {
            Box::pin(async move {
                events
                    .send(ProviderEvent::TextDelta {
                        text: event_text("queued"),
                    })
                    .await?;
                request.cancellation.cancel();
                assert!(
                    events
                        .send(ProviderEvent::TextDelta {
                            text: event_text("late")
                        })
                        .await
                        .is_err()
                );
                Ok(())
            })
        }
    }
    let request = request(ImagePolicy::Strict);
    assert!(run_port_with_capacity(&CancellationPort, request, 1).is_err());

    let cancellation = CancellationToken::new();
    let (guarded, mut receiver) = provider_event_channel(1, &cancellation).unwrap();
    poll_ready(guarded.send(stop())).unwrap();
    let mut blocked = std::pin::pin!(guarded.send(stop()));
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(blocked.as_mut().poll(&mut context), Poll::Pending));
    cancellation.cancel();
    assert!(matches!(
        blocked.as_mut().poll(&mut context),
        Poll::Ready(Err(_))
    ));
    assert!(poll_ready(receiver.receive()).is_err());
}

#[allow(clippy::too_many_lines)]
pub(crate) fn strict_drop_image_policy_behavior() {
    use std::sync::{Arc, Mutex};
    struct ImagePort {
        mapped: Arc<Mutex<Vec<ProviderContentPart>>>,
        fault: FaultMode,
    }
    impl ProviderPort for ImagePort {
        fn stream(
            &self,
            request: ProviderRequest,
            events: ProviderEventSink,
        ) -> PortFuture<'_, ()> {
            let mapped = Arc::clone(&self.mapped);
            let fault = self.fault;
            Box::pin(async move {
                let mut parts: Vec<_> = request
                    .messages
                    .as_slice()
                    .iter()
                    .flat_map(|message| message.content.as_slice().iter().cloned())
                    .collect();
                match fault {
                    FaultMode::DropImageParts => {
                        parts.retain(|part| !matches!(part, ProviderContentPart::Image { .. }));
                    }
                    FaultMode::ReorderImageParts => parts.reverse(),
                    _ => {}
                }
                *mapped.lock().unwrap() = parts;
                events.send(stop()).await
            })
        }
    }
    let expected = vec![
        ProviderContentPart::Text(text("before")),
        ProviderContentPart::Image {
            media_type: name("image/png"),
            bytes: ProviderImageBytes::new(vec![1, 2]).unwrap(),
        },
        ProviderContentPart::Text(text("middle")),
        ProviderContentPart::Image {
            media_type: name("image/jpeg"),
            bytes: ProviderImageBytes::new(vec![3, 4]).unwrap(),
        },
    ];
    let mut image_request = request(ImagePolicy::Strict);
    image_request.messages = ProviderMessages::new(vec![ProviderMessage {
        role: ProviderMessageRole::User,
        content: ProviderContent::new(expected.clone()).unwrap(),
        tool_call_id: None,
    }])
    .unwrap();
    for fault in [
        FaultMode::None,
        FaultMode::DropImageParts,
        FaultMode::ReorderImageParts,
    ] {
        let mapped = Arc::new(Mutex::new(Vec::new()));
        let port = ImagePort {
            mapped: Arc::clone(&mapped),
            fault,
        };
        assert!(validate_trace(&run_port(&port, image_request.clone()).unwrap()).is_ok());
        assert_eq!(
            *mapped.lock().unwrap() == expected,
            matches!(fault, FaultMode::None)
        );
    }
    let compliant = source_provider(vec![
        SourceEvent::Text("before".into()),
        SourceEvent::Text("after".into()),
        SourceEvent::Stop,
    ]);
    let expected = run_port(&compliant, request(ImagePolicy::Strict)).unwrap();
    let faulty = SourceProvider {
        source: vec![
            SourceEvent::Text("before".into()),
            SourceEvent::Reasoning("image-marker".into()),
            SourceEvent::Text("after".into()),
            SourceEvent::Stop,
        ],
        fault: FaultMode::Reverse,
    };
    assert_ne!(
        run_port(&faulty, request(ImagePolicy::Drop)).unwrap(),
        expected
    );

    let content = ProviderContent::new(vec![
        ProviderContentPart::Text(text("kept")),
        ProviderContentPart::Image {
            media_type: name("image/png"),
            bytes: ProviderImageBytes::new(vec![1]).unwrap(),
        },
    ])
    .unwrap();
    assert!(
        request(ImagePolicy::Strict)
            .content_for_image_support(&content, false)
            .is_err()
    );
    let dropped = request(ImagePolicy::Drop)
        .content_for_image_support(&content, false)
        .unwrap();
    assert_eq!(
        dropped.as_slice(),
        &[ProviderContentPart::Text(text("kept"))]
    );
    assert_eq!(
        request(ImagePolicy::Strict)
            .content_for_image_support(&content, true)
            .unwrap(),
        content
    );
}

#[allow(clippy::too_many_lines)]
pub(crate) fn persisted_provider_metadata_rejects_secrets_retaining_continuation() {
    struct MetadataPort {
        fault: FaultMode,
    }
    impl ProviderPort for MetadataPort {
        fn stream(
            &self,
            _request: ProviderRequest,
            events: ProviderEventSink,
        ) -> PortFuture<'_, ()> {
            let fault = self.fault;
            Box::pin(async move {
                let secret = BoundedJsonValue::new(
                    serde_json::json!({"nested":{"api_token":"recognizable-secret"}}),
                )
                .unwrap();
                let input = match fault {
                    FaultMode::None => ProviderMetadataInput::Secret {
                        key: name("credential"),
                        value: secret,
                    },
                    FaultMode::MisclassifySecret => ProviderMetadataInput::Persist {
                        key: name("api_token"),
                        value: BoundedJsonValue::new(serde_json::json!("recognizable-secret"))
                            .unwrap(),
                    },
                    FaultMode::PersistNestedSecret => ProviderMetadataInput::Persist {
                        key: name("safe"),
                        value: secret,
                    },
                    _ => unreachable!(),
                };
                let metadata = ProviderMetadata::classified([input])?;
                events
                    .send(ProviderEvent::ProviderMetadata { metadata })
                    .await?;
                events.send(stop()).await
            })
        }
    }
    let compliant = run_port(
        &MetadataPort {
            fault: FaultMode::None,
        },
        request(ImagePolicy::Strict),
    )
    .unwrap();
    assert!(validate_trace(&compliant).is_ok());
    assert!(!format!("{compliant:?}").contains("recognizable-secret"));
    for fault in [FaultMode::MisclassifySecret, FaultMode::PersistNestedSecret] {
        assert!(run_port(&MetadataPort { fault }, request(ImagePolicy::Strict)).is_err());
    }
    let continuation = name("response_id");
    let mut good = BTreeMap::new();
    good.insert(
        continuation.clone(),
        BoundedJsonValue::new(serde_json::json!("resp_1")).unwrap(),
    );
    let metadata = ProviderMetadata::classified(
        good.into_iter()
            .map(|(key, value)| ProviderMetadataInput::Persist { key, value }),
    )
    .unwrap();
    assert_eq!(
        metadata.get(&continuation),
        Some(&serde_json::json!("resp_1"))
    );
    let secret = "recognizable-secret";
    let classified = ProviderMetadata::classified([ProviderMetadataInput::Secret {
        key: name("innocent"),
        value: BoundedJsonValue::new(serde_json::json!(secret)).unwrap(),
    }])
    .unwrap();
    assert!(classified.get(&name("innocent")).is_none());
    assert!(!format!("{classified:?}").contains(secret));
    let event = ProviderEvent::ProviderMetadata {
        metadata: classified.clone(),
    };
    assert!(!format!("{event:?}").contains(secret));
    let trace = run_port(
        &source_provider(vec![
            SourceEvent::Metadata(classified.clone()),
            SourceEvent::Stop,
        ]),
        request(ImagePolicy::Strict),
    )
    .unwrap();
    assert!(!format!("{trace:?}").contains(secret));
    assert!(validate_trace(&trace).is_ok());
    let faulty = SourceProvider {
        source: vec![SourceEvent::Text(secret.into()), SourceEvent::Stop],
        fault: FaultMode::Reverse,
    };
    assert!(validate_trace(&run_port(&faulty, request(ImagePolicy::Strict)).unwrap()).is_err());
    let mut deep = serde_json::json!({"api_token":"recognizable-secret"});
    for _ in 0..20 {
        deep = serde_json::json!({"safe":[deep]});
    }
    assert!(
        ProviderMetadata::classified([ProviderMetadataInput::Persist {
            key: name("safe"),
            value: BoundedJsonValue::new(deep).unwrap(),
        }])
        .is_err()
    );
    let mut bad = BTreeMap::new();
    bad.insert(
        name("api_token"),
        BoundedJsonValue::new(serde_json::json!("recognizable-secret")).unwrap(),
    );
    assert!(
        ProviderMetadata::classified(
            bad.into_iter()
                .map(|(key, value)| ProviderMetadataInput::Persist { key, value })
        )
        .is_err()
    );
}
