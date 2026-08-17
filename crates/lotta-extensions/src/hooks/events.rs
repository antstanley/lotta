//! Pinned hook event aliases and typed payload constructors.

pub use lotta_runtime::hooks::{
    HOOK_ID_BYTES_MAX, HOOK_PAYLOAD_BYTES_MAX, HOOK_REASON_BYTES_MAX, HookContractError, HookEvent,
    HookFailure, HookFireResult, HookFuture, HookId, HookLifecycle, HookOutcome, HookOwner,
    HookPayload, HookReason, HookRuntime, NoopHookRuntime,
};

use serde_json::{Map, Value};

/// Creates a typed event payload after injecting the exact pinned discriminator.
pub fn payload(
    event: HookEvent,
    mut fields: Map<String, Value>,
) -> Result<HookPayload, HookContractError> {
    let wire = serde_json::to_value(event).map_err(|_| HookContractError::Payload)?;
    fields.insert("event_type".into(), wire);
    HookPayload::new(event, Value::Object(fields))
}

/// Creates an empty-field lifecycle payload for the event.
pub fn lifecycle_payload(event: HookEvent) -> Result<HookPayload, HookContractError> {
    payload(event, Map::new())
}

#[cfg(test)]
#[path = "events_certificate.rs"]
mod certificate;
