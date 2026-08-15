use super::*;
use lotta_runtime::boundary::DiffChunk;
use lotta_runtime::ports::{MemFsHistoryEntry, MemFsTreeEntry, PortFuture};
use tokio::time::{Duration, timeout};

fn decoded_description(rendered: &[u8]) -> String {
    let text = std::str::from_utf8(rendered).expect("utf8");
    let scalar = text
        .lines()
        .nth(1)
        .expect("description")
        .strip_prefix("description: ")
        .expect("description key");
    serde_json::from_str(scalar).expect("JSON-style YAML scalar")
}

#[tokio::test]
async fn frontmatter_fallback() {
    let (_root, port, agent) = fixture();
    let input = MemoryBlockInput {
        label: NonEmptyString::new("persona\nlabel").expect("label"),
        value: "friendly".into(),
        description: None,
    };
    let block = InitialMemoryBlock::from_domain(input).expect("block");
    let rendered = crate::render_block(&block).expect("render");
    assert_eq!(decoded_description(&rendered), "Memory block persona label");
    for (raw, expected) in [
        (": bad", ": bad"),
        ("# comment", "# comment"),
        ("{x: y}", "{x: y}"),
        ("say \"hi\" \\ path", "say \"hi\" \\ path"),
        ("control\u{0001}\nnext", "control\u{0001} next"),
    ] {
        let input = MemoryBlockInput {
            label: NonEmptyString::new("persona").expect("label"),
            value: "friendly".into(),
            description: Some(Some(raw.to_owned())),
        };
        let block = InitialMemoryBlock::from_domain(input).expect("block");
        let rendered = crate::render_block(&block).expect("render");
        assert_eq!(decoded_description(&rendered), expected);
        assert_eq!(
            std::str::from_utf8(&rendered)
                .expect("utf8")
                .lines()
                .count(),
            4
        );
    }
    let blocks = InitialMemoryBlocks::new(vec![block]).expect("blocks");
    port.initialize(&agent, &blocks).await.expect("initialize");
}

#[tokio::test]
async fn no_implicit_remote() {
    let (_root, port, agent) = fixture();
    initialized(&port, &agent).await;
    let repo = port.repo(&agent).expect("repo");
    let output = crate::git::checked(&repo, ["remote"], "git remote").expect("remote");
    assert!(output.is_empty());
    let configured = crate::git::run(
        &repo,
        ["config", "--local", "--get", "letta.memoryRepository.url"],
        "config",
    )
    .expect("config");
    assert!(!configured.status.success());
}

#[tokio::test]
async fn bounds() {
    assert_eq!(crate::MEMORY_FILE_BYTES_MAX.value, 8 * 1024 * 1024);
    assert_eq!(crate::MEMORY_FILES_MAX.value, 100_000);
    let (_root, port, agent) = fixture();
    initialized(&port, &agent).await;
    let file = path("binary.bin");
    for value in [
        crate::MEMORY_FILE_BYTES_MAX.value - 1,
        crate::MEMORY_FILE_BYTES_MAX.value,
    ] {
        let contents = MemoryFileContent::new(vec![0; value]).expect("bounded content");
        port.write(&agent, &file, &contents)
            .await
            .expect("write boundary");
        assert_eq!(
            port.read(&agent, &file).await.expect("read boundary"),
            contents
        );
    }
    assert!(MemoryFileContent::new(vec![0; crate::MEMORY_FILE_BYTES_MAX.value + 1]).is_err());
    assert!(crate::fs::validate_new_file_count(crate::MEMORY_FILES_MAX.value - 2).is_ok());
    assert!(crate::fs::validate_new_file_count(crate::MEMORY_FILES_MAX.value - 1).is_ok());
    assert!(crate::fs::validate_new_file_count(crate::MEMORY_FILES_MAX.value).is_err());
    assert!(crate::fs::validate_new_file_count(usize::MAX).is_err());
}

