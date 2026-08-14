use lotta_domain::AgentId;
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{
    CommitMessage, DiffChunk, InitialMemoryBlocks, MemoryFileContent, RepositoryPath, RevisionId,
    WorktreeId,
};
use lotta_runtime::ports::{MemFsHistoryEntry, MemFsPort, MemFsStatus, MemFsTreeEntry, PortFuture};
use std::future::Future;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Runs complete memory-filesystem semantics against isolated adapters.
///
/// # Panics
/// Panics when an adapter violates the asserted contract.
pub async fn memfs_contract<F, Fut, Adapter>(
    factory: F,
    agent_id: AgentId,
    blocks: InitialMemoryBlocks,
) where
    F: Fn() -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: MemFsPort,
{
    memfs_initialize(&factory, &agent_id, &blocks).await;
    let port = factory().await;
    let initial = port
        .initialize(&agent_id, &blocks)
        .await
        .expect("initialize");
    let source = RepositoryPath::new("system/new.md".into()).expect("source");
    let target = RepositoryPath::new("system/renamed.md".into()).expect("target");
    let original = MemoryFileContent::new(b"memory".to_vec()).expect("content");
    memfs_status_and_files(&port, &agent_id, &initial, &source, &target, &original).await;
    let committed = memfs_commit_history(&port, &agent_id, &initial, &target, &original).await;
    memfs_tree_revision(&port, &agent_id, &initial, &committed).await;
    memfs_diff_revision(&port, &agent_id, &initial, &committed, &target).await;
    memfs_worktree(&port, &agent_id, &committed).await;
    memfs_stream_failures(&port, &agent_id, &initial, &committed).await;
}

async fn memfs_initialize<F, Fut, Adapter>(
    factory: &F,
    agent: &AgentId,
    blocks: &InitialMemoryBlocks,
) where
    F: Fn() -> Fut,
    Fut: Future<Output = Adapter>,
    Adapter: MemFsPort,
{
    let port = factory().await;
    assert!(matches!(
        port.status(agent).await,
        Err(RuntimeError::NotFound { .. })
    ));
    let empty = InitialMemoryBlocks::new(Vec::new()).expect("empty blocks");
    let first = port
        .initialize(agent, &empty)
        .await
        .expect("empty initialize");
    assert_eq!(first.as_str(), "testkit-revision-0000000000000001");
    assert!(matches!(
        port.initialize(agent, blocks).await,
        Err(RuntimeError::Conflict { .. })
    ));
    assert!(collect_tree_at(&port, agent, None).await.is_empty());
    let other = factory().await;
    let revision = other
        .initialize(agent, blocks)
        .await
        .expect("block initialize");
    assert_eq!(revision.as_str(), "testkit-revision-0000000000000001");
    assert_eq!(
        collect_tree_at(&other, agent, None).await.len(),
        blocks.as_slice().len()
    );
}

async fn memfs_status_and_files(
    port: &impl MemFsPort,
    agent: &AgentId,
    initial: &RevisionId,
    source: &RepositoryPath,
    target: &RepositoryPath,
    original: &MemoryFileContent,
) {
    assert_eq!(
        port.status(agent).await.expect("status"),
        MemFsStatus {
            revision: Some(initial.clone()),
            dirty: false
        }
    );
    assert!(matches!(
        port.read(agent, source).await,
        Err(RuntimeError::NotFound { .. })
    ));
    port.write(agent, source, original).await.expect("write");
    assert_eq!(port.read(agent, source).await.expect("read"), *original);
    let replacement = MemoryFileContent::new(b"replacement".to_vec()).expect("replacement");
    port.write(agent, source, &replacement)
        .await
        .expect("replace");
    assert_eq!(
        port.read(agent, source).await.expect("replacement read"),
        replacement
    );
    port.write(agent, source, original).await.expect("restore");
    assert!(port.status(agent).await.expect("dirty write").dirty);
    port.rename(agent, source, target).await.expect("rename");
    assert!(matches!(
        port.rename(agent, source, target).await,
        Err(RuntimeError::Conflict { .. })
    ));
    assert_eq!(
        port.read(agent, target).await.expect("target retained"),
        *original
    );
    assert!(matches!(
        port.rename(
            agent,
            source,
            &RepositoryPath::new("x.md".into()).expect("x")
        )
        .await,
        Err(RuntimeError::NotFound { .. })
    ));
    port.delete(agent, target).await.expect("delete");
    assert!(matches!(
        port.delete(agent, target).await,
        Err(RuntimeError::NotFound { .. })
    ));
    assert!(
        !port
            .status(agent)
            .await
            .expect("delete restored snapshot")
            .dirty
    );
    port.write(agent, target, original)
        .await
        .expect("restore target");
}

