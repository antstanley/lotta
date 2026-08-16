use super::*;
use crate::permissions::*;
use lotta_domain::{AgentId, ConversationId};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("lotta-permissions-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}

fn setup() -> (PathBuf, PathBuf, PathBuf, PermissionSourcePaths) {
    let base = root();
    let home = base.join("home");
    let xdg = base.join("xdg");
    let cwd = base.join("work");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&xdg).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let paths = PermissionSourcePaths::new(&home, &xdg, &cwd);
    (base, cwd, home, paths)
}

fn write_raw(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn write(path: &Path, permissions: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(r#"{{"unrelated":true,"permissions":{permissions}}}"#),
    )
    .unwrap();
}

#[test]
fn missing_all_four_sources_is_empty() {
    let (base, cwd, _, paths) = setup();
    let loaded = load_permissions(&paths, &cwd).unwrap();
    assert_eq!(loaded.mode(), None);
    assert!(loaded.rules().is_empty());
    assert!(loaded.additional_directories().is_empty());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn unrelated_key_only_is_accepted_empty() {
    let (base, cwd, _, paths) = setup();
    write_raw(&paths.project, br#"{"unrelated":true}"#);
    let loaded = load_permissions(&paths, &cwd).unwrap();
    assert_eq!(loaded.mode(), None);
    assert!(loaded.rules().is_empty());
    assert!(loaded.additional_directories().is_empty());
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn settings_symlink_rejected() {
    use std::os::unix::fs::symlink;
    let (base, cwd, _, paths) = setup();
    let target = base.join("settings-target.json");
    fs::write(&target, r#"{"permissions":{}}"#).unwrap();
    fs::create_dir_all(paths.project.parent().unwrap()).unwrap();
    symlink(target, &paths.project).unwrap();
    assert_eq!(
        load_permissions(&paths, &cwd).unwrap_err(),
        PermissionError::InvalidSettings
    );
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn settings_directory_nonregular_rejected() {
    let (base, cwd, _, paths) = setup();
    fs::create_dir_all(&paths.project).unwrap();
    assert_eq!(
        load_permissions(&paths, &cwd).unwrap_err(),
        PermissionError::InvalidSettings
    );
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn legacy_location_loader() {
    location_case(|paths| &paths.legacy);
}

#[test]
fn canonical_user_location_loader() {
    location_case(|paths| &paths.user);
}

#[test]
fn project_location_loader() {
    location_case(|paths| &paths.project);
}

#[test]
fn local_location_loader() {
    location_case(|paths| &paths.local);
}

fn location_case(select: impl Fn(&PermissionSourcePaths) -> &PathBuf) {
    let (base, cwd, _, paths) = setup();
    write(select(&paths), r#"{"allow":["Read(src/**)"]}"#);
    let loaded = load_permissions(&paths, &cwd).unwrap();
    assert_eq!(loaded.rules().len(), 1);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn merged_ordering_and_local_mode_precedence() {
    let (base, cwd, _, paths) = setup();
    write(&paths.legacy, r#"{"mode":"standard","allow":["Read(a)"]}"#);
    write(&paths.user, r#"{"allow":["Read(b)"]}"#);
    write(&paths.project, r#"{"mode":"strict","ask":["Write"]}"#);
    write(&paths.local, r#"{"mode":"acceptEdits","deny":["Bash"]}"#);
    let loaded = load_permissions(&paths, &cwd).unwrap();
    assert_eq!(loaded.mode(), Some(PermissionMode::AcceptEdits));
    assert_eq!(loaded.rules().len(), 4);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn equivalent_rules_are_deduplicated_after_normalization() {
    let (base, cwd, _, paths) = setup();
    write(
        &paths.project,
        r#"{"allow":["read_file(src\\**)","Read(./src/**)","Read(src/**)"]}"#,
    );
    let loaded = load_permissions(&paths, &cwd).unwrap();
    assert_eq!(loaded.rules().len(), 1);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn malformed_and_overbound_are_fixed_errors() {
    let (base, cwd, _, paths) = setup();
    fs::create_dir_all(paths.project.parent().unwrap()).unwrap();
    fs::write(&paths.project, "{").unwrap();
    assert_eq!(
        load_permissions(&paths, &cwd).unwrap_err(),
        PermissionError::InvalidSettings
    );
    fs::write(
        &paths.project,
        vec![b'x'; PERMISSION_SETTINGS_FILE_BYTES_MAX + 1],
    )
    .unwrap();
    assert_eq!(
        load_permissions(&paths, &cwd).unwrap_err(),
        PermissionError::InvalidSettings
    );
    fs::remove_dir_all(base).unwrap();
}

fn padded_settings(length: usize) -> Vec<u8> {
    let prefix = br#"{"permissions":{}}"#;
    let mut content = prefix.to_vec();
    content.resize(length, b' ');
    content
}

fn settings_bytes_case(length: usize) -> Result<LoadedPermissions, PermissionError> {
    let (base, cwd, _, paths) = setup();
    write_raw(&paths.project, &padded_settings(length));
    let result = load_permissions(&paths, &cwd);
    fs::remove_dir_all(base).unwrap();
    result
}

#[test]
fn settings_file_bytes_below_max_minus_one_accepted() {
    assert!(settings_bytes_case(PERMISSION_SETTINGS_FILE_BYTES_MAX - 1).is_ok());
}

#[test]
fn settings_file_bytes_at_max_accepted() {
    assert!(settings_bytes_case(PERMISSION_SETTINGS_FILE_BYTES_MAX).is_ok());
}

#[test]
fn settings_file_bytes_above_max_rejected() {
    assert_eq!(
        settings_bytes_case(PERMISSION_SETTINGS_FILE_BYTES_MAX + 1).unwrap_err(),
        PermissionError::InvalidSettings
    );
}

#[test]
fn rule_bytes_below_4095_accepted() {
    let payload = "x".repeat(matcher::PERMISSION_RULE_BYTES_MAX - 7);
    assert!(PermissionRule::parse(&format!("Bash({payload})"), PermissionEffect::Allow).is_ok());
}

#[test]
fn rule_bytes_at_4096_accepted() {
    let payload = "x".repeat(matcher::PERMISSION_RULE_BYTES_MAX - 6);
    assert!(PermissionRule::parse(&format!("Bash({payload})"), PermissionEffect::Allow).is_ok());
}

#[test]
fn rule_bytes_above_4097_rejected() {
    let payload = "x".repeat(matcher::PERMISSION_RULE_BYTES_MAX - 5);
    assert_eq!(
        PermissionRule::parse(&format!("Bash({payload})"), PermissionEffect::Allow),
        Err(PermissionError::InvalidRule)
    );
}

fn unique_rules(count: usize) -> String {
    let rules = (0..count)
        .map(|index| format!("Bash(cmd{index})"))
        .collect::<Vec<_>>();
    serde_json::to_string(&serde_json::json!({"allow": rules})).unwrap()
}

#[test]
fn rule_count_below_1023_accepted() {
    let (base, cwd, _, paths) = setup();
    write(&paths.project, &unique_rules(1_023));
    assert_eq!(load_permissions(&paths, &cwd).unwrap().rules().len(), 1_023);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn rule_count_at_1024_accepted() {
    let (base, cwd, _, paths) = setup();
    write(&paths.project, &unique_rules(1_024));
    assert_eq!(load_permissions(&paths, &cwd).unwrap().rules().len(), 1_024);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn rule_count_above_1025_rejected_without_partial_result() {
    let (base, cwd, _, paths) = setup();
    write(&paths.project, &unique_rules(1_025));
    assert_eq!(
        load_permissions(&paths, &cwd).unwrap_err(),
        PermissionError::InvalidSettings
    );
    fs::remove_dir_all(base).unwrap();
}

fn directory_settings(count: usize) -> String {
    let directories = (0..count)
        .map(|index| format!("dir-{index}"))
        .collect::<Vec<_>>();
    serde_json::to_string(&serde_json::json!({"additionalDirectories": directories})).unwrap()
}

fn additional_directories_case(count: usize) -> Result<LoadedPermissions, PermissionError> {
    let (base, cwd, _, paths) = setup();
    for index in 0..count {
        fs::create_dir(cwd.join(format!("dir-{index}"))).unwrap();
    }
    write(&paths.project, &directory_settings(count));
    let result = load_permissions(&paths, &cwd);
    fs::remove_dir_all(base).unwrap();
    result
}

#[test]
fn additional_directories_below_63_accepted() {
    assert_eq!(
        additional_directories_case(63)
            .unwrap()
            .additional_directories()
            .len(),
        63
    );
}

#[test]
fn additional_directories_at_64_accepted() {
    assert_eq!(
        additional_directories_case(64)
            .unwrap()
            .additional_directories()
            .len(),
        64
    );
}

#[test]
fn additional_directories_above_65_rejected() {
    assert_eq!(
        additional_directories_case(65).unwrap_err(),
        PermissionError::InvalidSettings
    );
}

#[test]
fn session_and_mod_layer_at_check_time() {
    let (base, cwd, _, paths) = setup();
    let loaded = load_permissions(&paths, &cwd).unwrap();
    let runtime = RuntimeScope::new(
        AgentId::accept("agent-scope").unwrap(),
        ConversationId::default_for_agent(),
        None,
    );
    let policy =
        PermissionPolicy::new(&cwd, runtime, Some(PermissionMode::Standard), loaded).unwrap();
    let input = ValidatedToolInput::new(
        lotta_domain::BoundedJsonValue::new(serde_json::json!({"file_path":"x"})).unwrap(),
    )
    .unwrap();
    let action = PermissionAction::new("write".into()).unwrap();
    let invocation = PermissionInvocation {
        internal_name: "Write",
        input: &input,
        action: &action,
        approval_policy: ToolApprovalPolicy::Never,
    };
    let session = [PermissionRule::parse("Write", PermissionEffect::Allow).unwrap()];
    let extension = [PermissionRule::parse("Write", PermissionEffect::Deny).unwrap()];
    assert_eq!(
        policy.check(invocation, &[], &[]).unwrap(),
        PermissionDecision::Ask
    );
    assert_eq!(
        policy.check(invocation, &session, &[]).unwrap(),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check(invocation, &session, &extension).unwrap(),
        PermissionDecision::Deny
    );
    assert_eq!(
        policy.check(invocation, &[], &[]).unwrap(),
        PermissionDecision::Ask
    );
    fs::remove_dir_all(base).unwrap();
}