#[cfg(unix)]
#[test]
fn constructor_rejects_root_symlink_and_stores_canonical_nested_root() {
    use std::os::unix::fs::symlink;

    let root = TestRoot::new();
    let nested = root.0.join("nested");
    fs::create_dir(&nested).expect("nested");
    let alias_parent = root.0.join("alias-parent");
    symlink(&root.0, &alias_parent).expect("alias parent");
    let alias_nested = alias_parent.join("nested");
    let port = GitMemFs::new(alias_nested).expect("canonical nested root");
    assert_eq!(
        port.root_path(),
        &fs::canonicalize(&nested).expect("canonical")
    );
    let direct = root.0.join("direct");
    symlink(&nested, &direct).expect("direct symlink");
    assert!(matches!(
        GitMemFs::new(direct),
        Err(RuntimeError::InvalidData { .. })
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn retained_backend_survives_ambient_replacement() {
    use std::os::unix::fs::symlink;

    let root = TestRoot::new();
    let backend = root.0.join("backend");
    fs::create_dir(&backend).expect("backend");
    let port = GitMemFs::new(backend.clone()).expect("adapter");
    let agent = agent();
    initialized(&port, &agent).await;
    let moved = root.0.join("retained-backend");
    fs::rename(&backend, &moved).expect("move backend");
    let outside = root.0.join("outside");
    fs::create_dir(&outside).expect("outside");
    let sentinel = outside.join("sentinel");
    fs::write(&sentinel, b"unchanged").expect("sentinel");
    symlink(&outside, &backend).expect("replacement symlink");

    port.write(&agent, &path("nested/value.md"), &valid("retained"))
        .await
        .expect("capability write");
    assert_eq!(fs::read(&sentinel).expect("sentinel"), b"unchanged");
    assert!(!outside.join("nested/value.md").exists());
    assert!(
        moved
            .join("memfs")
            .join(agent.as_str())
            .join("memory/nested/value.md")
            .is_file()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn capability_effects_survive_post_validation_swaps() {
    use std::os::unix::fs::symlink;

    fn swap(parent: &std::path::Path, outside: &std::path::Path) {
        let retained = parent.with_extension("retained");
        fs::rename(parent, &retained).expect("retain validated parent");
        symlink(outside, parent).expect("install outside symlink");
    }

    for operation in ["read", "write-existing", "write-absent", "delete", "rename"] {
        let (root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let repository = port.repo(&agent).expect("repo");
        let parent = repository.join("system/race");
        fs::create_dir_all(&parent).expect("race parent");
        let outside = root.0.join(format!("outside-{operation}"));
        fs::create_dir(&outside).expect("outside");
        fs::write(outside.join("sentinel"), b"unchanged").expect("sentinel");
        fs::write(outside.join("source.md"), b"outside-source").expect("outside source");
        fs::write(outside.join("target.md"), b"outside-target").expect("outside target");
        fs::write(parent.join("source.md"), valid("inside").as_slice()).expect("inside");
        let directory = port.repo_dir_for_test(&agent).expect("repo capability");
        let source = path("system/race/source.md");
        let target = path("system/race/target.md");
        let result = match operation {
            "read" => crate::fs::read_file_with_hook(&directory, &source, || {
                swap(&parent, &outside);
            })
            .map(|_| ()),
            "write-existing" => {
                crate::fs::write_file_with_hook(&directory, &source, &valid("replacement"), || {
                    swap(&parent, &outside);
                })
            }
            "write-absent" => {
                crate::fs::write_file_with_hook(&directory, &target, &valid("new"), || {
                    swap(&parent, &outside);
                })
            }
            "delete" => crate::fs::delete_file_with_hook(&directory, &source, || {
                swap(&parent, &outside);
            }),
            "rename" => crate::fs::rename_file_with_hook(&directory, &source, &target, || {
                swap(&parent, &outside);
            }),
            _ => unreachable!(),
        };
        if let Err(error) = result {
            assert!(matches!(
                error,
                RuntimeError::InvalidData { .. }
                    | RuntimeError::NotFound { .. }
                    | RuntimeError::Conflict { .. }
                    | RuntimeError::AdapterFailure { .. }
            ));
        }
        assert_eq!(
            fs::read(outside.join("sentinel")).expect("sentinel"),
            b"unchanged"
        );
        assert_eq!(
            fs::read(outside.join("source.md")).expect("outside source"),
            b"outside-source"
        );
        assert_eq!(
            fs::read(outside.join("target.md")).expect("outside target"),
            b"outside-target"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn initialize_rejects_memfs_symlink_without_touching_outside() {
    use std::os::unix::fs::symlink;

    let root = TestRoot::new();
    let outside = root.0.join("outside");
    fs::create_dir(&outside).expect("outside");
    let sentinel = outside.join("sentinel");
    fs::write(&sentinel, b"unchanged").expect("sentinel");
    symlink(&outside, root.0.join("memfs")).expect("memfs symlink");
    let port = GitMemFs::new(root.0.clone()).expect("adapter");
    assert!(port.initialize(&agent(), &empty()).await.is_err());
    assert_eq!(fs::read(&sentinel).expect("sentinel"), b"unchanged");
}

async fn drain_diff(
    port: &GitMemFs,
    agent: &AgentId,
    source: Option<&RevisionId>,
    target: Option<&RevisionId>,
) -> Vec<u8> {
    let (sender, mut receiver) = mpsc::channel(1);
    let future = port.diff(agent, source, target, sender, CancellationToken::new());
    tokio::pin!(future);
    let mut output = Vec::new();
    loop {
        tokio::select! {
            result = &mut future => {
                result.expect("diff result");
                while let Some(value) = receiver.recv().await {
                    output.extend(value.into_vec());
                }
                return output;
            }
            value = receiver.recv() => if let Some(value) = value {
                output.extend(value.into_vec());
            }
        }
    }
}

async fn blocked_then_cancelled<T>(
    future: PortFuture<'_, ()>,
    mut receiver: mpsc::Receiver<T>,
    token: CancellationToken,
) -> (RuntimeError, Option<T>) {
    tokio::pin!(future);
    let first = timeout(Duration::from_secs(5), async {
        tokio::select! {
            result = &mut future => {
                result.expect("stream remains blocked");
                None
            }
            value = receiver.recv() => value,
        }
    })
    .await
    .expect("first timeout");
    assert!(first.is_some());
    assert!(
        timeout(Duration::from_millis(50), &mut future)
            .await
            .is_err()
    );
    token.cancel();
    let error = timeout(Duration::from_secs(5), &mut future)
        .await
        .expect("cancel timeout")
        .expect_err("cancelled");
    (error, first)
}

async fn assert_pre_cancelled(port: &GitMemFs) {
    let invalid = AgentId::accept("../escaped").expect("opaque agent");
    let unknown = RevisionId::new("f".repeat(40)).expect("unknown");
    let token = CancellationToken::new();
    token.cancel();
    let (tree_sender, _) = mpsc::channel::<MemFsTreeEntry>(1);
    assert!(matches!(
        port.tree(&invalid, Some(&unknown), tree_sender, token.clone())
            .await,
        Err(RuntimeError::Cancelled { .. })
    ));
    let (diff_sender, _) = mpsc::channel::<DiffChunk>(1);
    assert!(matches!(
        port.diff(&invalid, Some(&unknown), None, diff_sender, token.clone())
            .await,
        Err(RuntimeError::Cancelled { .. })
    ));
    let (history_sender, _) = mpsc::channel::<MemFsHistoryEntry>(1);
    assert!(matches!(
        port.history(&invalid, history_sender, token).await,
        Err(RuntimeError::Cancelled { .. })
    ));
}

fn assert_snapshot_symbols_removed() {
    let source = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/repo.rs"))
        .expect("repository source");
    assert!(!source.contains("working_snapshot"));
    assert!(!source.contains("committed_snapshot"));
}

#[tokio::test]
async fn streaming_bounds() {
    let (_root, port, agent) = fixture();
    initialized(&port, &agent).await;
    let deleted = path("system/a-deleted.md");
    let modified = path("system/b-modified.md");
    let unchanged = path("system/c-unchanged.md");
    for (file, value) in [
        (&deleted, "deleted"),
        (&modified, "old"),
        (&unchanged, "same"),
    ] {
        port.write(&agent, file, &valid(value))
            .await
            .expect("write");
    }
    let source = port
        .commit(&agent, &message("source"))
        .await
        .expect("source");
    port.delete(&agent, &deleted).await.expect("delete");
    port.write(&agent, &modified, &valid("new"))
        .await
        .expect("modify");
    let added = path("system/d-added.md");
    let large = "x".repeat(lotta_runtime::bounds::MEMFS_DIFF_CHUNK_BYTES_MAX.value + 1);
    port.write(&agent, &added, &valid(&large))
        .await
        .expect("add");
    let target = port
        .commit(&agent, &message("target"))
        .await
        .expect("target");
    assert_eq!(
        collect_tree(&port, &agent, Some(&target)).await,
        [
            "system/b-modified.md",
            "system/c-unchanged.md",
            "system/d-added.md"
        ]
    );
    assert_eq!(
        collect_tree(&port, &agent, None).await,
        collect_tree(&port, &agent, Some(&target)).await
    );
    let first = drain_diff(&port, &agent, Some(&source), Some(&target)).await;
    let second = drain_diff(&port, &agent, Some(&source), Some(&target)).await;
    assert_eq!(first, second);
    assert!(first.starts_with(b"system/a-deleted.md deleted\n- "));
    assert!(
        first
            .windows(b"system/b-modified.md modified\n".len())
            .any(|part| part == b"system/b-modified.md modified\n")
    );
    assert!(
        !first
            .windows(b"system/c-unchanged.md".len())
            .any(|part| part == b"system/c-unchanged.md")
    );
    assert!(
        first
            .windows(b"system/d-added.md added\n".len())
            .any(|part| part == b"system/d-added.md added\n")
    );

    let tree_token = CancellationToken::new();
    let (tree_sender, tree_receiver) = mpsc::channel::<MemFsTreeEntry>(1);
    let tree = port.tree(&agent, Some(&target), tree_sender, tree_token.clone());
    let (tree_error, _) = blocked_then_cancelled(tree, tree_receiver, tree_token).await;
    assert!(matches!(tree_error, RuntimeError::Cancelled { .. }));

    let diff_token = CancellationToken::new();
    let (diff_sender, diff_receiver) = mpsc::channel::<DiffChunk>(1);
    let diff = port.diff(
        &agent,
        Some(&source),
        Some(&target),
        diff_sender,
        diff_token.clone(),
    );
    let (diff_error, _) = blocked_then_cancelled(diff, diff_receiver, diff_token).await;
    assert!(matches!(diff_error, RuntimeError::Cancelled { .. }));

    assert_pre_cancelled(&port).await;
    assert_snapshot_symbols_removed();
}

#[tokio::test]
async fn memfs_port_contract() {
    let roots = Arc::new(std::sync::Mutex::new(Vec::new()));
    lotta_testkit::contract::memfs_contract(
        {
            let roots = Arc::clone(&roots);
            move || {
                let root = TestRoot::new();
                let adapter = GitMemFs::new(root.0.clone()).expect("adapter");
                roots.lock().expect("roots").push(root);
                async move { adapter }
            }
        },
        agent(),
        empty(),
    )
    .await;
}
