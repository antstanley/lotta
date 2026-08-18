//! Bounded permission policy, file scopes, matching, and shell analysis.

/// Bounded structural shell analysis.
pub mod analyzer;
pub mod matcher;
pub mod mode;
pub mod scopes;

use analyzer::{ShellAnalysis, analyze_shell};
use lotta_domain::{PermissionMode, RuntimeScope};
use lotta_runtime::ports::{
    PermissionAction, ToolApprovalPolicy, ToolDefinition, ValidatedToolInput,
};
use matcher::{
    PermissionEffect, PermissionError, PermissionMatchInput, PermissionRule, canonicalize_root,
};
use scopes::{
    LoadedPermissions, PERMISSION_ADDITIONAL_DIRECTORIES_MAX, PERMISSION_RULES_PER_CATEGORY_MAX,
};

/// Maximum rules accepted from each check-time session or mod layer.
pub const PERMISSION_RULES_AT_CHECK_MAX: usize = 1_024;
use std::path::{Path, PathBuf};

pub use matcher::{canonical_tool_name, canonicalize_invocation_path, path_within};

/// Exhaustive policy result consumed by the tool pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionDecision {
    /// Continue execution.
    Allow,
    /// Stop with a fixed denial.
    Deny,
    /// Stop and request approval.
    Ask,
}

/// Borrowed invocation data extracted from a resolved validated tool call.
#[derive(Clone, Copy)]
pub struct PermissionInvocation<'a> {
    /// Resolved stable internal name.
    pub internal_name: &'a str,
    /// Validated input.
    pub input: &'a ValidatedToolInput,
    /// Definition's requested action.
    pub action: &'a PermissionAction,
    /// Definition approval policy. `Always` mandates approval before mode evaluation (including a
    /// future mod `alwaysAsk` mapping); it is distinct from an ordinary ask rule.
    pub approval_policy: ToolApprovalPolicy,
}

impl<'a> PermissionInvocation<'a> {
    /// Constructs an invocation directly from a resolved definition.
    #[must_use]
    pub fn from_definition(definition: &'a ToolDefinition, input: &'a ValidatedToolInput) -> Self {
        Self {
            internal_name: definition.internal_name.as_str(),
            input,
            action: &definition.permission_action,
            approval_policy: definition.approval_policy,
        }
    }
}

/// Synchronous gate seam used at the exact permission pipeline stage.
///
/// The gate precedes the sandbox stage and returns only a permission snapshot; it does not claim
/// that a sandbox exists or grant an I/O capability.
pub trait PermissionGate: Send + Sync {
    /// Checks one resolved validated invocation.
    ///
    /// # Errors
    /// Returns a fixed permission failure for unsafe input or analysis.
    fn check(
        &self,
        invocation: PermissionInvocation<'_>,
    ) -> Result<PermissionDecision, PermissionError>;
}

/// Explicit permissive gate used by Task 33 callers and tests.
pub struct AllowAllPermissions;
impl PermissionGate for AllowAllPermissions {
    fn check(&self, _: PermissionInvocation<'_>) -> Result<PermissionDecision, PermissionError> {
        Ok(PermissionDecision::Allow)
    }
}

/// Borrowed policy adapter that supplies check-time session and mod layers.
pub struct PolicyGate<'a> {
    policy: &'a PermissionPolicy,
    session_rules: &'a [PermissionRule],
    mod_rules: &'a [PermissionRule],
}

impl<'a> PolicyGate<'a> {
    /// Constructs a non-mutating check-time adapter.
    #[must_use]
    pub const fn new(
        policy: &'a PermissionPolicy,
        session_rules: &'a [PermissionRule],
        mod_rules: &'a [PermissionRule],
    ) -> Self {
        Self {
            policy,
            session_rules,
            mod_rules,
        }
    }
}

impl PermissionGate for PolicyGate<'_> {
    fn check(
        &self,
        invocation: PermissionInvocation<'_>,
    ) -> Result<PermissionDecision, PermissionError> {
        self.policy
            .check(invocation, self.session_rules, self.mod_rules)
    }
}

