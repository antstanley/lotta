//! Configurable normalized provider fake.

use super::{cancelled, limit, lock};
use crate::TESTKIT_ITEMS_MAX;
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{ProviderEvent, ProviderEventSink, ProviderPort, ProviderRequest};
use std::sync::Mutex;

#[derive(Clone, Debug, Default)]
struct Script {
    events: Vec<ProviderEvent>,
    terminal: Option<RuntimeError>,
}

/// In-memory provider that emits bounded normalized scripts through the opaque sink.
#[derive(Debug, Default)]
pub struct FakeProvider {
    script: Mutex<Script>,
}

impl FakeProvider {
    /// Replaces the deterministic script and optional post-script port failure.
    ///
    /// # Errors
    /// Rejects scripts above the focused testkit event bound.
    pub fn configure(
        &self,
        events: Vec<ProviderEvent>,
        terminal: Option<RuntimeError>,
    ) -> Result<(), RuntimeError> {
        if events.len() > TESTKIT_ITEMS_MAX {
            return Err(limit("fake_provider_events_max"));
        }
        let terminal_positions: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                matches!(
                    event,
                    ProviderEvent::Stop { .. } | ProviderEvent::Error { .. }
                )
                .then_some(index)
            })
            .collect();
        if terminal_positions.len() > 1
            || terminal_positions
                .first()
                .is_some_and(|index| *index + 1 != events.len())
            || (terminal.is_some() && !terminal_positions.is_empty())
        {
            return Err(RuntimeError::InvalidData {
                context: "fake provider terminal script".into(),
            });
        }
        *lock(&self.script) = Script { events, terminal };
        Ok(())
    }
}

impl ProviderPort for FakeProvider {
    fn stream(
        &self,
        request: ProviderRequest,
        events: ProviderEventSink,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let script = lock(&self.script).clone();
        Box::pin(async move {
            for event in script.events {
                if request.cancellation.is_cancelled() || events.is_cancelled() {
                    return Err(cancelled("fake provider"));
                }
                tokio::select! {
                    biased;
                    () = request.cancellation.cancelled() => return Err(cancelled("fake provider")),
                    result = events.send(event) => result?,
                }
            }
            if request.cancellation.is_cancelled() || events.is_cancelled() {
                return Err(cancelled("fake provider"));
            }
            match script.terminal {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
    }
}
