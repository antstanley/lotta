//! Immutable hook registration snapshots and atomic owner refreshes.

use super::{
    command::CommandHookConfig,
    events::{HookEvent, HookId, HookOwner},
    prompt::PromptHookConfig,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

/// Maximum hooks registered for one event.
pub const HOOKS_PER_EVENT_MAX: usize = 64;
/// Maximum matcher bytes.
pub const HOOK_MATCHER_BYTES_MAX: usize = 4_096;

/// A validated command or prompt hook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HookConfig {
    /// Sandboxed command hook.
    Command(CommandHookConfig),
    /// Model-backed prompt hook.
    Prompt(PromptHookConfig),
}

/// One owner-scoped hook registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookRegistration {
    /// Registration owner.
    pub owner: HookOwner,
    /// Stable hook identifier.
    pub hook_id: HookId,
    /// Event fired by this registration.
    pub event: HookEvent,
    /// Optional exact/piped tool matcher for tool events.
    matcher: Option<String>,
    /// Executable configuration.
    pub config: HookConfig,
}

impl HookRegistration {
    /// Validates event/config compatibility and matcher bounds.
    pub fn new(
        owner: HookOwner,
        hook_id: HookId,
        event: HookEvent,
        matcher: Option<String>,
        config: HookConfig,
    ) -> Result<Self, HookLoadError> {
        if matches!(config, HookConfig::Prompt(_)) && !event.supports_prompt() {
            return Err(HookLoadError::PromptUnsupported);
        }
        if matcher
            .as_ref()
            .is_some_and(|value| value.len() > HOOK_MATCHER_BYTES_MAX || value.contains('\0'))
        {
            return Err(HookLoadError::Matcher);
        }
        Ok(Self {
            owner,
            hook_id,
            event,
            matcher,
            config,
        })
    }
    /// Whether this hook matches the optional tool name.
    #[must_use]
    pub fn matches(&self, tool_name: Option<&str>) -> bool {
        let Some(pattern) = self.matcher.as_deref() else {
            return true;
        };
        let Some(tool_name) = tool_name else {
            return false;
        };
        pattern.is_empty() || pattern == "*" || pattern.split('|').any(|name| name == tool_name)
    }
}

/// Immutable per-event registration snapshot.
#[derive(Clone, Debug, Default)]
pub struct HookRegistrySnapshot {
    by_event: BTreeMap<HookEvent, Arc<[HookRegistration]>>,
}
impl HookRegistrySnapshot {
    /// Borrows hooks for one event in load order.
    #[must_use]
    pub fn event(&self, event: HookEvent) -> &[HookRegistration] {
        self.by_event.get(&event).map_or(&[], AsRef::as_ref)
    }
}

/// Atomic owner-refresh registry.
pub struct HookRegistry {
    snapshot: RwLock<Arc<HookRegistrySnapshot>>,
}
impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}
impl HookRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            snapshot: RwLock::new(Arc::new(HookRegistrySnapshot::default())),
        }
    }
    /// Captures one immutable registry snapshot.
    pub fn snapshot(&self) -> Result<Arc<HookRegistrySnapshot>, HookLoadError> {
        self.snapshot
            .read()
            .map(|value| Arc::clone(&value))
            .map_err(|_| HookLoadError::Registry)
    }
    /// Atomically replaces all registrations owned by `owner`.
    pub fn replace_owner(
        &self,
        owner: &HookOwner,
        registrations: Vec<HookRegistration>,
    ) -> Result<Arc<HookRegistrySnapshot>, HookLoadError> {
        if registrations
            .iter()
            .any(|registration| &registration.owner != owner)
        {
            return Err(HookLoadError::OwnerMismatch);
        }
        let mut guard = self.snapshot.write().map_err(|_| HookLoadError::Registry)?;
        let next = Arc::new(build_replacement(&guard, owner, registrations)?);
        *guard = Arc::clone(&next);
        Ok(next)
    }
    /// Atomically removes one owner's registrations while preserving every other owner.
    pub fn remove_owner(
        &self,
        owner: &HookOwner,
    ) -> Result<Arc<HookRegistrySnapshot>, HookLoadError> {
        self.replace_owner(owner, Vec::new())
    }
}

fn build_replacement(
    current: &HookRegistrySnapshot,
    owner: &HookOwner,
    registrations: Vec<HookRegistration>,
) -> Result<HookRegistrySnapshot, HookLoadError> {
    let mut by_event = BTreeMap::new();
    for event in HookEvent::ALL {
        let mut merged = Vec::new();
        merged.extend(
            current
                .event(event)
                .iter()
                .filter(|item| &item.owner != owner)
                .cloned(),
        );
        merged.extend(
            registrations
                .iter()
                .filter(|item| item.event == event)
                .cloned(),
        );
        if merged.len() > HOOKS_PER_EVENT_MAX {
            return Err(HookLoadError::EventLimit);
        }
        if !merged.is_empty() {
            by_event.insert(event, Arc::from(merged));
        }
    }
    Ok(HookRegistrySnapshot { by_event })
}

/// Stable hook loading failure without external values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum HookLoadError {
    /// Prompt hooks do not support the event.
    #[error("prompt hook unsupported for event")]
    PromptUnsupported,
    /// Matcher is malformed or overbound.
    #[error("invalid hook matcher")]
    Matcher,
    /// Replacement contains another owner.
    #[error("hook owner mismatch")]
    OwnerMismatch,
    /// Per-event registration ceiling was exceeded.
    #[error("hook event registration limit exceeded")]
    EventLimit,
    /// Registry lock is unavailable.
    #[error("hook registry unavailable")]
    Registry,
}
