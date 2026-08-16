use super::*;
use crate::permissions::matcher::{PermissionEffect, PermissionRuleSpec};
use lotta_domain::{AgentId, ConversationId};
use lotta_runtime::ports::{PermissionAction, ToolApprovalPolicy};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static POLICY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn scope(value: &str) -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept(value).unwrap(),
        ConversationId::default_for_agent(),
        None,
    )
}

fn action(value: &str) -> PermissionAction {
    PermissionAction::new(value.into()).unwrap()
}

fn policy(mode: PermissionMode) -> (PathBuf, PermissionPolicy) {
    let id = POLICY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let cwd = std::env::temp_dir().join(format!("lotta-policy-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&cwd);
    fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let sources = scopes::PermissionSourcePaths::new(&cwd, &cwd, &cwd);
    let loaded = scopes::load_permissions(&sources, &cwd).unwrap();
    let policy = PermissionPolicy::new(&cwd, scope("agent-policy"), Some(mode), loaded).unwrap();
    (cwd, policy)
}

fn check_result(
    policy: &PermissionPolicy,
    tool: &str,
    value: serde_json::Value,
    approval: ToolApprovalPolicy,
    session: &[PermissionRule],
    extension: &[PermissionRule],
) -> Result<PermissionDecision, PermissionError> {
    let input =
        ValidatedToolInput::new(lotta_domain::BoundedJsonValue::new(value).unwrap()).unwrap();
    policy.check(
        PermissionInvocation {
            internal_name: tool,
            input: &input,
            action: &action("write"),
            approval_policy: approval,
        },
        session,
        extension,
    )
}

fn check(
    policy: &PermissionPolicy,
    tool: &str,
    value: serde_json::Value,
    approval: ToolApprovalPolicy,
    rules: &[PermissionRule],
) -> PermissionDecision {
    check_result(policy, tool, value, approval, rules, &[]).unwrap()
}

pub(crate) fn four_modes() {
    let cases = [
        ("standard", PermissionMode::Standard),
        ("acceptEdits", PermissionMode::AcceptEdits),
        ("unrestricted", PermissionMode::Unrestricted),
        ("strict", PermissionMode::Strict),
    ];
    for (spelling, mode) in cases {
        assert_eq!(
            serde_json::from_str::<PermissionMode>(&format!("\"{spelling}\"")).unwrap(),
            mode
        );
        assert_eq!(
            serde_json::to_string(&mode).unwrap(),
            format!("\"{spelling}\"")
        );
    }
    assert_eq!(PermissionMode::default(), PermissionMode::Unrestricted);
}

pub(crate) fn default_is_unrestricted() {
    assert_eq!(PermissionMode::default(), PermissionMode::Unrestricted);
}

pub(crate) fn mode_differences_and_precedence() {
    unrestricted_precedence();
    standard_and_strict_precedence();
}

fn unrestricted_precedence() {
    let (cwd, unrestricted) = policy(PermissionMode::Unrestricted);
    assert_eq!(
        check(
            &unrestricted,
            "Bash",
            json!({"cmd":"rm -rf /"}),
            ToolApprovalPolicy::Never,
            &[]
        ),
        PermissionDecision::Allow
    );
    assert_eq!(
        check(
            &unrestricted,
            "Bash",
            json!({"cmd":"cat a > b $(date)"}),
            ToolApprovalPolicy::Never,
            &[]
        ),
        PermissionDecision::Allow
    );
    let deny = [PermissionRule::parse("Bash(rm:*)", PermissionEffect::Deny).unwrap()];
    assert_eq!(
        check(
            &unrestricted,
            "Bash",
            json!({"cmd":"pwd; rm local"}),
            ToolApprovalPolicy::Never,
            &deny
        ),
        PermissionDecision::Deny
    );
    assert_eq!(
        check(
            &unrestricted,
            "Write",
            json!({"file_path":"x"}),
            ToolApprovalPolicy::Always,
            &[]
        ),
        PermissionDecision::Ask
    );
    let always_ask = [PermissionRule::parse("Write", PermissionEffect::AlwaysAsk).unwrap()];
    assert_eq!(
        check(
            &unrestricted,
            "Write",
            json!({"file_path":"x"}),
            ToolApprovalPolicy::Never,
            &always_ask
        ),
        PermissionDecision::Ask
    );
    let ask = [PermissionRule::parse("Write", PermissionEffect::Ask).unwrap()];
    assert_eq!(
        check(
            &unrestricted,
            "Write",
            json!({"file_path":"x"}),
            ToolApprovalPolicy::Never,
            &ask
        ),
        PermissionDecision::Allow
    );
    fs::remove_dir_all(cwd).unwrap();
}

fn standard_and_strict_precedence() {
    let (_, standard) = policy(PermissionMode::Standard);
    let (_, edits) = policy(PermissionMode::AcceptEdits);
    assert_eq!(
        check(
            &standard,
            "Edit",
            json!({"file_path":"x"}),
            ToolApprovalPolicy::Never,
            &[]
        ),
        PermissionDecision::Ask
    );
    assert_eq!(
        check(
            &edits,
            "Edit",
            json!({"file_path":"x"}),
            ToolApprovalPolicy::Never,
            &[]
        ),
        PermissionDecision::Allow
    );
    let allow =
        [PermissionRule::parse("Bash(cat allowed.txt:*)", PermissionEffect::Allow).unwrap()];
    assert_eq!(
        check(
            &standard,
            "Bash",
            json!({"cmd":"cat allowed.txt"}),
            ToolApprovalPolicy::Never,
            &allow
        ),
        PermissionDecision::Allow
    );
    assert_eq!(
        check(
            &standard,
            "Bash",
            json!({"cmd":"cat allowed.txt; printf other"}),
            ToolApprovalPolicy::Never,
            &allow
        ),
        PermissionDecision::Ask
    );
    assert_eq!(
        check(
            &standard,
            "Bash",
            json!({"cmd":"cat allowed.txt; rm local"}),
            ToolApprovalPolicy::Never,
            &allow
        ),
        PermissionDecision::Ask
    );
    assert_eq!(
        check(
            &standard,
            "Bash",
            json!({"cmd":"cat allowed.txt & rm local"}),
            ToolApprovalPolicy::Never,
            &allow
        ),
        PermissionDecision::Ask
    );
    unsafe_shell_does_not_fall_through(&standard);
    let (_, strict) = policy(PermissionMode::Strict);
    assert_eq!(
        check(
            &strict,
            "Bash",
            json!({"cmd":"cat allowed.txt"}),
            ToolApprovalPolicy::Never,
            &allow
        ),
        PermissionDecision::Allow
    );
}

fn unsafe_shell_does_not_fall_through(standard: &PermissionPolicy) {
    let bare = [PermissionRule::parse("Bash", PermissionEffect::Allow).unwrap()];
    assert_eq!(
        check(
            standard,
            "Bash",
            json!({"cmd":"cat allowed.txt > copy.txt"}),
            ToolApprovalPolicy::Never,
            &bare
        ),
        PermissionDecision::Ask
    );
    assert_eq!(
        check(
            standard,
            "Bash",
            json!({"cmd":"pwd"}),
            ToolApprovalPolicy::Never,
            &bare
        ),
        PermissionDecision::Allow
    );
    let wildcard = [PermissionRule::parse("*", PermissionEffect::Allow).unwrap()];
    assert_eq!(
        check(
            standard,
            "Bash",
            json!({"cmd":"cat $(printf allowed.txt)"}),
            ToolApprovalPolicy::Never,
            &wildcard
        ),
        PermissionDecision::Ask
    );
}

pub(crate) fn six_inputs_must_match() {
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let path = cwd.join("target.txt");
    let expected_scope = scope("agent-a");
    let expected_action = action("read");
    let rule = PermissionRule::from_spec(PermissionRuleSpec {
        effect: PermissionEffect::Allow,
        tool: Some("read_file".into()),
        command: Some("cat target.txt".into()),
        path: Some(path.clone()),
        cwd: Some(cwd.clone()),
        scope: Some(expected_scope.clone()),
        action: Some(expected_action.clone()),
    })
    .unwrap();
    let matches = |tool: &str,
                   command: &str,
                   path: &std::path::Path,
                   cwd: &std::path::Path,
                   scope: &RuntimeScope,
                   action: &PermissionAction| {
        rule.matches(&PermissionMatchInput {
            tool,
            command: Some(command),
            path: Some(path),
            cwd,
            scope,
            action,
        })
    };
    assert!(matches(
        "Read",
        "cat target.txt",
        &path,
        &cwd,
        &expected_scope,
        &expected_action
    ));
    assert!(!matches(
        "Write",
        "cat target.txt",
        &path,
        &cwd,
        &expected_scope,
        &expected_action
    ));
    assert!(!matches(
        "Read",
        "cat other",
        &path,
        &cwd,
        &expected_scope,
        &expected_action
    ));
    assert!(!matches(
        "Read",
        "cat target.txt",
        &cwd.join("other"),
        &cwd,
        &expected_scope,
        &expected_action
    ));
    assert!(!matches(
        "Read",
        "cat target.txt",
        &path,
        &cwd.join("other"),
        &expected_scope,
        &expected_action
    ));
    let other_scope = scope("agent-b");
    assert!(!matches(
        "Read",
        "cat target.txt",
        &path,
        &cwd,
        &other_scope,
        &expected_action
    ));
    let other_action = action("write");
    assert!(!matches(
        "Read",
        "cat target.txt",
        &path,
        &cwd,
        &expected_scope,
        &other_action
    ));
}

pub(crate) fn aliases_and_wildcard() {
    assert_eq!(matcher::canonical_tool_name("run_shell_command"), "Bash");
    assert_eq!(matcher::canonical_tool_name("SearchFileContent"), "Grep");
    assert_eq!(matcher::canonical_tool_name("Agent"), "Task");
    assert!(
        PermissionRule::parse("*", PermissionEffect::Allow)
            .unwrap()
            .matches(&PermissionMatchInput {
                tool: "Anything",
                command: None,
                path: None,
                cwd: PathBuf::from("/").as_path(),
                scope: &scope("agent-a"),
                action: &action("x"),
            })
    );
}

pub(crate) fn canonicalizes_before_policy() {
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(
        matcher::canonicalize_invocation_path(&cwd, "./a/../b/file").unwrap(),
        cwd.join("b/file")
    );
    assert_eq!(
        matcher::canonicalize_invocation_path(&cwd, "./b/file").unwrap(),
        cwd.join("b/file")
    );
    let rule = PermissionRule::parse("Read(b/file)", PermissionEffect::Allow).unwrap();
    assert!(rule.matches(&PermissionMatchInput {
        tool: "Read",
        command: None,
        path: Some(&cwd.join("b/file")),
        cwd: &cwd,
        scope: &scope("canonical-path"),
        action: &action("read"),
    }));
}

#[cfg(unix)]
#[test]
fn symlink_target_is_policy_input_without_relative_escape_match() {
    use std::os::unix::fs::symlink;
    let (cwd, _) = policy(PermissionMode::Standard);
    let outside = cwd.parent().unwrap().join(format!(
        "lotta-policy-outside-{}-{}",
        std::process::id(),
        POLICY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("target.txt"), "x").unwrap();
    symlink(&outside, cwd.join("escape")).unwrap();
    let canonical = matcher::canonicalize_invocation_path(&cwd, "escape/target.txt").unwrap();
    assert_eq!(
        canonical,
        outside.canonicalize().unwrap().join("target.txt")
    );
    let relative = PermissionRule::parse("Read(escape/**)", PermissionEffect::Allow).unwrap();
    assert!(!relative.matches(&PermissionMatchInput {
        tool: "Read",
        command: None,
        path: Some(&canonical),
        cwd: &cwd,
        scope: &scope("symlink-policy"),
        action: &action("read"),
    }));
    fs::remove_dir_all(cwd).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn path_globs_and_typed_paths_are_strict() {
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let expected_scope = scope("agent-path");
    let expected_action = action("read");
    let shallow = PermissionRule::parse(r"Read(src\*.rs)", PermissionEffect::Allow).unwrap();
    let deep = PermissionRule::parse("Read(src/**)", PermissionEffect::Allow).unwrap();
    let anywhere = PermissionRule::parse("Read(**/*.rs)", PermissionEffect::Allow).unwrap();
    let exact_deep = PermissionRule::parse("Read(src/**/a.rs)", PermissionEffect::Allow).unwrap();
    let matches = |rule: &PermissionRule, path: &Path| {
        rule.matches(&PermissionMatchInput {
            tool: "Read",
            command: None,
            path: Some(path),
            cwd: &cwd,
            scope: &expected_scope,
            action: &expected_action,
        })
    };
    assert!(matches(&shallow, &cwd.join("src/a.rs")));
    assert!(!matches(&shallow, &cwd.join("src/nested/a.rs")));
    assert!(matches(&deep, &cwd.join("src/nested/a.rs")));
    assert!(matches(&anywhere, &cwd.join("a.rs")));
    assert!(matches(&anywhere, &cwd.join("x/a.rs")));
    assert!(matches(&exact_deep, &cwd.join("src/a.rs")));
    assert!(matches(&exact_deep, &cwd.join("src/x/a.rs")));
    let mixed = PermissionRule::parse("Read(src/**/a*.rs)", PermissionEffect::Allow).unwrap();
    assert!(matches(&mixed, &cwd.join("src/x/y/alpha.rs")));
    assert!(!matches(&mixed, &cwd.join("src/x/y/alpha.txt")));
    let shallow_stars = PermissionRule::parse("Read(src/***.rs)", PermissionEffect::Allow).unwrap();
    assert!(matches(&shallow_stars, &cwd.join("src/a.rs")));
    assert!(!matches(&shallow_stars, &cwd.join("src/x/a.rs")));
    let repeated = PermissionRule::parse("Read(**/src/**/a*.rs)", PermissionEffect::Allow).unwrap();
    assert!(matches(&repeated, &cwd.join("src/alpha.rs")));
    assert!(matches(&repeated, &cwd.join("x/y/src/z/w/alpha.rs")));
    assert!(!matches(&repeated, &cwd.join("x/y/src/z/w/alpha.txt")));
    let absolute = format!(
        "Read(/**/{}/**/a*.rs)",
        cwd.file_name().unwrap().to_string_lossy()
    );
    let rooted = PermissionRule::parse(&absolute, PermissionEffect::Allow).unwrap();
    assert!(matches(&rooted, &cwd.join("alpha.rs")));
    assert!(matches(&rooted, &cwd.join("x/y/alpha.rs")));
    let excessive = std::iter::repeat_n("x", matcher::PERMISSION_PATH_COMPONENTS_MAX + 1)
        .collect::<Vec<_>>()
        .join("/");
    let excessive = format!("Read({excessive})");
    let excessive = PermissionRule::parse(&excessive, PermissionEffect::Allow).unwrap();
    assert!(!matches(&excessive, &cwd.join("x")));
    assert_eq!(
        PermissionRule::from_spec(PermissionRuleSpec {
            effect: PermissionEffect::Allow,
            tool: Some("Read".into()),
            command: None,
            path: Some(PathBuf::from("relative")),
            cwd: None,
            scope: None,
            action: None,
        })
        .unwrap_err(),
        PermissionError::InvalidPath
    );
}

fn repeated_command(segment: &str, count: usize) -> String {
    std::iter::repeat_n(segment, count)
        .collect::<Vec<_>>()
        .join(";")
}

#[test]
fn shell_segments_below_limit_127_accepted() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    assert_eq!(
        check(
            &policy,
            "Bash",
            json!({"cmd": repeated_command("pwd", 127)}),
            ToolApprovalPolicy::Never,
            &[]
        ),
        PermissionDecision::Allow
    );
}

#[test]
fn shell_segments_at_limit_128_accepted() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    assert_eq!(
        check(
            &policy,
            "Bash",
            json!({"cmd": repeated_command("pwd", 128)}),
            ToolApprovalPolicy::Never,
            &[]
        ),
        PermissionDecision::Allow
    );
}

#[test]
fn quoted_separators_do_not_count_as_shell_segments() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    let quoted = format!("printf '{}'", ";".repeat(129));
    assert_eq!(
        check(
            &policy,
            "Bash",
            json!({"cmd": quoted}),
            ToolApprovalPolicy::Never,
            &[]
        ),
        PermissionDecision::Allow
    );
}

#[test]
fn shell_segments_above_limit_129_is_unsafe_before_unrestricted() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    let deny = [PermissionRule::parse("Bash(rm:*)", PermissionEffect::Deny).unwrap()];
    let command = format!("{};rm local", repeated_command("pwd", 128));
    assert_eq!(
        check_result(
            &policy,
            "Bash",
            json!({"cmd": command}),
            ToolApprovalPolicy::Never,
            &deny,
            &[]
        ),
        Err(PermissionError::UnsafeShell)
    );
}