/// Immutable policy state loaded for one canonical cwd and exact runtime scope.
///
/// Canonical paths retained here are policy snapshots, never I/O capabilities. A later
/// sandbox/effect adapter must re-resolve and no-follow-check every path before every effect.
#[derive(Clone)]
pub struct PermissionPolicy {
    file_rules: Vec<PermissionRule>,
    cwd: PathBuf,
    additional_roots: Vec<PathBuf>,
    scope: RuntimeScope,
    mode: PermissionMode,
}

impl PermissionPolicy {
    /// Builds a policy from already-loaded file settings.
    ///
    /// # Errors
    /// Returns a fixed path error when `cwd` is not canonicalizable.
    pub fn new(
        cwd: &Path,
        scope: RuntimeScope,
        selected_mode: Option<PermissionMode>,
        loaded: LoadedPermissions,
    ) -> Result<Self, PermissionError> {
        let cwd = canonicalize_root(cwd)?;
        validate_loaded(&loaded)?;
        let mode = selected_mode.or(loaded.mode()).unwrap_or_default();
        let (file_rules, additional_roots) = loaded.into_parts();
        Ok(Self {
            file_rules,
            cwd,
            additional_roots,
            scope,
            mode,
        })
    }

    /// Checks one invocation with session and mod rules layered at check time.
    ///
    /// Decision precedence is hard rejection, deny, always-ask, definition approval, mode,
    /// explicit allow, explicit ask, modest implicit allowance, then mode fallback.
    /// `ToolApprovalPolicy::Always` maps definition-mandatory approval (including a future mod
    /// `alwaysAsk`) and therefore precedes mode; ordinary `PermissionEffect::Ask` remains below
    /// mode. This gate only emits a policy snapshot. The subsequent sandbox stage must re-resolve
    /// and no-follow-check paths before every effect; no permission decision authorizes I/O.
    ///
    /// # Errors
    /// Returns a fixed failure for unsafe path or shell analysis.
    pub fn check(
        &self,
        invocation: PermissionInvocation<'_>,
        session_rules: &[PermissionRule],
        mod_rules: &[PermissionRule],
    ) -> Result<PermissionDecision, PermissionError> {
        if session_rules.len() > PERMISSION_RULES_AT_CHECK_MAX
            || mod_rules.len() > PERMISSION_RULES_AT_CHECK_MAX
        {
            return Err(PermissionError::InvalidRule);
        }
        let normalized = self.normalize_invocation(&invocation)?;
        let segments = raw_segments(normalized.raw_command.as_deref())?;
        let input = self.match_input(&normalized, invocation.action);
        if let Some(decision) = self.strong_decision(
            &input,
            segments.as_deref(),
            invocation.approval_policy,
            session_rules,
            mod_rules,
        ) {
            return Ok(decision);
        }
        if let Some(effect) = mode::decision_for_mode(self.mode, normalized.tool) {
            return Ok(map_effect(effect));
        }
        self.analyzed_decision(&normalized, &input, session_rules, mod_rules)
    }

