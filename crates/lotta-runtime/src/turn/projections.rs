use super::test_support::*;
use super::*;
use crate::ports::{ProviderEvent, ProviderUsage, StopReason};
use serde::Deserialize;

async fn run_projection(script: Vec<ProviderEvent>) -> RecordingEffects {
    let (mut runtime, handle, lease) = runtime();
    let provider = ScriptedProvider::new(vec![script]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    run_turn(
        &mut runtime,
        handle,
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    effects
}

async fn assert_projection(event: ProviderEvent, expected: TurnProjection) {
    let mut script = vec![event];
    script.push(ProviderEvent::Stop {
        reason: StopReason::EndTurn,
    });
    let effects = run_projection(script).await;
    assert_eq!(*effects.projections.lock().unwrap(), vec![expected.clone()]);
    assert_eq!(
        *effects.events.lock().unwrap(),
        vec![
            TurnEvent::StreamDelta(expected),
            TurnEvent::Finished {
                reason: StopReason::EndTurn
            }
        ]
    );
}

#[tokio::test]
async fn text_delta_exact_projection_and_event() {
    assert_projection(
        ProviderEvent::TextDelta { text: text("text") },
        TurnProjection::new(ProjectionKind::Text, text("text")),
    )
    .await;
}

#[tokio::test]
async fn reasoning_delta_exact_projection_and_event() {
    assert_projection(
        ProviderEvent::ReasoningDelta {
            text: text("reason"),
        },
        TurnProjection::new(ProjectionKind::Reasoning, text("reason")),
    )
    .await;
}

#[tokio::test]
async fn redacted_delta_exact_projection_and_event() {
    assert_projection(
        ProviderEvent::RedactedReasoning {
            marker: text("<opaque>"),
        },
        TurnProjection::new(ProjectionKind::RedactedReasoning, text("<opaque>")),
    )
    .await;
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FixtureEvent {
    ThinkingDelta {
        text: String,
    },
    RedactedReasoning {
        marker: String,
    },
    TextDelta {
        text: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        reasoning_tokens: u64,
    },
    Done {
        reason: String,
    },
}

#[tokio::test]
async fn anthropic_reasoning_fixture_exact_replay() {
    let fixture = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/providers/anthropic/reasoning-redacted/baseline-events.jsonl"
    ));
    let mut events = Vec::new();
    for line in fixture.lines() {
        let event: FixtureEvent = serde_json::from_str(line).unwrap();
        events.push(match event {
            FixtureEvent::ThinkingDelta { text: value } => {
                ProviderEvent::ReasoningDelta { text: text(&value) }
            }
            FixtureEvent::RedactedReasoning { marker } => ProviderEvent::RedactedReasoning {
                marker: text(&marker),
            },
            FixtureEvent::TextDelta { text: value } => {
                ProviderEvent::TextDelta { text: text(&value) }
            }
            FixtureEvent::Usage {
                input_tokens,
                output_tokens,
                cached_input_tokens,
                reasoning_tokens,
            } => ProviderEvent::Usage {
                usage: ProviderUsage {
                    input_tokens,
                    output_tokens,
                    cached_input_tokens,
                    reasoning_tokens,
                },
            },
            FixtureEvent::Done { reason } => {
                assert_eq!(reason, "end_turn");
                ProviderEvent::Stop {
                    reason: StopReason::EndTurn,
                }
            }
        });
    }
    let effects = run_projection(events).await;
    let expected = vec![
        TurnProjection::new(
            ProjectionKind::Reasoning,
            text("SANITIZED_FIXTURE_REASONING"),
        ),
        TurnProjection::new(
            ProjectionKind::RedactedReasoning,
            text("<redacted-fixture>"),
        ),
        TurnProjection::new(ProjectionKind::Text, text("SANITIZED_FIXTURE_TEXT")),
    ];
    assert_eq!(*effects.projections.lock().unwrap(), expected);
    let mut expected_events: Vec<_> = expected.into_iter().map(TurnEvent::StreamDelta).collect();
    expected_events.push(TurnEvent::Finished {
        reason: StopReason::EndTurn,
    });
    assert_eq!(*effects.events.lock().unwrap(), expected_events);
}
