//! Pure permission-mode and precedence helpers.

use super::matcher::{PermissionEffect, mode_override};
use lotta_domain::PermissionMode;

/// Applies only the mode override after stronger rule classes have been checked.
#[must_use]
pub fn decision_for_mode(mode: PermissionMode, tool: &str) -> Option<PermissionEffect> {
    mode_override(mode, tool)
}

/// Returns the mode's fallback when no rule or implicit allowance applies.
#[must_use]
pub const fn default_effect(mode: PermissionMode) -> PermissionEffect {
    match mode {
        PermissionMode::Strict | PermissionMode::Standard | PermissionMode::AcceptEdits => {
            PermissionEffect::Ask
        }
        PermissionMode::Unrestricted => PermissionEffect::Allow,
    }
}
