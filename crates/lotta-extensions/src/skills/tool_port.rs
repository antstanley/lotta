use super::{Skill, SkillError, SkillLoader};
use lotta_tools::builtin::skill::{
    RegisteredSkillCompanion, RegisteredSkillContent, RegisteredSkillPort, SkillLoadFuture,
    SkillPortError,
};
use std::collections::BTreeMap;

/// Registry-backed Task 40 adapter. Unknown IDs are rejected before Task 36 filesystem work.
pub struct SkillToolPort {
    registry: BTreeMap<String, Skill>,
}

impl SkillToolPort {
    /// Builds an immutable exact-ID registry from discovery output.
    ///
    /// # Errors
    /// Rejects duplicate IDs.
    pub fn new(skills: impl IntoIterator<Item = Skill>) -> Result<Self, SkillPortError> {
        let mut registry = BTreeMap::new();
        for skill in skills {
            if registry.insert(skill.id.clone(), skill).is_some() {
                return Err(SkillPortError::Invalid);
            }
        }
        Ok(Self { registry })
    }
}

impl RegisteredSkillPort for SkillToolPort {
    fn load(&self, id: &str) -> SkillLoadFuture<'_> {
        let skill = self.registry.get(id).cloned();
        Box::pin(async move {
            let skill = skill.ok_or(SkillPortError::Unknown)?;
            let loaded = SkillLoader::load(&skill).map_err(map_error)?;
            let companions = loaded
                .companions
                .into_iter()
                .map(|value| RegisteredSkillCompanion {
                    relative_path: value.relative_path,
                    bytes: value.bytes,
                })
                .collect();
            Ok(RegisteredSkillContent {
                instructions: loaded.document.instructions,
                companions,
            })
        })
    }
}

fn map_error(error: SkillError) -> SkillPortError {
    match error {
        SkillError::LimitExceeded => SkillPortError::Limit,
        _ => SkillPortError::Invalid,
    }
}
