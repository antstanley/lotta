use super::{ModelHandle, ModelSettings};
use lotta_runtime::model::{PrecedenceLevel, select_precedence};

/// A complete or settings-only override at one resolution level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelOverride {
    /// Optional handle selected at this level.
    pub handle: Option<ModelHandle>,
    /// Optional settings selected at this level.
    pub settings: Option<ModelSettings>,
}

/// The level which selected the effective handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolutionLevel {
    /// Request-scoped temporary override.
    Request,
    /// Persisted conversation override.
    Conversation,
    /// Persisted agent selection.
    Agent,
    /// Configured local fallback.
    LocalDefault,
}

/// How effective settings were derived.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsSource {
    /// Exactly one precedence level supplied settings.
    SameLevel,
    /// Settings from multiple precedence levels were merged low to high.
    MergedByPrecedence,
}

/// Fully resolved provider/model identity and settings provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedModel {
    /// Effective stable handle.
    pub handle: ModelHandle,
    /// Effective open settings.
    pub settings: ModelSettings,
    /// Level that selected the handle.
    pub level: ResolutionLevel,
    /// Derivation of the settings attached to the selected handle.
    pub settings_source: SettingsSource,
}

/// Resolves request, conversation, agent, then required local default.
///
/// Settings merge independently from low to high, matching the pinned baseline: local defaults,
/// agent settings, conversation settings, then request settings.
///
/// # Errors
/// Returns an error when the required configured local default has no handle.
pub fn resolve_model(
    request: &ModelOverride,
    conversation: &ModelOverride,
    agent: &ModelOverride,
    local_default: &ModelOverride,
) -> Result<ResolvedModel, ResolutionError> {
    let levels = [request, conversation, agent, local_default];
    let selected = select_precedence(levels.map(|candidate| candidate.handle.as_ref()))
        .ok_or(ResolutionError::MissingLocalDefault)?;
    let mut settings = ModelSettings::default();
    let mut settings_levels = 0_usize;
    for candidate in [local_default, agent, conversation, request] {
        if let Some(candidate_settings) = &candidate.settings {
            settings = settings.merged_with(candidate_settings);
            settings_levels += 1;
        }
    }
    Ok(ResolvedModel {
        handle: selected.value.clone(),
        settings,
        level: resolution_level(selected.level),
        settings_source: if settings_levels <= 1 {
            SettingsSource::SameLevel
        } else {
            SettingsSource::MergedByPrecedence
        },
    })
}

const fn resolution_level(level: PrecedenceLevel) -> ResolutionLevel {
    match level {
        PrecedenceLevel::Request => ResolutionLevel::Request,
        PrecedenceLevel::Conversation => ResolutionLevel::Conversation,
        PrecedenceLevel::Agent => ResolutionLevel::Agent,
        PrecedenceLevel::LocalDefault => ResolutionLevel::LocalDefault,
    }
}

/// Stable model-resolution failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ResolutionError {
    /// The configured local default omitted its required handle.
    #[error("missing local default model")]
    MissingLocalDefault,
}