async fn memfs_commit_history(
    port: &impl MemFsPort,
    agent: &AgentId,
    initial: &RevisionId,
    target: &RepositoryPath,
    contents: &MemoryFileContent,
) -> RevisionId {
    let message = CommitMessage::new("contract commit".into()).expect("message");
    let committed = port.commit(agent, &message).await.expect("commit");
    assert_ne!(&committed, initial);
    assert_eq!(committed.as_str(), "testkit-revision-0000000000000002");
    assert_eq!(
        port.status(agent).await.expect("clean"),
        MemFsStatus {
            revision: Some(committed.clone()),
            dirty: false
        }
    );
    let no_change = port
        .commit(
            agent,
            &CommitMessage::new("no change".into()).expect("message"),
        )
        .await
        .expect("no-change commit");
    assert_eq!(no_change.as_str(), "testkit-revision-0000000000000003");
    let history = collect_history(port, agent).await;
    assert_eq!(
        history
            .iter()
            .map(|item| (item.revision.as_str(), item.summary.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("testkit-revision-0000000000000003", "no change"),
            ("testkit-revision-0000000000000002", "contract commit"),
            ("testkit-revision-0000000000000001", "initialize"),
        ]
    );
    assert_eq!(
        port.file_at_revision(agent, target, &committed)
            .await
            .expect("historical"),
        *contents
    );
    port.write(
        agent,
        target,
        &MemoryFileContent::new(b"later".to_vec()).expect("later"),
    )
    .await
    .expect("later write");
    assert_eq!(
        port.file_at_revision(agent, target, &committed)
            .await
            .expect("stable historical"),
        *contents
    );
    assert!(matches!(
        port.file_at_revision(
            agent,
            &RepositoryPath::new("missing.md".into()).expect("missing"),
            &committed
        )
        .await,
        Err(RuntimeError::NotFound { .. })
    ));
    committed
}

async fn memfs_tree_revision(
    port: &impl MemFsPort,
    agent: &AgentId,
    initial: &RevisionId,
    committed: &RevisionId,
) {
    let current = collect_tree_at(port, agent, None).await;
    assert!(current.windows(2).all(|pair| pair[0].path < pair[1].path));
    let initial_tree = collect_tree_at(port, agent, Some(initial)).await;
    assert!(!current.is_empty());
    assert_ne!(current, initial_tree);
    assert!(
        collect_tree_at(port, agent, Some(committed))
            .await
            .iter()
            .any(|entry| entry.path.as_path().ends_with("renamed.md"))
    );
    let unknown = RevisionId::new("unknown".into()).expect("unknown");
    let (sender, _receiver) = mpsc::channel(1);
    assert!(matches!(
        port.tree(agent, Some(&unknown), sender, CancellationToken::new())
            .await,
        Err(RuntimeError::NotFound { .. })
    ));
}

async fn memfs_diff_revision(
    port: &impl MemFsPort,
    agent: &AgentId,
    initial: &RevisionId,
    committed: &RevisionId,
    target: &RepositoryPath,
) {
    let chunks = collect_diff(port, agent, Some(initial), Some(committed)).await;
    assert_eq!(chunks.len(), 1);
    let expected = format!("{} added\n+ memory\n", target.as_path().display());
    assert_eq!(chunks[0].as_slice(), expected.as_bytes());
    assert!(
        collect_diff(port, agent, Some(committed), Some(committed))
            .await
            .is_empty()
    );
    let current = collect_diff(port, agent, Some(committed), None).await;
    assert_eq!(current.len(), 1);
    assert!(current[0].as_slice().ends_with(b"- memory\n+ later\n"));
    let unknown = RevisionId::new("unknown".into()).expect("unknown");
    let (sender, _receiver) = mpsc::channel(1);
    assert!(matches!(
        port.diff(
            agent,
            Some(&unknown),
            None,
            sender,
            CancellationToken::new()
        )
        .await,
        Err(RuntimeError::NotFound { .. })
    ));
}

async fn memfs_worktree(port: &impl MemFsPort, agent: &AgentId, _committed: &RevisionId) {
    let worktree = port.create_worktree(agent).await.expect("worktree");
    assert_eq!(worktree.as_str(), "testkit-worktree-0000000000000001");
    assert!(!worktree.as_str().contains('/'));
    let message = CommitMessage::new("merge snapshot".into()).expect("merge message");
    let merged = port
        .merge_worktree(agent, &worktree, &message)
        .await
        .expect("merge");
    assert_eq!(merged.as_str(), "testkit-revision-0000000000000004");
    assert!(!port.status(agent).await.expect("merged status").dirty);
    assert!(matches!(
        port.merge_worktree(agent, &worktree, &message).await,
        Err(RuntimeError::NotFound { .. })
    ));
    let unknown = WorktreeId::new("unknown".into()).expect("unknown");
    assert!(matches!(
        port.merge_worktree(agent, &unknown, &message).await,
        Err(RuntimeError::NotFound { .. })
    ));
    let history = collect_history(port, agent).await;
    assert_eq!(history[0].summary, message);
    let missing_agent = AgentId::accept("missing-agent").expect("agent");
    assert!(matches!(
        port.create_worktree(&missing_agent).await,
        Err(RuntimeError::NotFound { .. })
    ));
}

async fn memfs_stream_failures(
    port: &impl MemFsPort,
    agent: &AgentId,
    initial: &RevisionId,
    committed: &RevisionId,
) {
    assert_memfs_stream(|sender, token| port.tree(agent, None, sender, token)).await;
    assert_memfs_stream(|sender, token| port.history(agent, sender, token)).await;
    assert_memfs_stream(|sender, token| {
        port.diff(agent, Some(initial), Some(committed), sender, token)
    })
    .await;
}

async fn assert_memfs_stream<'a, T: Send + 'static>(
    run: impl Fn(mpsc::Sender<T>, CancellationToken) -> PortFuture<'a, ()>,
) {
    let (sender, receiver) = mpsc::channel(1);
    drop(receiver);
    assert!(matches!(
        run(sender, CancellationToken::new()).await,
        Err(RuntimeError::AdapterFailure { .. })
    ));
    let token = CancellationToken::new();
    token.cancel();
    let (sender, mut receiver) = mpsc::channel(1);
    assert!(matches!(
        run(sender, token).await,
        Err(RuntimeError::Cancelled { .. })
    ));
    assert!(receiver.try_recv().is_err());
}

async fn collect_tree_at(
    port: &impl MemFsPort,
    agent: &AgentId,
    revision: Option<&RevisionId>,
) -> Vec<MemFsTreeEntry> {
    let (sender, mut receiver) = mpsc::channel(1);
    let listing = port.tree(agent, revision, sender, CancellationToken::new());
    tokio::pin!(listing);
    let mut values = Vec::new();
    loop {
        tokio::select! {
            result = &mut listing => {
                result.expect("tree");
                while let Some(value) = receiver.recv().await { values.push(value); }
                break;
            }
            value = receiver.recv() => if let Some(value) = value { values.push(value); }
        }
    }
    values
}

async fn collect_history(port: &impl MemFsPort, agent: &AgentId) -> Vec<MemFsHistoryEntry> {
    let (sender, mut receiver) = mpsc::channel(1);
    let listing = port.history(agent, sender, CancellationToken::new());
    tokio::pin!(listing);
    let mut values = Vec::new();
    loop {
        tokio::select! {
            result = &mut listing => {
                result.expect("history");
                while let Some(value) = receiver.recv().await { values.push(value); }
                break;
            }
            value = receiver.recv() => if let Some(value) = value { values.push(value); }
        }
    }
    values
}

async fn collect_diff(
    port: &impl MemFsPort,
    agent: &AgentId,
    source: Option<&RevisionId>,
    target: Option<&RevisionId>,
) -> Vec<DiffChunk> {
    let (sender, mut receiver) = mpsc::channel(1);
    let listing = port.diff(agent, source, target, sender, CancellationToken::new());
    tokio::pin!(listing);
    let mut values = Vec::new();
    loop {
        tokio::select! {
            result = &mut listing => {
                result.expect("diff");
                while let Some(value) = receiver.recv().await { values.push(value); }
                break;
            }
            value = receiver.recv() => if let Some(value) = value { values.push(value); }
        }
    }
    values
}