#[test]
fn shell_command_below_limit_accepted() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    let command = format!("pwd{}", " ".repeat(analyzer::SHELL_COMMAND_BYTES_MAX - 4));
    assert!(
        check_result(
            &policy,
            "Bash",
            json!({"cmd": command}),
            ToolApprovalPolicy::Never,
            &[],
            &[]
        )
        .is_ok()
    );
}

#[test]
fn shell_command_at_limit_accepted() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    let command = format!("pwd{}", " ".repeat(analyzer::SHELL_COMMAND_BYTES_MAX - 3));
    assert!(
        check_result(
            &policy,
            "Bash",
            json!({"cmd": command}),
            ToolApprovalPolicy::Never,
            &[],
            &[]
        )
        .is_ok()
    );
}

#[test]
fn shell_command_above_limit_rejected() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    let command = format!("pwd{}", " ".repeat(analyzer::SHELL_COMMAND_BYTES_MAX - 2));
    assert_eq!(
        check_result(
            &policy,
            "Bash",
            json!({"cmd": command}),
            ToolApprovalPolicy::Never,
            &[],
            &[]
        ),
        Err(PermissionError::UnsafeShell)
    );
}

#[test]
fn shell_tokens_below_limit_1023_accepted() {
    let (cwd, _) = policy(PermissionMode::Unrestricted);
    let command = format!("unknown {}", "x ".repeat(analyzer::SHELL_TOKENS_MAX - 2));
    assert!(analyzer::analyze_shell(&command, &cwd, std::slice::from_ref(&cwd)).is_ok());
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn shell_tokens_at_limit_1024_accepted() {
    let (cwd, _) = policy(PermissionMode::Unrestricted);
    let command = format!("unknown {}", "x ".repeat(analyzer::SHELL_TOKENS_MAX - 1));
    assert!(analyzer::analyze_shell(&command, &cwd, std::slice::from_ref(&cwd)).is_ok());
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn shell_tokens_above_limit_1025_is_unsafe() {
    let (cwd, _) = policy(PermissionMode::Standard);
    let command = format!("unknown {}", "x ".repeat(analyzer::SHELL_TOKENS_MAX));
    assert_eq!(
        analyzer::analyze_shell(&command, &cwd, std::slice::from_ref(&cwd)).unwrap_err(),
        PermissionError::UnsafeShell
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn permission_path_bytes_at_16384_accepted() {
    let cwd = std::env::temp_dir().canonicalize().unwrap();
    let value = "x".repeat(matcher::PERMISSION_PATH_BYTES_MAX);
    assert_eq!(
        matcher::canonicalize_invocation_path(&cwd, &value),
        Err(PermissionError::InvalidPath),
        "the policy text bound does not override filesystem component limits"
    );
}

#[test]
fn permission_path_bytes_above_16385_rejected() {
    let cwd = std::env::temp_dir().canonicalize().unwrap();
    let value = "x".repeat(matcher::PERMISSION_PATH_BYTES_MAX + 1);
    assert_eq!(
        matcher::canonicalize_invocation_path(&cwd, &value),
        Err(PermissionError::InvalidPath)
    );
}

#[test]
fn rule_path_components_at_256_accepted() {
    let mut path = PathBuf::from("/");
    for _ in 0..255 {
        path.push("x");
    }
    assert!(
        PermissionRule::from_spec(PermissionRuleSpec {
            effect: PermissionEffect::Allow,
            tool: Some("Read".into()),
            command: None,
            path: Some(path),
            cwd: None,
            scope: None,
            action: None
        })
        .is_ok()
    );
}

#[test]
fn rule_path_components_above_257_rejected() {
    let mut path = PathBuf::from("/");
    for _ in 0..256 {
        path.push("x");
    }
    assert_eq!(
        PermissionRule::from_spec(PermissionRuleSpec {
            effect: PermissionEffect::Allow,
            tool: Some("Read".into()),
            command: None,
            path: Some(path),
            cwd: None,
            scope: None,
            action: None
        }),
        Err(PermissionError::InvalidPath)
    );
}

#[test]
fn check_time_session_at_limit_1024_accepted() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    let rules = vec![
        PermissionRule::parse("Read", PermissionEffect::Allow).unwrap();
        PERMISSION_RULES_AT_CHECK_MAX
    ];
    assert!(
        check_result(
            &policy,
            "Read",
            json!({"file_path":"x"}),
            ToolApprovalPolicy::Never,
            &rules,
            &[]
        )
        .is_ok()
    );
}

#[test]
fn check_time_session_above_limit_1025_rejected() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    let rules = vec![
        PermissionRule::parse("Read", PermissionEffect::Allow).unwrap();
        PERMISSION_RULES_AT_CHECK_MAX + 1
    ];
    assert_eq!(
        check_result(
            &policy,
            "Read",
            json!({"file_path":"x"}),
            ToolApprovalPolicy::Never,
            &rules,
            &[]
        ),
        Err(PermissionError::InvalidRule)
    );
}

#[test]
fn check_time_mod_at_limit_1024_accepted() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    let rules = vec![
        PermissionRule::parse("Read", PermissionEffect::Allow).unwrap();
        PERMISSION_RULES_AT_CHECK_MAX
    ];
    assert!(
        check_result(
            &policy,
            "Read",
            json!({"file_path":"x"}),
            ToolApprovalPolicy::Never,
            &[],
            &rules
        )
        .is_ok()
    );
}

#[test]
fn check_time_mod_above_limit_1025_rejected() {
    let (_, policy) = policy(PermissionMode::Unrestricted);
    let rules = vec![
        PermissionRule::parse("Read", PermissionEffect::Allow).unwrap();
        PERMISSION_RULES_AT_CHECK_MAX + 1
    ];
    assert_eq!(
        check_result(
            &policy,
            "Read",
            json!({"file_path":"x"}),
            ToolApprovalPolicy::Never,
            &[],
            &rules
        ),
        Err(PermissionError::InvalidRule)
    );
}

#[test]
fn invocation_extracts_pinned_fields() {
    let value = lotta_domain::BoundedJsonValue::new(json!({"cmd":"pwd"})).unwrap();
    let input = ValidatedToolInput::new(value).unwrap();
    assert_eq!(
        extract_string(input.as_value(), &["cmd", "command"]),
        Some("pwd")
    );
}
