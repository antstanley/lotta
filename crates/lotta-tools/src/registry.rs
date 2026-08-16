//! Atomic bounded registry snapshots.

use crate::{allowlist::ToolAllowlist, names, toolset::ToolsetId};
use lotta_runtime::ports::{ModelFacingToolName, ToolDefinition, ToolPort};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, RwLock},
};

/// Canonical definition paired directly with its one executor.
#[derive(Clone)]
pub struct ToolRegistration {
    /// Shared definition whose internal name identifies the implementation.
    pub definition: Arc<ToolDefinition>,
    /// Shared executor used by every protocol alias.
    pub executor: Arc<dyn ToolPort>,
}

impl ToolRegistration {
    /// Replaces the executor while retaining the definition.
    #[must_use]
    pub fn with_executor(mut self, executor: Arc<dyn ToolPort>) -> Self {
        self.executor = executor;
        self
    }
}

/// One exposed tool in an immutable snapshot.
#[derive(Clone)]
pub struct RegisteredTool {
    /// Canonical shared definition.
    pub definition: Arc<ToolDefinition>,
    /// Validated model-facing name resolved for this snapshot.
    pub model_name: ModelFacingToolName,
    /// Shared executor.
    pub executor: Arc<dyn ToolPort>,
}

/// Typed registry construction or synchronization error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryError {
    /// A synchronization primitive was poisoned.
    Poisoned,
    /// A selected built-in implementation was not registered.
    MissingBuiltin(String),
    /// Two candidates had one canonical internal name.
    DuplicateInternal(String),
    /// Two candidates exposed one model-facing name.
    DuplicateModel(String),
    /// Candidate count exceeded [`TOOLS_LOADED_MAX`].
    TooManyTools,
    /// Allocation for a bounded candidate failed.
    Allocation,
    /// A resolved model name failed runtime bounds.
    InvalidModelName,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "tool registry error: {self:?}")
    }
}
impl std::error::Error for RegistryError {}

/// Immutable complete registry snapshot.
pub struct RegistrySnapshot {
    registrations: BTreeMap<String, Arc<RegisteredTool>>,
    model_to_internal: BTreeMap<String, String>,
}

impl RegistrySnapshot {
    /// Number of exposed registrations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.registrations.len()
    }
    /// Whether no tools are exposed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }
    /// Looks up by canonical internal registration name.
    #[must_use]
    pub fn by_internal(&self, name: &str) -> Option<&Arc<RegisteredTool>> {
        self.registrations.get(name)
    }
    /// Looks up by exposed model-facing name.
    #[must_use]
    pub fn by_model(&self, name: &str) -> Option<&Arc<RegisteredTool>> {
        self.model_to_internal
            .get(name)
            .and_then(|internal| self.registrations.get(internal))
    }
    /// Returns exposed names in stable order.
    #[must_use]
    pub fn model_names(&self) -> Vec<&str> {
        self.model_to_internal.keys().map(String::as_str).collect()
    }
}

/// Registry whose readers acquire one immutable snapshot atomically.
pub struct ToolRegistry {
    builtins: BTreeMap<String, ToolRegistration>,
    current: RwLock<Arc<RegistrySnapshot>>,
}

impl ToolRegistry {
    /// Validates canonical built-ins and creates an empty snapshot.
    ///
    /// # Errors
    /// Rejects duplicate names or more than [`TOOLS_LOADED_MAX`] registrations.
    pub fn new(
        builtins: impl IntoIterator<Item = ToolRegistration>,
    ) -> Result<Self, RegistryError> {
        let mut indexed = BTreeMap::new();
        for registration in builtins {
            if indexed.len() == TOOLS_LOADED_MAX {
                return Err(RegistryError::TooManyTools);
            }
            let internal = registration.definition.internal_name.as_str().to_owned();
            if indexed.insert(internal.clone(), registration).is_some() {
                return Err(RegistryError::DuplicateInternal(internal));
            }
        }
        Ok(Self {
            builtins: indexed,
            current: RwLock::new(Arc::new(empty_snapshot())),
        })
    }

