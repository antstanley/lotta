use super::registrations::ModRegistrationSnapshot;
use super::types::{ModError, ModOwner};
use std::sync::Arc;

/// Pinned source trust classification used by safe mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModTrust {
    /// Shipped with the application.
    BuiltIn,
    /// Explicitly trusted by configuration.
    Trusted,
    /// Third-party source skipped in safe mode.
    ThirdParty,
}
/// Startup policy adapter reserved for the future CLI composition task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModsStartupMode {
    /// Launch admitted mods normally.
    Enabled,
    /// Retain built-in/trusted mods and skip third-party mods.
    SafeMode,
    /// Do not launch any host and remove every mod registration.
    NoMods,
}
/// One source considered for startup.
#[derive(Clone)]
pub struct ModSource {
    /// Exact owner generation.
    pub owner: ModOwner,
    /// Trust classification.
    pub trust: ModTrust,
    /// Fully validated registrations.
    pub registrations: Arc<ModRegistrationSnapshot>,
}
/// Safe owner-attributed diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafeModeDiagnostic {
    /// Owner skipped or removed.
    pub owner: ModOwner,
    /// Stable reason without paths or source content.
    pub reason: &'static str,
}

/// Stable reason recorded for a third-party mod skipped by safe mode.
pub const MOD_SKIP_REASON_SAFE_MODE: &str = "safe_mode";
/// Stable reason recorded for every mod skipped because the host is disabled.
pub const MOD_SKIP_REASON_NO_MODS: &str = "mods_disabled";
/// Stable reason recorded for a candidate whose load failed before publication.
pub const MOD_SKIP_REASON_LOAD_FAILED: &str = "load_failed";

/// Decides the exact startup fate of one source, returning its stable skip reason.
///
/// This is the single policy used by both source filtering and controller startup, so normal,
/// safe, and no-mods semantics cannot diverge between the two entry points.
#[must_use]
pub const fn skip_reason(mode: ModsStartupMode, trust: ModTrust) -> Option<&'static str> {
    match mode {
        ModsStartupMode::Enabled => None,
        ModsStartupMode::SafeMode => match trust {
            ModTrust::ThirdParty => Some(MOD_SKIP_REASON_SAFE_MODE),
            ModTrust::BuiltIn | ModTrust::Trusted => None,
        },
        ModsStartupMode::NoMods => Some(MOD_SKIP_REASON_NO_MODS),
    }
}

/// Applies the pinned safe-mode trust policy before any launch.
pub fn filter_sources(
    mode: ModsStartupMode,
    sources: Vec<ModSource>,
) -> Result<(Vec<ModSource>, Vec<SafeModeDiagnostic>), ModError> {
    let mut retained = Vec::new();
    let mut diagnostics = Vec::new();
    for source in sources {
        if let Some(reason) = skip_reason(mode, source.trust) {
            diagnostics.push(SafeModeDiagnostic {
                owner: source.owner,
                reason,
            });
        } else {
            retained.push(source);
        }
    }
    if diagnostics.len() > super::types::MOD_DIAGNOSTICS_ITEMS_MAX {
        return Err(ModError::InvalidRegistration);
    }
    Ok((retained, diagnostics))
}