    fn match_input<'a>(
        &'a self,
        normalized: &'a NormalizedInvocation<'a>,
        action: &'a PermissionAction,
    ) -> PermissionMatchInput<'a> {
        PermissionMatchInput {
            tool: normalized.tool,
            command: normalized.raw_command.as_deref(),
            path: normalized.path.as_deref(),
            cwd: &self.cwd,
            scope: &self.scope,
            action,
        }
    }

    fn strong_decision(
        &self,
        input: &PermissionMatchInput<'_>,
        segments: Option<&[String]>,
        approval: ToolApprovalPolicy,
        session: &[PermissionRule],
        extension: &[PermissionRule],
    ) -> Option<PermissionDecision> {
        if matches_effect(
            &self.file_rules,
            session,
            extension,
            input,
            segments,
            PermissionEffect::Deny,
        ) {
            Some(PermissionDecision::Deny)
        } else if approval == ToolApprovalPolicy::Always
            || matches_effect(
                &self.file_rules,
                session,
                extension,
                input,
                segments,
                PermissionEffect::AlwaysAsk,
            )
        {
            Some(PermissionDecision::Ask)
        } else {
            None
        }
    }

    fn analyzed_decision(
        &self,
        normalized: &NormalizedInvocation<'_>,
        input: &PermissionMatchInput<'_>,
        session: &[PermissionRule],
        extension: &[PermissionRule],
    ) -> Result<PermissionDecision, PermissionError> {
        let shell = match self.shell_analysis(normalized)? {
            ShellSafety::NotShell => None,
            ShellSafety::Safe(value) => Some(value),
            ShellSafety::Unsafe => return Ok(PermissionDecision::Ask),
        };
        let segments = shell.as_ref().map(ShellAnalysis::segments);
        if matches_effect(
            &self.file_rules,
            session,
            extension,
            input,
            segments,
            PermissionEffect::Allow,
        ) {
            return Ok(PermissionDecision::Allow);
        }
        if matches_effect(
            &self.file_rules,
            session,
            extension,
            input,
            segments,
            PermissionEffect::Ask,
        ) {
            return Ok(PermissionDecision::Ask);
        }
        if self.mode != PermissionMode::Strict
            && implicit_allow(
                normalized.tool,
                shell.as_ref(),
                normalized.path.as_deref(),
                self,
            )
        {
            return Ok(PermissionDecision::Allow);
        }
        Ok(map_effect(mode::default_effect(self.mode)))
    }

    fn shell_analysis(
        &self,
        normalized: &NormalizedInvocation<'_>,
    ) -> Result<ShellSafety, PermissionError> {
        if normalized.tool != "Bash" {
            return Ok(ShellSafety::NotShell);
        }
        let command = normalized
            .raw_command
            .as_deref()
            .ok_or(PermissionError::UnsafeShell)?;
        match analyze_shell(command, &self.cwd, &self.allowed_roots()?) {
            Ok(value) => Ok(ShellSafety::Safe(value)),
            Err(PermissionError::UnsafeShell) => Ok(ShellSafety::Unsafe),
            Err(error) => Err(error),
        }
    }

    fn normalize_invocation<'a>(
        &self,
        invocation: &'a PermissionInvocation<'_>,
    ) -> Result<NormalizedInvocation<'a>, PermissionError> {
        let tool = canonical_tool_name(invocation.internal_name);
        let value = invocation.input.as_value();
        let path = extract_string(value, &["file_path", "path", "notebook_path"])
            .map(|raw| canonicalize_invocation_path(&self.cwd, raw))
            .transpose()?;
        let raw_command = extract_string(value, &["cmd", "command"])
            .map(collapse_command)
            .transpose()?;
        Ok(NormalizedInvocation {
            tool,
            raw_command,
            path,
        })
    }

    fn allowed_roots(&self) -> Result<Vec<PathBuf>, PermissionError> {
        let capacity = self
            .additional_roots
            .len()
            .checked_add(1)
            .ok_or(PermissionError::InvalidPath)?;
        let mut roots = Vec::new();
        roots
            .try_reserve(capacity)
            .map_err(|_| PermissionError::InvalidPath)?;
        roots.push(self.cwd.clone());
        roots.extend(self.additional_roots.iter().cloned());
        Ok(roots)
    }
}

enum ShellSafety {
    NotShell,
    Safe(ShellAnalysis),
    Unsafe,
}

struct NormalizedInvocation<'a> {
    tool: &'a str,
    raw_command: Option<String>,
    path: Option<PathBuf>,
}

fn matches_effect(
    file: &[PermissionRule],
    session: &[PermissionRule],
    extension: &[PermissionRule],
    input: &PermissionMatchInput<'_>,
    segments: Option<&[String]>,
    effect: PermissionEffect,
) -> bool {
    file.iter()
        .chain(session)
        .chain(extension)
        .any(|rule| rule.effect() == effect && rule_matches_effect(rule, input, segments, effect))
}

