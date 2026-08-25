//! Task 40-local production composition of planning, task, skill, interaction, LSP, and shell tools.

use super::{
    interaction::{self, InteractionPort},
    lsp::{self, LanguageServerRegistry},
    planning::{self, PlanningPort},
    shell::ShellToolBundle,
    skill::{self, RegisteredSkillPort},
    task::{self, TaskLifecyclePort},
};
use crate::{ToolRegistry, registry::ToolRegistration};
use std::{collections::BTreeSet, sync::Arc};

/// Fixed Task 40 bundle construction error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Task40BundleError;

/// Task 40 registrations plus the canonical Task 38 shell registrations.
pub struct Task40ToolBundle {
    registrations: Vec<ToolRegistration>,
    tasks: Arc<TaskLifecyclePort>,
}

impl Task40ToolBundle {
    /// Composes explicit conversation ports with an existing canonical shell bundle.
    ///
    /// This helper performs no app/global wiring. Approval requests are only produced through the
    /// interaction port; Task 56 owns runtime permission-request consumption and resolution.
    ///
    /// # Errors
    /// Rejects malformed assets or any internal-name collision before exposing registrations.
    pub fn new(
        planning: Arc<PlanningPort>,
        tasks: Arc<TaskLifecyclePort>,
        skills: Arc<dyn RegisteredSkillPort>,
        interaction: Arc<InteractionPort>,
        lsp_registry: Arc<LanguageServerRegistry>,
        shell: &ShellToolBundle,
    ) -> Result<Self, Task40BundleError> {
        let mut registrations = planning::registrations(planning).map_err(|_| Task40BundleError)?;
        registrations
            .extend(task::registrations(Arc::clone(&tasks)).map_err(|_| Task40BundleError)?);
        registrations.push(skill::registration(skills).map_err(|_| Task40BundleError)?);
        registrations.push(interaction::registration(interaction).map_err(|_| Task40BundleError)?);
        registrations.push(lsp::registration(lsp_registry).map_err(|_| Task40BundleError)?);
        registrations.extend_from_slice(shell.registrations());
        validate_unique(&registrations)?;
        ToolRegistry::new(registrations.clone()).map_err(|_| Task40BundleError)?;
        Ok(Self {
            registrations,
            tasks,
        })
    }

    /// Returns the collision-checked registration set for Task 32 toolsets.
    #[must_use]
    pub fn registrations(&self) -> &[ToolRegistration] {
        &self.registrations
    }

    /// Returns the exact lifecycle port used by the registered task tools.
    #[must_use]
    pub fn tasks(&self) -> Arc<TaskLifecyclePort> {
        Arc::clone(&self.tasks)
    }
}

fn validate_unique(registrations: &[ToolRegistration]) -> Result<(), Task40BundleError> {
    let mut names = BTreeSet::new();
    if registrations.iter().any(|registration| {
        !names.insert(registration.definition.internal_name.as_str().to_owned())
    }) {
        Err(Task40BundleError)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
