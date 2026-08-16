use super::support;
use lotta_runtime::ports::AgentStore;

#[tokio::test]
async fn no_concurrent_writers() {
    direction_rust_holds().await;
    direction_typescript_holds().await;
}

async fn direction_rust_holds() {
    let root = support::TestRoot::new("lease-rust-holds");
    let lease = support::Lease::acquire(root.path(), "rust").expect("Rust handoff lease");
    let store = support::backend_store(&root);
    AgentStore::save(&store, &agent("Rust lease owner"))
        .await
        .expect("actual Rust save while lease held");
    let before = super::tree::inventory_tree(&root.backend()).expect("before rejected TS");
    let failure = support::run_ts_failure(&root, "backend_write", "agent");
    assert_eq!(failure["ok"], false);
    assert!(failure["message"].as_str().unwrap().contains("lease"));
    assert_eq!(
        before,
        super::tree::inventory_tree(&root.backend()).expect("after rejected TS")
    );
    drop(lease);
}

async fn direction_typescript_holds() {
    let root = support::TestRoot::new("lease-ts-holds");
    let holder = support::TsHolder::spawn(&root).expect("spawn TS holder");
    holder.wait_ready().expect("TS holder ready");
    let before = super::tree::inventory_tree(&root.backend()).expect("holder inventory");
    assert!(support::Lease::acquire(root.path(), "rust-contender").is_err());
    assert_eq!(
        before,
        super::tree::inventory_tree(&root.backend()).expect("rejected Rust inventory")
    );
    let response = holder.release_and_finish().expect("holder full exit");
    assert_eq!(response["value"]["phase"], "holder-released");
    let lease = support::Lease::acquire(root.path(), "rust-after-holder").expect("Rust lease");
    let store = support::backend_store(&root);
    AgentStore::save(&store, &agent("Rust after TS holder"))
        .await
        .expect("actual Rust save after holder exit");
    drop(lease);
    let persisted = AgentStore::load(&store, &support::agent_id())
        .await
        .expect("persisted Rust update");
    assert_eq!(persisted.name.as_str(), "Rust after TS holder");
}

#[test]
fn process_output_is_bounded() {
    for operation in ["overflow_stdout", "overflow_stderr"] {
        let root = support::TestRoot::new(operation);
        let started = std::time::Instant::now();
        let error = support::run_ts_raw(&root, operation).expect_err("bounded output failure");
        assert!(
            error.contains("output"),
            "unexpected bounded error: {error}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}

#[test]
fn runner_uses_pinned_sources() {
    let root = support::TestRoot::new("provenance");
    let response = support::run_ts(&root, "provenance", "sources");
    let pinned = support::pinned_root();
    let manifest_path =
        support::manifest_dir().join("tests/conformance/harness/pinned-sources.json");
    let manifest = support::read_json(&manifest_path);
    let index = support::read_json(&support::fixture("index.json"));
    let expected = "300f923f16cc8eee50656d7da732902c1dea2b65";
    let before_status = command(
        &pinned,
        "git",
        &["status", "--porcelain", "--untracked-files=no"],
    );
    assert!(before_status.status.success());
    assert!(
        before_status.stdout.is_empty(),
        "pinned tracked tree starts dirty"
    );
    assert!(
        before_status.stderr.len() <= 4_096,
        "git status stderr too large"
    );
    let package = pinned.join("package.json");
    let lock = pinned.join("bun.lock");
    let package_hash = super::tree::hash_file_bounded(&package, super::tree::TREE_FILE_BYTES_MAX)
        .expect("bounded package hash");
    let lock_hash = super::tree::hash_file_bounded(&lock, super::tree::TREE_FILE_BYTES_MAX)
        .expect("bounded lock hash");
    let commit = command(&pinned, "git", &["rev-parse", "HEAD"]);
    assert_eq!(String::from_utf8(commit.stdout).unwrap().trim(), expected);
    assert_eq!(manifest["source_commit"], expected);
    assert_eq!(index["source_commit"], expected);
    assert_eq!(response["sourceCommit"], expected);
    assert_eq!(response["pinnedRoot"].as_str(), pinned.to_str());
    assert_sources(&pinned, &manifest, &response);
    let install = command(
        &pinned,
        "bun",
        &["install", "--frozen-lockfile", "--ignore-scripts"],
    );
    assert!(install.status.success());
    assert!(pinned.join("node_modules").is_dir());
    assert_eq!(
        before_status.stdout,
        command(
            &pinned,
            "git",
            &["status", "--porcelain", "--untracked-files=no"]
        )
        .stdout
    );
    assert_eq!(
        package_hash,
        super::tree::hash_file_bounded(&package, super::tree::TREE_FILE_BYTES_MAX).unwrap()
    );
    assert_eq!(
        lock_hash,
        super::tree::hash_file_bounded(&lock, super::tree::TREE_FILE_BYTES_MAX).unwrap()
    );
}

fn assert_sources(
    pinned: &std::path::Path,
    manifest: &serde_json::Value,
    response: &serde_json::Value,
) {
    for (relative, expected_sha) in manifest["sources"].as_object().expect("sources") {
        let path = pinned.join(relative);
        let actual = super::tree::hash_file_bounded(&path, super::tree::TREE_FILE_BYTES_MAX)
            .expect("bounded source hash");
        assert_eq!(expected_sha, &actual, "manifest SHA for {relative}");
        assert_eq!(response["sources"][relative]["sha256"], actual);
        assert_eq!(
            response["sources"][relative]["url"],
            format!("file://{}", path.display())
        );
    }
}

fn command(directory: &std::path::Path, program: &str, args: &[&str]) -> support::ProcessOutput {
    let output = support::run_command_bounded(program, args, directory).expect("bounded command");
    assert!(output.status.success(), "{program} command failed");
    output
}

fn agent(name: &str) -> lotta_domain::Agent {
    serde_json::from_value(serde_json::json!({
        "id": support::AGENT_ID, "name": name, "description": null,
        "system": "synthetic", "tags": ["handoff"],
        "model": "openai/gpt-4.1-mini", "model_settings": {}
    }))
    .expect("handoff agent")
}

#[test]
fn paths_remain_confined() {
    let root = support::TestRoot::new("confinement");
    let outside = root
        .path()
        .parent()
        .expect("parent")
        .join("task31-outside-sentinel");
    std::fs::write(&outside, b"unchanged").expect("sentinel");
    let failure = support::run_ts_with_paths(
        &root,
        "side_write",
        "settings",
        root.path().join("../escaped-backend"),
    );
    assert_eq!(failure["ok"], false);
    assert_eq!(
        std::fs::read(&outside).expect("sentinel read"),
        b"unchanged"
    );
    assert!(!root.path().join("../escaped-backend").exists());
    std::fs::remove_file(outside).expect("remove sentinel");
}
