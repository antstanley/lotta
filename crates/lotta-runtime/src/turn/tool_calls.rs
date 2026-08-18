use crate::RuntimeError;
use crate::bounds::PROVIDER_TOOLS_MAX;
use crate::ports::{ModelFacingToolName, ToolDefinition};
use std::collections::BTreeMap;

/// Bounded model-name catalog for tools eligible during one turn.
pub struct TurnToolCatalog {
    definitions: BTreeMap<ModelFacingToolName, ToolDefinition>,
}

impl TurnToolCatalog {
    /// Builds a bounded duplicate-free model-facing tool catalog.
    ///
    /// # Errors
    /// Rejects an overbound catalog or duplicate model-facing names.
    pub fn new(definitions: Vec<ToolDefinition>) -> Result<Self, RuntimeError> {
        if definitions.len() > PROVIDER_TOOLS_MAX.value {
            return Err(RuntimeError::LimitExceeded {
                context: PROVIDER_TOOLS_MAX.name.into(),
            });
        }
        let mut indexed = BTreeMap::new();
        for definition in definitions {
            if indexed
                .insert(definition.model_name.clone(), definition)
                .is_some()
            {
                return Err(RuntimeError::InvalidData {
                    context: "turn tool catalog duplicate model name".into(),
                });
            }
        }
        Ok(Self {
            definitions: indexed,
        })
    }

    pub(crate) fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.definitions
            .iter()
            .find_map(|(candidate, definition)| (candidate.as_str() == name).then_some(definition))
    }

    /// Returns definitions in stable model-name order for provider request composition.
    pub fn definitions(&self) -> impl Iterator<Item = &ToolDefinition> {
        self.definitions.values()
    }
}