    /// Clones one complete immutable snapshot under a read lock.
    ///
    /// # Errors
    /// Returns [`RegistryError::Poisoned`] if the read lock was poisoned.
    pub fn snapshot(&self) -> Result<Arc<RegistrySnapshot>, RegistryError> {
        self.current
            .read()
            .map(|value| Arc::clone(&value))
            .map_err(|_| RegistryError::Poisoned)
    }

    /// Builds and validates a complete candidate off-side, then performs one assignment.
    ///
    /// # Errors
    /// Rejects missing built-ins, duplicates, over-limit candidates, invalid names, allocation
    /// failure, or poisoned synchronization without modifying the current snapshot.
    pub fn update(
        &self,
        toolset: ToolsetId,
        external: &[ToolRegistration],
        allowlist: Option<&[&str]>,
    ) -> Result<Arc<RegistrySnapshot>, RegistryError> {
        let candidate = Arc::new(self.build(toolset, external, allowlist)?);
        let mut guard = self.current.write().map_err(|_| RegistryError::Poisoned)?;
        *guard = Arc::clone(&candidate);
        Ok(candidate)
    }

    fn build(
        &self,
        toolset: ToolsetId,
        external: &[ToolRegistration],
        allowlist: Option<&[&str]>,
    ) -> Result<RegistrySnapshot, RegistryError> {
        let filter = ToolAllowlist::new(allowlist);
        let builtin_count = names::rows()
            .iter()
            .filter(|row| row.toolset == toolset)
            .filter(|row| filter.permits(row.internal, row.model))
            .count();
        let external_count = external
            .iter()
            .filter(|item| {
                filter.permits(
                    item.definition.internal_name.as_str(),
                    item.definition.model_name.as_str(),
                )
            })
            .count();
        let count = builtin_count
            .checked_add(external_count)
            .ok_or(RegistryError::TooManyTools)?;
        if count > TOOLS_LOADED_MAX {
            return Err(RegistryError::TooManyTools);
        }
        let mut selected = Vec::new();
        selected
            .try_reserve_exact(count)
            .map_err(|_| RegistryError::Allocation)?;
        self.select_builtins(toolset, &filter, &mut selected)?;
        for registration in external {
            let internal = registration.definition.internal_name.as_str();
            let model = registration.definition.model_name.as_str();
            if filter.permits(internal, model) {
                selected.push(resolve_name(registration, model)?);
            }
        }
        build_snapshot(selected)
    }

    fn select_builtins(
        &self,
        toolset: ToolsetId,
        filter: &ToolAllowlist<'_>,
        selected: &mut Vec<RegisteredTool>,
    ) -> Result<(), RegistryError> {
        for row in names::rows().iter().filter(|row| row.toolset == toolset) {
            if !filter.permits(row.internal, row.model) {
                continue;
            }
            let registration = self
                .builtins
                .get(row.internal)
                .ok_or_else(|| RegistryError::MissingBuiltin(row.internal.into()))?;
            selected.push(resolve_name(registration, row.model)?);
        }
        Ok(())
    }
}

fn resolve_name(
    registration: &ToolRegistration,
    model: &str,
) -> Result<RegisteredTool, RegistryError> {
    let model_name =
        ModelFacingToolName::new(model.to_owned()).map_err(|_| RegistryError::InvalidModelName)?;
    Ok(RegisteredTool {
        definition: Arc::clone(&registration.definition),
        model_name,
        executor: Arc::clone(&registration.executor),
    })
}

fn build_snapshot(selected: Vec<RegisteredTool>) -> Result<RegistrySnapshot, RegistryError> {
    if selected.len() > TOOLS_LOADED_MAX {
        return Err(RegistryError::TooManyTools);
    }
    let mut registrations = BTreeMap::new();
    let mut model_to_internal = BTreeMap::new();
    for registration in selected {
        let internal = registration.definition.internal_name.as_str().to_owned();
        let model = registration.model_name.as_str().to_owned();
        if registrations.contains_key(&internal) {
            return Err(RegistryError::DuplicateInternal(internal));
        }
        if model_to_internal
            .insert(model.clone(), internal.clone())
            .is_some()
        {
            return Err(RegistryError::DuplicateModel(model));
        }
        registrations.insert(internal, Arc::new(registration));
    }
    Ok(RegistrySnapshot {
        registrations,
        model_to_internal,
    })
}

