use super::{ConnectionError, DisconnectProviderInput, ProviderAuthStore};
use lotta_runtime::{ListenerRuntime, RuntimeHandle};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;

/// Cancellation acknowledgment boundary for one active provider turn.
pub trait TurnCancellation: Send + Sync {
    /// Requests cancellation and waits until the existing Task47 lifecycle settles.
    fn cancel_and_wait<'a>(
        &'a self,
        runtime: &'a mut ListenerRuntime,
        handle: &'a RuntimeHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConnectionError>> + Send + 'a>>;
}

/// Provider-to-runtime index backed by Task47 runtime handles, not a second lease registry.
#[derive(Default)]
pub struct ActiveTurnRegistry {
    by_provider: HashMap<String, Vec<RuntimeHandle>>,
}

impl ActiveTurnRegistry {
    /// Associates a current Task47 runtime handle with the provider selected for its turn.
    pub fn attach(&mut self, provider_id: String, handle: RuntimeHandle) {
        self.by_provider
            .entry(provider_id)
            .or_default()
            .push(handle);
    }

    /// Removes a settled Task47 runtime handle from one provider association.
    pub fn detach(&mut self, provider_id: &str, handle: &RuntimeHandle) {
        if let Some(handles) = self.by_provider.get_mut(provider_id) {
            handles.retain(|candidate| candidate != handle);
            if handles.is_empty() {
                self.by_provider.remove(provider_id);
            }
        }
    }

    pub(crate) fn active<'a>(
        &'a self,
        provider_id: &str,
        runtime: &ListenerRuntime,
    ) -> Vec<&'a RuntimeHandle> {
        self.by_provider
            .get(provider_id)
            .into_iter()
            .flatten()
            .filter(|handle| {
                runtime
                    .lifecycle(handle)
                    .is_some_and(|owner| owner.projection().is_processing())
            })
            .collect()
    }

    pub(crate) fn handles(&self, provider_id: &str) -> Vec<RuntimeHandle> {
        self.by_provider
            .get(provider_id)
            .cloned()
            .unwrap_or_default()
    }
}

pub(crate) async fn disconnect<C: TurnCancellation>(
    store: &ProviderAuthStore,
    records: &mut BTreeMap<String, super::ProviderRecord>,
    revision: &mut u64,
    input: DisconnectProviderInput,
    turns: &mut ActiveTurnRegistry,
    runtime: &mut ListenerRuntime,
    cancellation: &C,
) -> Result<(), ConnectionError> {
    if *revision != input.expected_revision {
        return Err(ConnectionError::Conflict);
    }
    let name = locate(records, &input)?;
    let active = turns.active(&input.provider_id, runtime);
    if !active.is_empty() && !input.force {
        tracing::warn!(
            provider_id = input.provider_id,
            active_turns = active.len(),
            "provider disconnect refused"
        );
        return Err(ConnectionError::ActiveTurns);
    }
    if input.force {
        for handle in turns.handles(&input.provider_id) {
            if runtime
                .lifecycle(&handle)
                .is_some_and(|owner| owner.projection().is_processing())
            {
                tracing::info!(
                    provider_id = input.provider_id,
                    marker = "provider.turn.cancel.request",
                    "forced provider disconnect"
                );
                cancellation.cancel_and_wait(runtime, &handle).await?;
                if runtime
                    .lifecycle(&handle)
                    .is_some_and(|owner| owner.projection().is_processing())
                {
                    return Err(ConnectionError::Cancellation);
                }
                turns.detach(&input.provider_id, &handle);
                tracing::info!(
                    provider_id = input.provider_id,
                    marker = "provider.turn.cancel.ack",
                    "forced provider disconnect"
                );
            }
        }
    }
    let removed = records.remove(&name).ok_or(ConnectionError::NotFound)?;
    match store.replace(records, *revision) {
        Ok(next) => {
            *revision = next;
            tracing::info!(
                provider_id = input.provider_id,
                marker = "provider.connection.removed",
                "provider disconnected"
            );
            Ok(())
        }
        Err(error) => {
            records.insert(name, removed);
            Err(error)
        }
    }
}

fn locate(
    records: &BTreeMap<String, super::ProviderRecord>,
    input: &DisconnectProviderInput,
) -> Result<String, ConnectionError> {
    if let Some(name) = &input.provider_name {
        return records
            .get(name)
            .filter(|record| record.id == input.provider_id)
            .map(|_| name.clone())
            .ok_or(ConnectionError::NotFound);
    }
    records
        .iter()
        .find(|(_, record)| record.id == input.provider_id)
        .map(|(name, _)| name.clone())
        .ok_or(ConnectionError::NotFound)
}
