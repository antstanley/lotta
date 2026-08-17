//! Typed hook registry and command/model executors.

pub mod command;
pub mod events;
pub mod loader;
pub mod prompt;

use command::CommandHookExecutor;
use events::{HookContractError, HookFailure, HookOutcome, HookPayload, HookRuntime};
use loader::{HookConfig, HookRegistry, HookRegistrySnapshot};
use prompt::PromptHookExecutor;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Reusable production hook dispatcher over immutable registry snapshots.
pub struct RegisteredHookRuntime {
    registry: Arc<HookRegistry>,
    command: Arc<CommandHookExecutor>,
    prompt: Arc<PromptHookExecutor>,
}
impl RegisteredHookRuntime {
    /// Creates a runtime over shared registry and executors.
    #[must_use]
    pub fn new(
        registry: Arc<HookRegistry>,
        command: Arc<CommandHookExecutor>,
        prompt: Arc<PromptHookExecutor>,
    ) -> Self {
        Self {
            registry,
            command,
            prompt,
        }
    }
    /// Fires using an already captured immutable snapshot.
    pub async fn fire_snapshot(
        &self,
        snapshot: Arc<HookRegistrySnapshot>,
        mut payload: HookPayload,
        cancellation: CancellationToken,
    ) -> Result<HookOutcome, HookFailure> {
        let event = payload.event();
        let original = payload.clone();
        let mut modified = false;
        for registration in snapshot.event(event) {
            let tool = payload
                .value()
                .get("tool_name")
                .and_then(serde_json::Value::as_str);
            if !registration.matches(tool) {
                continue;
            }
            let outcome = match &registration.config {
                HookConfig::Command(config) => self
                    .command
                    .execute(config, &payload, cancellation.child_token())
                    .await
                    .map_err(|error| failure(registration, error_code(error)))?,
                HookConfig::Prompt(config) => self
                    .prompt
                    .execute(event, config, &payload, cancellation.child_token())
                    .await
                    .map_err(|error| failure(registration, prompt_error_code(error)))?,
            };
            match validate_outcome(&payload, outcome) {
                Ok(HookOutcome::Allow) => {}
                Ok(HookOutcome::Modify(replacement)) => {
                    payload = replacement;
                    modified = true;
                }
                Ok(blocked @ HookOutcome::Block(_)) => return Ok(blocked),
                Err(_) => return Err(failure(registration, "illegal_modification")),
            }
        }
        if modified && payload != original {
            Ok(HookOutcome::Modify(payload))
        } else {
            Ok(HookOutcome::Allow)
        }
    }
}
impl HookRuntime for RegisteredHookRuntime {
    fn fire(
        &self,
        payload: HookPayload,
        cancellation: CancellationToken,
    ) -> events::HookFuture<'_> {
        Box::pin(async move {
            let snapshot = self.registry.snapshot().map_err(|_| HookFailure {
                owner: events::HookOwner::new("runtime".into()).expect("constant owner"),
                hook_id: events::HookId::new("registry".into()).expect("constant hook id"),
                code: "hook_registry",
            })?;
            self.fire_snapshot(snapshot, payload, cancellation).await
        })
    }
}

pub(crate) fn validate_outcome(
    current: &HookPayload,
    outcome: HookOutcome,
) -> Result<HookOutcome, HookContractError> {
    let event = current.event();
    let HookOutcome::Modify(payload) = &outcome else {
        return Ok(outcome);
    };
    if !event.permits_modification() || payload.event() != event {
        return Err(HookContractError::Modification);
    }
    let permitted = match event {
        events::HookEvent::PreToolUse => "tool_input",
        events::HookEvent::PostToolUse => "tool_result",
        _ => return Err(HookContractError::Modification),
    };
    if payload.value().get(permitted).is_none() {
        return Err(HookContractError::Modification);
    }
    let mut expected = current.value().clone();
    let expected = expected
        .as_object_mut()
        .ok_or(HookContractError::Modification)?;
    let replacement = payload
        .value()
        .as_object()
        .ok_or(HookContractError::Modification)?;
    expected.insert(
        permitted.into(),
        replacement
            .get(permitted)
            .cloned()
            .ok_or(HookContractError::Modification)?,
    );
    if payload.value().as_object() != Some(expected) {
        return Err(HookContractError::Modification);
    }
    Ok(outcome)
}
pub(crate) fn failure(registration: &loader::HookRegistration, code: &'static str) -> HookFailure {
    HookFailure {
        owner: registration.owner.clone(),
        hook_id: registration.hook_id.clone(),
        code,
    }
}
fn error_code(_: command::CommandHookError) -> &'static str {
    "command_hook_failed"
}
fn prompt_error_code(_: prompt::PromptHookError) -> &'static str {
    "prompt_hook_failed"
}

#[cfg(test)]
#[path = "bounds_certificate.rs"]
mod bounds;
#[cfg(test)]
#[path = "command_is_sandboxed_certificate.rs"]
mod command_is_sandboxed;
#[cfg(test)]
#[path = "outcomes_certificate.rs"]
mod outcomes;
#[cfg(test)]
mod pipeline_certificate_support;
#[cfg(test)]
#[path = "prompt_subset_certificate.rs"]
mod prompt_subset;
#[cfg(test)]
mod test_support;
#[cfg(test)]
#[path = "timeouts_certificate.rs"]
mod timeouts;