fn rule_matches_effect(
    rule: &PermissionRule,
    input: &PermissionMatchInput<'_>,
    segments: Option<&[String]>,
    effect: PermissionEffect,
) -> bool {
    let Some(segments) = segments else {
        return rule.matches(input);
    };
    if input.tool != "Bash" || segments.is_empty() {
        return rule.matches(input);
    }
    let bare = !rule.has_command_constraint()
        && rule.matches(&PermissionMatchInput {
            command: None,
            ..*input
        });
    match effect {
        PermissionEffect::Allow => {
            bare || segments.iter().all(|segment| {
                let candidate = PermissionMatchInput {
                    command: Some(segment),
                    ..*input
                };
                rule.matches(&candidate)
            })
        }
        PermissionEffect::Deny | PermissionEffect::AlwaysAsk | PermissionEffect::Ask => {
            bare || segments.iter().any(|segment| {
                let candidate = PermissionMatchInput {
                    command: Some(segment),
                    ..*input
                };
                rule.matches(&candidate)
            }) || rule.matches(input)
        }
    }
}

fn raw_segments(command: Option<&str>) -> Result<Option<Vec<String>>, PermissionError> {
    let Some(command) = command else {
        return Ok(None);
    };
    analyzer::split_raw_segments(command).map(Some)
}

fn collapse_command(value: &str) -> Result<String, PermissionError> {
    if value.is_empty() || value.len() > analyzer::SHELL_COMMAND_BYTES_MAX || value.contains('\0') {
        return Err(PermissionError::UnsafeShell);
    }
    let mut output = String::new();
    output
        .try_reserve(value.len())
        .map_err(|_| PermissionError::UnsafeShell)?;
    for word in value.split_whitespace() {
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(word);
    }
    if output.is_empty() {
        return Err(PermissionError::UnsafeShell);
    }
    Ok(output)
}

fn validate_loaded(loaded: &LoadedPermissions) -> Result<(), PermissionError> {
    let category_limit = PERMISSION_RULES_PER_CATEGORY_MAX
        .checked_mul(4)
        .ok_or(PermissionError::InvalidSettings)?;
    if loaded.rules().len() > category_limit
        || loaded.additional_directories().len() > PERMISSION_ADDITIONAL_DIRECTORIES_MAX
    {
        return Err(PermissionError::InvalidSettings);
    }
    for root in loaded.additional_directories() {
        canonicalize_root(root)?;
    }
    Ok(())
}

fn map_effect(effect: PermissionEffect) -> PermissionDecision {
    match effect {
        PermissionEffect::Allow => PermissionDecision::Allow,
        PermissionEffect::Deny => PermissionDecision::Deny,
        PermissionEffect::AlwaysAsk | PermissionEffect::Ask => PermissionDecision::Ask,
    }
}

fn implicit_allow(
    tool: &str,
    shell: Option<&ShellAnalysis>,
    path: Option<&Path>,
    policy: &PermissionPolicy,
) -> bool {
    if tool == "Bash" {
        return shell.is_some_and(ShellAnalysis::is_read_only);
    }
    if matches!(tool, "Read" | "Glob" | "Grep" | "ListDir") {
        return path.is_some_and(|path| {
            path_within(path, &policy.cwd)
                || policy
                    .additional_roots
                    .iter()
                    .any(|root| path_within(path, root))
        });
    }
    matches!(
        tool,
        "TodoWrite" | "TaskOutput" | "update_plan" | "UpdatePlan" | "memory"
    )
}

fn extract_string<'a>(value: &'a serde_json::Value, fields: &[&str]) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| value.get(field).and_then(serde_json::Value::as_str))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "tests/modes.rs"]
mod modes;

#[cfg(test)]
#[path = "tests/matching.rs"]
mod matching;
