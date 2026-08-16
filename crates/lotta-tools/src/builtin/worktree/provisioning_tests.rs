use super::provisioning;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lotta-task39-provision-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("primary/.husky/_")).expect("primary");
        fs::create_dir_all(path.join("worktree/.git")).expect("worktree");
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn includes_hooks_settings_and_dependency_symlink() {
    let root = Root::new();
    let primary = root.0.join("primary");
    let worktree = root.0.join("worktree");
    fs::write(primary.join(".worktreeinclude"), ".env\nconfig\n").expect("include");
    fs::write(primary.join(".env"), "TOKEN=fixture").expect("env");
    fs::create_dir(primary.join("config")).expect("config");
    fs::write(primary.join("config/a"), "a").expect("a");
    fs::create_dir_all(primary.join(".letta")).expect("letta");
    fs::write(
        primary.join(".letta/settings.json"),
        r#"{"worktree":{"copyLocalSettings":true,"linkHooks":true}}"#,
    )
    .expect("config");
    fs::write(primary.join(".letta/settings.local.json"), "{}").expect("settings");
    fs::write(primary.join(".husky/_/pre-commit"), "hook").expect("hook");
    fs::create_dir(primary.join("node_modules")).expect("deps");
    let report = provisioning::provision(
        &primary,
        &worktree,
        &serde_json::json!({"symlink_dependencies":true}),
        Some(std::path::Path::new(".husky/_")),
    );
    let text = serde_json::to_string(&report).expect("report");
    assert!(text.contains("include:.env"));
    assert_eq!(
        fs::read_to_string(worktree.join(".env")).expect("env"),
        "TOKEN=fixture"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("config/a")).expect("a"),
        "a"
    );
    assert_eq!(
        fs::read_to_string(worktree.join(".letta/settings.local.json")).expect("settings"),
        "{}"
    );
    assert!(
        fs::symlink_metadata(worktree.join(".husky/_"))
            .expect("hooks")
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::symlink_metadata(worktree.join("node_modules"))
            .expect("deps")
            .file_type()
            .is_symlink()
    );
}
#[test]
fn unsafe_includes_are_best_effort_skips() {
    let root = Root::new();
    let primary = root.0.join("primary");
    let worktree = root.0.join("worktree");
    fs::write(primary.join(".worktreeinclude"), "../peer\nmissing\n").expect("include");
    let report = provisioning::provision(&primary, &worktree, &serde_json::json!({}), None);
    let text = serde_json::to_string(&report).expect("report");
    assert!(text.contains("skipped"));
    assert!(!root.0.join("peer").exists());
}
#[test]
fn include_file_bound_is_reported_not_fatal() {
    let root = Root::new();
    let primary = root.0.join("primary");
    let worktree = root.0.join("worktree");
    fs::write(
        primary.join(".worktreeinclude"),
        vec![b'x'; provisioning::INCLUDE_FILE_BYTES_MAX + 1],
    )
    .expect("include");
    let report = provisioning::provision(&primary, &worktree, &serde_json::json!({}), None);
    assert!(
        serde_json::to_string(&report)
            .expect("report")
            .contains("include-file")
    );
}
