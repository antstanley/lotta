pub(super) use super::*;
pub(super) use serde_json::{Value, json};
pub(super) use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

pub(super) fn handle(model: &str) -> ModelHandle {
    ModelHandle::new("openai", model).unwrap_or_else(|error| panic!("handle: {error}"))
}
pub(super) fn settings(value: &Value) -> ModelSettings {
    ModelSettings::normalize(value).unwrap_or_else(|error| panic!("settings: {error}"))
}
pub(super) fn level(model: Option<&str>, value: Option<&Value>) -> ModelOverride {
    ModelOverride {
        handle: model.map(handle),
        settings: value.map(settings),
    }
}

#[derive(Clone)]
pub(super) struct Availability(pub(super) Result<bool, AvailabilityError>);
impl ModelAvailability for Availability {
    fn is_available(&self, _: &ModelHandle) -> Result<bool, AvailabilityError> {
        self.0
    }
}
pub(super) struct Store {
    pub(super) state: Mutex<StoredModel>,
    pub(super) writes: AtomicUsize,
}
impl Store {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(StoredModel {
                handle: handle("prior"),
                settings: settings(&serde_json::json!({"prior":true})),
                revision: 7,
            }),
            writes: AtomicUsize::new(0),
        }
    }
}
impl ModelStore for Store {
    fn replace_model(
        &self,
        expected: u64,
        handle: ModelHandle,
        settings: ModelSettings,
    ) -> Result<StoredModel, ModelUpdateError> {
        let mut state = self.state.lock().map_err(|_| ModelUpdateError::Store)?;
        if state.revision != expected {
            return Err(ModelUpdateError::RevisionConflict);
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        *state = StoredModel {
            handle,
            settings,
            revision: expected + 1,
        };
        Ok(state.clone())
    }
}
