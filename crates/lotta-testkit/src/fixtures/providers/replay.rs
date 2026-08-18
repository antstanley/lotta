//! Sanitized provider-stream fixture corpus and replay support.

use super::types::{ProviderCase, REPLAY_CHANNEL_EVENTS_MAX, TRACE_EVENTS_MAX};
use crate::TestkitError;
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{ProviderEvent, ProviderPort, provider_event_channel};
use std::fmt;

/// Provider fixture loading or semantics failure.
#[derive(Debug, thiserror::Error)]
pub enum ProviderFixtureError {
    /// Confined loader failure.
    #[error("provider fixture load failed: {0}")]
    Fixture(#[from] TestkitError),
    /// Runtime bounded-construction failure.
    #[error("provider fixture conversion failed: {0}")]
    Runtime(#[from] RuntimeError),
    /// Domain bounded-construction failure.
    #[error("provider fixture conversion failed")]
    Domain(#[from] lotta_domain::DomainError),
    /// Bounded trace limit.
    #[error("provider fixture trace limit exceeded")]
    TraceLimit,
    /// Stable sanitized semantic context.
    #[error("provider fixture semantic failure: {0}")]
    Semantic(&'static str),
}

/// One side of a first-event divergence.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderReplaySide {
    /// Event was present.
    Event(ProviderEvent),
    /// Event was missing.
    Missing,
}
/// First divergent zero-based event index and sides.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderReplayDivergence {
    /// Zero-based index.
    pub index: usize,
    /// Expected event or missing marker.
    pub expected: ProviderReplaySide,
    /// Actual event or missing marker.
    pub actual: ProviderReplaySide,
}
impl fmt::Display for ProviderReplayDivergence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!(
            "provider replay divergence at event {}: expected {:?}, actual {:?}",
            self.index, self.expected, self.actual
        ))
    }
}
/// Public replay failure taxonomy.
#[derive(Debug, thiserror::Error)]
pub enum ProviderReplayError {
    /// Fixture loading/conversion failed.
    #[error(transparent)]
    Fixture(#[from] ProviderFixtureError),
    /// Provider returned a failure outside its event trace.
    #[error("provider replay provider failure: {0}")]
    Provider(Box<RuntimeError>),
    /// Bounded channel failed.
    #[error("provider replay channel failure: {0}")]
    Channel(Box<RuntimeError>),
    /// Actual trace exceeded the bound.
    #[error("provider replay trace limit exceeded")]
    TraceLimit,
    /// First expected/actual divergence.
    #[error("{0}")]
    Divergence(Box<ProviderReplayDivergence>),
}

/// Drives any provider port, concurrently drains bounded backpressure, and compares its trace.
///
/// # Errors
/// Returns distinct provider, channel, bound, fixture, or first-divergence failures.
pub async fn replay_provider(
    provider: &dyn ProviderPort,
    case: ProviderCase,
) -> Result<(), ProviderReplayError> {
    let cancellation = case.request.cancellation.clone();
    let (sink, mut receiver) = provider_event_channel(REPLAY_CHANNEL_EVENTS_MAX, &cancellation)
        .map_err(|error| ProviderReplayError::Channel(Box::new(error)))?;
    let producer = provider.stream(case.request, sink);
    let consumer = async {
        let mut events = Vec::new();
        while let Some(event) = receiver
            .receive()
            .await
            .map_err(|error| ProviderReplayError::Channel(Box::new(error)))?
        {
            if events.len() == TRACE_EVENTS_MAX {
                return Err(ProviderReplayError::TraceLimit);
            }
            events.push(event);
        }
        Ok(events)
    };
    let (producer_result, actual) = tokio::join!(producer, consumer);
    producer_result.map_err(|error| ProviderReplayError::Provider(Box::new(error)))?;
    compare_provider_traces(case.expected_trace.as_slice(), &actual?)
}
/// Compares two already-normalized traces and reports their first divergence.
///
/// # Errors
/// Returns the first differing or missing event.
pub fn compare_provider_traces(
    expected: &[ProviderEvent],
    actual: &[ProviderEvent],
) -> Result<(), ProviderReplayError> {
    let count = expected.len().max(actual.len());
    for index in 0..count {
        if expected.get(index) != actual.get(index) {
            return Err(ProviderReplayError::Divergence(Box::new(
                ProviderReplayDivergence {
                    index,
                    expected: expected
                        .get(index)
                        .cloned()
                        .map_or(ProviderReplaySide::Missing, ProviderReplaySide::Event),
                    actual: actual
                        .get(index)
                        .cloned()
                        .map_or(ProviderReplaySide::Missing, ProviderReplaySide::Event),
                },
            )));
        }
    }
    Ok(())
}