fn empty_snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        registrations: BTreeMap::new(),
        model_to_internal: BTreeMap::new(),
    }
}

/// Maximum complete candidate registry size, in tools.
pub const TOOLS_LOADED_MAX: usize = 1_024;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) mod support {
        use super::*;
        use lotta_runtime::{
            RuntimeError,
            bounds::{
                EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX,
            },
            ports::*,
        };
        use std::{future, time::Duration};
        pub(crate) struct Stub;
        impl ToolPort for Stub {
            fn execute(&self, _: ToolExecutionRequest) -> PortFuture<'_, ToolOutcome> {
                Box::pin(future::pending::<Result<ToolOutcome, RuntimeError>>())
            }
        }
        pub(crate) fn registration(internal: &str, model: &str) -> ToolRegistration {
            let schema = serde_json::from_str::<ToolInputSchema>("{\"type\":\"object\"}").unwrap();
            let secrets = serde_json::from_str::<SecretRedactionSpec>(
                "{\"fields\":[],\"policy\":\"redact\"}",
            )
            .unwrap();
            let definition = ToolDefinition::new(
                InternalToolName::new(internal.into()).unwrap(),
                ModelFacingToolName::new(model.into()).unwrap(),
                schema,
                ToolDescriptionAsset::new(String::new()).unwrap(),
                ToolExecutionOwner::Rust,
                ToolApprovalPolicy::Never,
                PermissionAction::new("execute".into()).unwrap(),
                ToolTimeout::new(Duration::from_millis(
                    EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u64,
                ))
                .unwrap(),
                ToolOutputLimit::new(
                    TOOL_RESULT_BYTES_MAX.value,
                    TOOL_RESULT_MODEL_CHARS_MAX.value,
                )
                .unwrap(),
                secrets,
            );
            ToolRegistration {
                definition: Arc::new(definition),
                executor: Arc::new(Stub),
            }
        }
        pub(crate) fn registry_with<const N: usize>(items: [ToolRegistration; N]) -> ToolRegistry {
            ToolRegistry::new(items).unwrap()
        }
    }

    pub(crate) mod atomic_swap {
        use super::support::*;
        use super::*;
        use std::sync::{
            Arc, Barrier,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };

        #[test]
        fn failed_update_leaves_previous() {
            let registry = registry_with([registration("Read", "Read")]);
            let old = registry
                .update(ToolsetId::Default, &[], Some(&["Read"]))
                .unwrap();

            let duplicate_internal = [registration("Read", "external")];
            assert!(matches!(
                registry.update(
                    ToolsetId::Default,
                    &duplicate_internal,
                    Some(&["Read", "external"]),
                ),
                Err(RegistryError::DuplicateInternal(name)) if name == "Read"
            ));
            assert!(Arc::ptr_eq(&old, &registry.snapshot().unwrap()));

            let duplicate_model = [registration("external", "Read")];
            assert!(matches!(
                registry.update(
                    ToolsetId::Default,
                    &duplicate_model,
                    Some(&["Read", "external"]),
                ),
                Err(RegistryError::DuplicateModel(name)) if name == "Read"
            ));
            assert!(Arc::ptr_eq(&old, &registry.snapshot().unwrap()));

            assert!(matches!(
                registry.update(
                    ToolsetId::None,
                    &external_tools(TOOLS_LOADED_MAX + 1, "above"),
                    None,
                ),
                Err(RegistryError::TooManyTools)
            ));
            assert!(Arc::ptr_eq(&old, &registry.snapshot().unwrap()));

            let missing = registry_with([]);
            let before = missing.snapshot().unwrap();
            let first_expected = names::rows()
                .iter()
                .find(|row| row.toolset == ToolsetId::Default)
                .unwrap()
                .internal;
            assert!(matches!(
                missing.update(ToolsetId::Default, &[], None),
                Err(RegistryError::MissingBuiltin(name)) if name == first_expected
            ));
            assert!(Arc::ptr_eq(&before, &missing.snapshot().unwrap()));
        }

        #[test]
        fn reader_never_sees_partial() {
            let registry = Arc::new(registry_with([]));
            let old_tools = external_tools(10, "old");
            let new_tools = external_tools(20, "new");
            registry.update(ToolsetId::None, &old_tools, None).unwrap();
            let start = Arc::new(Barrier::new(2));
            let sampled = Arc::new(AtomicUsize::new(0));
            let done = Arc::new(AtomicBool::new(false));
            let reader_registry = Arc::clone(&registry);
            let reader_start = Arc::clone(&start);
            let reader_sampled = Arc::clone(&sampled);
            let reader_done = Arc::clone(&done);
            let reader = std::thread::spawn(move || {
                reader_start.wait();
                while !reader_done.load(Ordering::Acquire) {
                    assert_complete(&reader_registry.snapshot().unwrap());
                    reader_sampled.fetch_add(1, Ordering::Release);
                }
            });
            start.wait();
            while sampled.load(Ordering::Acquire) == 0 {
                std::thread::yield_now();
            }
            for index in 0..100 {
                let tools = if index % 2 == 0 {
                    &new_tools
                } else {
                    &old_tools
                };
                registry.update(ToolsetId::None, tools, None).unwrap();
            }
            done.store(true, Ordering::Release);
            reader.join().unwrap();
            assert!(sampled.load(Ordering::Acquire) > 0);
        }

        #[test]
        fn concurrent_writers_leave_one_complete_candidate() {
            let registry = Arc::new(registry_with([]));
            let start = Arc::new(Barrier::new(3));
            let mut threads = Vec::new();
            for (count, prefix) in [(10, "left"), (20, "right")] {
                let registry = Arc::clone(&registry);
                let start = Arc::clone(&start);
                threads.push(std::thread::spawn(move || {
                    let tools = external_tools(count, prefix);
                    start.wait();
                    registry.update(ToolsetId::None, &tools, None).unwrap();
                }));
            }
            start.wait();
            for thread in threads {
                thread.join().unwrap();
            }
            let snapshot = registry.snapshot().unwrap();
            assert!(
                matches_complete(&snapshot, 10, "left") || matches_complete(&snapshot, 20, "right")
            );
        }

        #[test]
        fn rejects_at_tools_loaded_max() {
            let registry = registry_with([]);
            registry
                .update(
                    ToolsetId::None,
                    &external_tools(TOOLS_LOADED_MAX - 1, "below"),
                    None,
                )
                .unwrap();
            let exact = registry
                .update(
                    ToolsetId::None,
                    &external_tools(TOOLS_LOADED_MAX, "exact"),
                    None,
                )
                .unwrap();
            assert_eq!(exact.len(), TOOLS_LOADED_MAX);
            assert!(matches!(
                registry.update(
                    ToolsetId::None,
                    &external_tools(TOOLS_LOADED_MAX + 1, "above"),
                    None
                ),
                Err(RegistryError::TooManyTools)
            ));
            assert!(Arc::ptr_eq(&exact, &registry.snapshot().unwrap()));
        }

        #[test]
        fn new_accepts_exact_and_rejects_above() {
            ToolRegistry::new(external_tools(TOOLS_LOADED_MAX, "exact")).unwrap();
            assert!(matches!(
                ToolRegistry::new(external_tools(TOOLS_LOADED_MAX + 1, "above")),
                Err(RegistryError::TooManyTools)
            ));
        }

        #[test]
        fn none_exposes_external_and_empty_is_empty() {
            let registry = registry_with([]);
            assert!(
                registry
                    .update(ToolsetId::None, &[], None)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                registry
                    .update(
                        ToolsetId::None,
                        &[registration("external", "external")],
                        None
                    )
                    .unwrap()
                    .len(),
                1
            );
        }

        fn assert_complete(snapshot: &RegistrySnapshot) {
            assert!(matches_complete(snapshot, 10, "old") || matches_complete(snapshot, 20, "new"));
        }
        fn matches_complete(snapshot: &RegistrySnapshot, count: usize, prefix: &str) -> bool {
            snapshot.len() == count
                && (0..count)
                    .all(|index| snapshot.by_internal(&format!("{prefix}_{index}")).is_some())
        }
        fn external_tools(count: usize, prefix: &str) -> Vec<ToolRegistration> {
            (0..count)
                .map(|index| {
                    registration(&format!("{prefix}_{index}"), &format!("{prefix}_{index}"))
                })
                .collect()
        }
    }
}
