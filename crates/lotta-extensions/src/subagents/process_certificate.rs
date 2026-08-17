use super::spawn::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

fn owner() -> crate::sidecar::SidecarOwnerIdentity {
    crate::sidecar::SidecarOwnerIdentity::new(
        "certificate-agent",
        "certificate-runtime",
        "certificate-conversation",
    )
    .unwrap()
}

fn pinned() -> Option<PinnedLettaCodeSpec> {
    let root = PathBuf::from("/Volumes/Delorean/code/five-letters/letta-code");
    let bun = PathBuf::from("/Users/stan/.bun/bin/bun");
    (root.is_dir() && bun.is_file()).then_some(PinnedLettaCodeSpec {
        bun_executable: bun,
        source_root: root.clone(),
        cwd: PathBuf::from("/Volumes/Delorean/code/five-letters/lotta-workspaces/task-46"),
        environment: BTreeMap::from([
            (
                "HOME".into(),
                std::env::temp_dir()
                    .join(format!("lotta-task46-home-{}", std::process::id()))
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("PATH".into(), "/Users/stan/.bun/bin:/usr/bin:/bin".into()),
            (
                "BUN_INSTALL_CACHE_DIR".into(),
                root.join("node_modules/.cache")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("LOTTA_TASK46_DEV_BACKEND".into(), "fake-headless".into()),
            (
                "TMPDIR".into(),
                std::env::temp_dir()
                    .join(format!("lotta-task46-tmp-{}", std::process::id()))
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "LETTA_CODE_DEV_BACKEND_DIR".into(),
                std::env::temp_dir()
                    .join(format!("lotta-task46-backend-{}", std::process::id()))
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "LETTA_CODE_VERSION".into(),
                PINNED_LETTA_CODE_VERSION.into(),
            ),
        ]),
    })
}

#[tokio::test]
async fn real_pinned_process_version_is_exact() {
    let Some(spec) = pinned() else { return };
    assert_eq!(spec.version().await.unwrap(), PINNED_LETTA_CODE_VERSION);
}

#[tokio::test]
async fn full_pinned_adapter_runs_unmodified_cli_and_returns_real_stream() {
    let spec = pinned().expect("pinned 0.30.20 checkout and Bun are required");
    std::fs::create_dir_all(&spec.environment["HOME"]).unwrap();
    std::fs::create_dir_all(&spec.environment["LETTA_CODE_DEV_BACKEND_DIR"]).unwrap();
    std::fs::create_dir_all(&spec.environment["TMPDIR"]).unwrap();
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(format!("lotta-task46-visible-{nonce}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("allowed.txt"), "allowed-certificate-bytes").unwrap();
    let outside = root.with_file_name(format!("lotta-task46-outside-{nonce}"));
    let primary = root.with_file_name(format!("lotta-task46-primary-{nonce}"));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(&primary).unwrap();
    std::fs::write(outside.join("marker"), "task46-outside-secret").unwrap();
    std::fs::write(primary.join("marker"), "task46-primary-secret").unwrap();
    std::os::unix::fs::symlink(outside.join("marker"), root.join("escape-link")).unwrap();
    let probe = serde_json::json!({
        "inside": root.join("allowed.txt"),
        "absolute": outside.join("marker"),
        "parent": root.join("../").join(outside.file_name().unwrap()).join("marker"),
        "symlink": root.join("escape-link"),
        "primary": primary.join("marker"),
    });
    let mut spec = spec;
    spec.environment
        .insert("LOTTA_TASK46_MARKER_PROBE".into(), probe.to_string());
    let launcher = PinnedProcessLauncher::new(spec.clone()).unwrap();
    let mut request = crate::subagents::types_certificate::request_for_process_test();
    request.description = "certificate-description".into();
    request.prompt = "Return the deterministic response and preserve metadata.".into();
    request.resolved_context = Some("certificate-parent-context".into());
    request.model = super::types::ModelPolicy::Explicit("dev/fake-headless".into());
    request.tools = super::types::ToolPolicy::All;
    request.filesystem_roots = vec![root.clone()];
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let result = launcher
        .run(46, request, owner(), CancellationToken::new(), tx)
        .await
        .unwrap();
    assert!(result.success, "adapter diagnostic: {}", result.diagnostic);
    assert!(
        result.diagnostic.contains(
            "TASK46_MARKER_PROBE={\"absolute\":false,\"inside\":true,\"parent\":false,\"primary\":false,\"symlink\":false}"
        ),
        "{}",
        result.diagnostic
    );
    assert!(!result.diagnostic.contains("task46-outside-secret"));
    assert!(!result.diagnostic.contains("task46-primary-secret"));
    assert_eq!(result.report, "pong");
    let mut event = false;
    let mut response = false;
    while let Some(item) = rx.recv().await {
        let value: serde_json::Value = serde_json::from_slice(&item.bytes).unwrap();
        event |= value["kind"] == "event";
        response |= value["kind"] == "response"
            && value["payload"]["type"] == "result"
            && value["payload"]["result"] == "pong";
    }
    assert!(event && response);
    let storage = PathBuf::from(spec.environment["LETTA_CODE_DEV_BACKEND_DIR"].clone());
    let state = std::fs::read_dir(storage.join("agents"))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .collect::<String>();
    assert!(state.contains("dev/fake-headless"));
    let observed = std::fs::read_dir(storage.join("conversations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("messages.jsonl")).ok())
        .collect::<String>();
    for marker in [
        "certificate-description",
        "certificate-parent-context",
        "runtime_id\\\":\\\"runtime",
        &root.to_string_lossy(),
    ] {
        assert!(
            observed.contains(marker),
            "missing observed semantic: {marker}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(outside);
    let _ = std::fs::remove_dir_all(primary);
    let _ = std::fs::remove_dir_all(storage);
}

#[test]
fn package_hash_mismatch_is_refused_before_spawn() {
    let Some(spec) = pinned() else { return };
    let root = std::env::temp_dir().join(format!("lotta-task46-pin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("package.json"), br#"{"version":"0.29.12"}"#).unwrap();
    std::fs::write(root.join("src/index.ts"), "console.log('bad')").unwrap();
    let bad = PinnedLettaCodeSpec {
        source_root: root.clone(),
        ..spec
    };
    assert_eq!(bad.validate().unwrap_err(), SpawnError::Pin);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pinned_stream_arguments_are_exact() {
    let mut request = crate::subagents::types_certificate::request_for_process_test();
    request.model = super::types::ModelPolicy::AutoFast;
    let args = pinned_arguments(&request);
    assert_eq!(
        &args[..5],
        [
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
        ]
    );
    assert!(!args.iter().any(|arg| arg == "unrestricted"));
    assert!(args.windows(2).any(|pair| pair == ["--model", "haiku-4.5"]));
}

#[test]
fn installed_path_fallback_is_impossible() {
    let Some(mut spec) = pinned() else { return };
    spec.bun_executable = PathBuf::from("bun");
    assert_eq!(spec.validate().unwrap_err(), SpawnError::Pin);
}
