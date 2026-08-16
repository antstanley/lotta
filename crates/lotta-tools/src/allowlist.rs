//! Shared built-in and external allowlist matching.

/// Borrowed optional allowlist without retaining caller input.
#[derive(Clone, Copy, Debug)]
pub struct ToolAllowlist<'a>(Option<&'a [&'a str]>);

impl<'a> ToolAllowlist<'a> {
    /// Creates a borrowed allowlist; `None` exposes every candidate.
    #[must_use]
    pub const fn new(entries: Option<&'a [&'a str]>) -> Self {
        Self(entries)
    }

    /// Tests both canonical/internal and model-facing identities through one path.
    #[must_use]
    pub fn permits(self, internal: &str, model: &str) -> bool {
        self.0.is_none_or(|entries| {
            entries
                .iter()
                .any(|entry| *entry == internal || *entry == model)
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::registry::tests::support::{registration, registry_with};
    use crate::toolset::ToolsetId;

    #[test]
    fn filters_builtins() {
        let registry = registry_with([registration("Task", "Task"), registration("Read", "Read")]);
        registry
            .update(ToolsetId::Default, &[], Some(&["Agent"]))
            .unwrap();
        let snapshot = registry.snapshot().unwrap();
        assert!(snapshot.by_internal("Task").is_some());
        assert_eq!(snapshot.len(), 1);
        registry
            .update(ToolsetId::Default, &[], Some(&["Task"]))
            .unwrap();
        assert_eq!(registry.snapshot().unwrap().model_names(), vec!["Agent"]);
    }

    #[test]
    fn filters_external() {
        let registry = registry_with([]);
        registry
            .update(
                ToolsetId::None,
                &[registration("external_key", "weather")],
                Some(&["weather"]),
            )
            .unwrap();
        assert_eq!(registry.snapshot().unwrap().model_names(), vec!["weather"]);
        registry
            .update(
                ToolsetId::None,
                &[registration("external_key", "weather")],
                Some(&["external_key"]),
            )
            .unwrap();
        assert_eq!(registry.snapshot().unwrap().len(), 1);
    }

    #[test]
    fn empty_exposes_none() {
        let registry = registry_with([registration("Read", "Read")]);
        registry
            .update(
                ToolsetId::Default,
                &[registration("external", "weather")],
                Some(&[]),
            )
            .unwrap();
        assert!(registry.snapshot().unwrap().is_empty());
    }
}
