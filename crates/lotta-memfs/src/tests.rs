use crate::GitMemFs;
use lotta_domain::{AgentId, MemoryBlockInput, NonEmptyString};
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{
    CommitMessage, InitialMemoryBlock, InitialMemoryBlocks, MemoryFileContent, RepositoryPath,
    RevisionId,
};
use lotta_runtime::ports::MemFsPort;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

static TEST_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);
impl TestRoot {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = TEST_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "lotta-memfs-test-{}-{stamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("test root");
        Self(path)
    }
}
impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn agent() -> AgentId {
    AgentId::accept("agent-local-test").expect("agent")
}
fn empty() -> InitialMemoryBlocks {
    InitialMemoryBlocks::new(Vec::new()).expect("blocks")
}
fn path(value: &str) -> RepositoryPath {
    RepositoryPath::new(value.into()).expect("path")
}
fn content(value: &str) -> MemoryFileContent {
    MemoryFileContent::new(value.as_bytes().to_vec()).expect("content")
}
fn valid(value: &str) -> MemoryFileContent {
    content(&format!("---\ndescription: test\n---\n{value}"))
}
fn message(value: &str) -> CommitMessage {
    CommitMessage::new(value.into()).expect("message")
}
fn fixture() -> (TestRoot, GitMemFs, AgentId) {
    let root = TestRoot::new();
    let port = GitMemFs::new(root.0.clone()).expect("adapter");
    (root, port, agent())
}
async fn initialized(port: &GitMemFs, agent: &AgentId) -> RevisionId {
    port.initialize(agent, &empty()).await.expect("initialize")
}

mod repo;

async fn collect_tree(
    port: &GitMemFs,
    agent: &AgentId,
    revision: Option<&RevisionId>,
) -> Vec<String> {
    let (sender, mut receiver) = mpsc::channel(1);
    let future = port.tree(agent, revision, sender, CancellationToken::new());
    tokio::pin!(future);
    let mut values = Vec::new();
    loop {
        tokio::select! {
            result = &mut future => {
                result.expect("tree");
                while let Some(value) = receiver.recv().await {
                    values.push(value.path.as_path().to_string_lossy().into_owned());
                }
                return values;
            }
            value = receiver.recv() => if let Some(value) = value {
                values.push(value.path.as_path().to_string_lossy().into_owned());
            }
        }
    }
}

mod ops {
    use super::*;

    #[tokio::test]
    async fn status() {
        let (_root, port, agent) = fixture();
        let revision = initialized(&port, &agent).await;
        let status = port.status(&agent).await.expect("status");
        assert_eq!(status.revision, Some(revision));
        assert!(!status.dirty);
    }

    #[tokio::test]
    async fn tree() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        port.write(&agent, &path("system/b.md"), &valid("b"))
            .await
            .expect("write");
        port.write(&agent, &path("system/a.md"), &valid("a"))
            .await
            .expect("write");
        assert_eq!(
            collect_tree(&port, &agent, None).await,
            ["system/a.md", "system/b.md"]
        );
    }

    #[tokio::test]
    async fn read() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let path = path("system/a.md");
        port.write(&agent, &path, &valid("a")).await.expect("write");
        assert_eq!(port.read(&agent, &path).await.expect("read"), valid("a"));
    }

    #[tokio::test]
    async fn write() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        port.write(&agent, &path("system/a.md"), &valid("a"))
            .await
            .expect("write");
        assert!(port.status(&agent).await.expect("status").dirty);
    }

    #[tokio::test]
    async fn delete() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let file = path("system/a.md");
        port.write(&agent, &file, &valid("a")).await.expect("write");
        port.delete(&agent, &file).await.expect("delete");
        assert!(matches!(
            port.read(&agent, &file).await,
            Err(RuntimeError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn rename() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let source = path("system/a.md");
        let target = path("system/b.md");
        port.write(&agent, &source, &valid("a"))
            .await
            .expect("write");
        port.rename(&agent, &source, &target).await.expect("rename");
        assert_eq!(port.read(&agent, &target).await.expect("read"), valid("a"));
    }

    #[tokio::test]
    async fn history() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        port.commit(&agent, &message("second"))
            .await
            .expect("commit");
        let repo = port.repo(&agent).expect("repo");
        let history = crate::repo::history(&repo).expect("history");
        assert_eq!(history[0].summary.as_str(), "second");
    }

    #[tokio::test]
    async fn file_at_revision() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let file = path("system/a.md");
        port.write(&agent, &file, &valid("one"))
            .await
            .expect("write");
        let revision = port.commit(&agent, &message("one")).await.expect("commit");
        port.write(&agent, &file, &valid("two"))
            .await
            .expect("write");
        assert_eq!(
            port.file_at_revision(&agent, &file, &revision)
                .await
                .expect("old"),
            valid("one")
        );
    }

    #[tokio::test]
    async fn diff() {
        let (_root, port, agent) = fixture();
        let initial = initialized(&port, &agent).await;
        port.write(&agent, &path("system/a.md"), &valid("one"))
            .await
            .expect("write");
        let committed = port.commit(&agent, &message("one")).await.expect("commit");
        let (sender, mut receiver) = mpsc::channel(1);
        let listing = port.diff(
            &agent,
            Some(&initial),
            Some(&committed),
            sender,
            CancellationToken::new(),
        );
        tokio::pin!(listing);
        let mut output = Vec::new();
        loop {
            tokio::select! {
                result = &mut listing => { result.expect("diff"); break; }
                value = receiver.recv() => if let Some(value) = value {
                    output.extend(value.into_vec());
                }
            }
        }
        while let Some(value) = receiver.recv().await {
            output.extend(value.into_vec());
        }
        assert!(output.starts_with(b"system/a.md added\n+ "));
    }

    #[tokio::test]
    async fn commit() {
        let (_root, port, agent) = fixture();
        let initial = initialized(&port, &agent).await;
        let revision = port
            .commit(&agent, &message("empty"))
            .await
            .expect("commit");
        assert_ne!(revision, initial);
        let repo = port.repo(&agent).expect("repo");
        let staged = crate::git::checked(&repo, ["write-tree"], "staged tree").expect("tree");
        let committed = crate::git::checked(&repo, ["rev-parse", "HEAD^{tree}"], "head tree")
            .expect("head tree");
        assert_eq!(staged, committed);
    }

    #[tokio::test]
    async fn configured_push_failure_keeps_local_commit() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let repo = port.repo(&agent).expect("repo");
        crate::git::checked(
            &repo,
            [
                "config",
                "--local",
                "letta.memoryRepository.url",
                "/definitely/missing/lotta-remote.git",
            ],
            "config",
        )
        .expect("config");
        let revision = port
            .commit(&agent, &message("local despite push"))
            .await
            .expect("local commit");
        assert_eq!(
            port.status(&agent).await.expect("status").revision,
            Some(revision)
        );
    }

    #[tokio::test]
    async fn create_worktree() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let id = port.create_worktree(&agent).await.expect("worktree");
        assert!(!id.as_str().contains('/'));
        let repo = port.repo(&agent).expect("repo");
        let expected = repo
            .parent()
            .expect("agent directory")
            .join("memory-worktrees")
            .join(id.as_str());
        assert_eq!(port.worktree_path(&id).expect("path"), expected);
        assert!(expected.is_dir());
    }

    #[tokio::test]
    async fn merge_worktree() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let id = port.create_worktree(&agent).await.expect("worktree");
        let location = port.worktree_path(&id).expect("path");
        fs::create_dir_all(location.join("system")).expect("system directory");
        fs::write(
            location.join("system/reflected.md"),
            valid("reflection").as_slice(),
        )
        .expect("edit");
        let revision = port
            .merge_worktree(&agent, &id, &message("merge"))
            .await
            .expect("merge");
        assert_eq!(
            port.status(&agent).await.expect("status").revision,
            Some(revision)
        );
        assert!(
            port.read(&agent, &path("system/reflected.md"))
                .await
                .is_ok()
        );
        assert!(matches!(
            port.merge_worktree(&agent, &id, &message("again")).await,
            Err(RuntimeError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn wrong_agent_and_dirty_main_retain_worktree() {
        let (_root, port, owner) = fixture();
        initialized(&port, &owner).await;
        let other = AgentId::accept("agent-other").expect("agent");
        initialized(&port, &other).await;
        let id = port.create_worktree(&owner).await.expect("worktree");
        let location = port.worktree_path(&id).expect("path");
        fs::write(
            location.join("reflection.md"),
            valid("reflection").as_slice(),
        )
        .expect("reflection");
        assert!(matches!(
            port.merge_worktree(&other, &id, &message("wrong")).await,
            Err(RuntimeError::NotFound { .. })
        ));
        let reflection_head =
            crate::git::checked(&location, ["rev-parse", "HEAD"], "head").expect("head");
        port.write(&owner, &path("system/dirty.md"), &valid("dirty"))
            .await
            .expect("dirty");
        assert!(matches!(
            port.merge_worktree(&owner, &id, &message("dirty")).await,
            Err(RuntimeError::Conflict { .. })
        ));
        assert_eq!(
            crate::git::checked(&location, ["rev-parse", "HEAD"], "head").expect("head"),
            reflection_head
        );
        port.delete(&owner, &path("system/dirty.md"))
            .await
            .expect("clean");
        port.merge_worktree(&owner, &id, &message("retry"))
            .await
            .expect("retry");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn worktree_root_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let (root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let repo = port.repo(&agent).expect("repo");
        let outside = root.0.join("outside-worktrees");
        fs::create_dir(&outside).expect("outside");
        let sentinel = outside.join("sentinel");
        fs::write(&sentinel, b"unchanged").expect("sentinel");
        symlink(
            &outside,
            repo.parent().expect("agent").join("memory-worktrees"),
        )
        .expect("symlink");
        assert!(port.create_worktree(&agent).await.is_err());
        assert_eq!(fs::read(&sentinel).expect("sentinel"), b"unchanged");
    }

    #[tokio::test]
    async fn post_commit_push() {
        let root = TestRoot::new();
        let bare = root.0.join("remote.git");
        fs::create_dir(&bare).expect("bare");
        let bare = fs::canonicalize(&bare).expect("canonical bare");
        crate::git::checked(&bare, ["init", "--bare"], "bare").expect("bare init");
        let port = GitMemFs::new(root.0.clone()).expect("adapter");
        let agent = agent();
        initialized(&port, &agent).await;
        let repo = port.repo(&agent).expect("repo");
        crate::git::checked(
            &repo,
            [
                "config",
                "--local",
                "letta.memoryRepository.url",
                bare.to_str().expect("utf8"),
            ],
            "config",
        )
        .expect("config");
        let revision = port.commit(&agent, &message("push")).await.expect("commit");
        let remote = crate::git::checked(&bare, ["rev-parse", "refs/heads/main"], "remote head")
            .expect("remote");
        assert_eq!(
            std::str::from_utf8(&remote).expect("utf8").trim(),
            revision.as_str()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repository_and_nested_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let (root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let repo = port.repo(&agent).expect("repo");
        let outside = root.0.join("outside");
        fs::create_dir(&outside).expect("outside");
        let sentinel = outside.join("sentinel.md");
        fs::write(&sentinel, b"unchanged").expect("sentinel");
        let nested = repo.join("system/escape.md");
        symlink(&sentinel, &nested).expect("nested symlink");
        let escaped = path("system/escape.md");
        assert!(port.read(&agent, &escaped).await.is_err());
        assert!(port.write(&agent, &escaped, &valid("write")).await.is_err());
        assert!(port.delete(&agent, &escaped).await.is_err());
        assert!(
            port.rename(&agent, &escaped, &path("system/moved.md"))
                .await
                .is_err()
        );
        assert!(
            port.tree(&agent, None, mpsc::channel(1).0, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(fs::read(&sentinel).expect("sentinel"), b"unchanged");
        fs::remove_file(&nested).expect("remove symlink");
        let moved = root.0.join("moved-memory");
        fs::rename(&repo, &moved).expect("move repo");
        symlink(&moved, &repo).expect("repo symlink");
        assert!(port.status(&agent).await.is_err());
        assert_eq!(fs::read(&sentinel).expect("sentinel"), b"unchanged");
    }

    #[cfg(unix)]
    #[test]
    fn git_rejects_direct_and_parent_symlink_current_dir() {
        use std::os::unix::fs::symlink;

        let root = TestRoot::new();
        let real = root.0.join("real");
        fs::create_dir(&real).expect("real");
        let direct = root.0.join("direct");
        symlink(&real, &direct).expect("direct");
        assert!(crate::git::checked(&direct, ["--version"], "git").is_err());
        let child = real.join("child");
        fs::create_dir(&child).expect("child");
        let alias = root.0.join("alias");
        symlink(&real, &alias).expect("alias");
        assert!(crate::git::checked(&alias.join("child"), ["--version"], "git").is_err());
    }

    #[tokio::test]
    async fn internal_git_path_is_rejected() {
        let (_root, port, agent) = fixture();
        initialized(&port, &agent).await;
        assert!(port.read(&agent, &path(".git/config")).await.is_err());
        assert!(
            port.write(&agent, &path(".git/config"), &valid("bad"))
                .await
                .is_err()
        );
    }

    fn staged_names(port: &GitMemFs, agent: &AgentId) -> Vec<String> {
        let repo = port.repo(agent).expect("repo");
        let output = crate::git::checked(
            &repo,
            ["diff", "--cached", "--name-only", "-z"],
            "staged names",
        )
        .expect("staged names");
        output
            .split(|byte| *byte == 0)
            .filter(|value| !value.is_empty())
            .map(|value| String::from_utf8(value.to_vec()).expect("UTF-8 path"))
            .collect()
    }

    async fn assert_rejected_new(relative: &str, bytes: &str) {
        let (_root, port, agent) = fixture();
        let before = initialized(&port, &agent).await;
        let file = path(relative);
        let expected = content(bytes);
        port.write(&agent, &file, &expected).await.expect("write");
        assert!(matches!(
            port.commit(&agent, &message("invalid")).await,
            Err(RuntimeError::InvalidData { .. })
        ));
        let status = port.status(&agent).await.expect("status");
        assert_eq!(status.revision, Some(before));
        assert!(status.dirty);
        assert_eq!(port.read(&agent, &file).await.expect("retained"), expected);
        assert!(staged_names(&port, &agent).contains(&relative.to_owned()));
    }

    async fn assert_valid_new(relative: &str, bytes: &str) {
        let (_root, port, agent) = fixture();
        let before = initialized(&port, &agent).await;
        let file = path(relative);
        port.write(&agent, &file, &content(bytes))
            .await
            .expect("write");
        let revision = port
            .commit(&agent, &message("valid"))
            .await
            .expect("commit");
        assert_ne!(revision, before);
        assert!(!port.status(&agent).await.expect("status").dirty);
    }

    async fn assert_delete_compatible() {
        let (_root, port, agent) = fixture();
        let file = path("notes/ordinary.txt");
        initialized(&port, &agent).await;
        port.write(&agent, &file, &content("ordinary"))
            .await
            .expect("write");
        let before = port.commit(&agent, &message("add")).await.expect("add");
        port.delete(&agent, &file).await.expect("delete");
        let revision = port
            .commit(&agent, &message("delete"))
            .await
            .expect("delete commit");
        assert_ne!(revision, before);
        assert!(!port.status(&agent).await.expect("status").dirty);
    }

    async fn protected_fixture(
        read_only: bool,
    ) -> (TestRoot, GitMemFs, AgentId, RevisionId, String) {
        let (root, port, agent) = fixture();
        initialized(&port, &agent).await;
        let relative = "system/protected.md";
        let original =
            format!("---\ndescription: protected\nread_only: {read_only}\n---\nserver body");
        let repo = port.repo(&agent).expect("repo");
        fs::create_dir_all(repo.join("system")).expect("system directory");
        fs::write(repo.join(relative), original.as_bytes()).expect("server file");
        crate::git::checked(&repo, ["add", "--", relative], "seed stage").expect("seed stage");
        let revision = crate::repo::commit_staged(&repo, "server origin", "refs/heads/main", false)
            .expect("server-origin seed");
        (root, port, agent, revision, original)
    }

    async fn assert_protected_rejection(read_only: bool, operation: Option<&str>) {
        let (_root, port, agent, before, original) = protected_fixture(read_only).await;
        let relative = "system/protected.md";
        let file = path(relative);
        match operation {
            Some(bytes) => port
                .write(&agent, &file, &content(bytes))
                .await
                .expect("write"),
            None => port.delete(&agent, &file).await.expect("delete"),
        }
        assert!(matches!(
            port.commit(&agent, &message("protected rejection")).await,
            Err(RuntimeError::InvalidData { .. })
        ));
        let status = port.status(&agent).await.expect("status");
        assert_eq!(status.revision, Some(before.clone()));
        assert!(status.dirty);
        assert!(staged_names(&port, &agent).contains(&relative.to_owned()));
        if let Some(bytes) = operation {
            assert_eq!(
                port.read(&agent, &file).await.expect("retained"),
                content(bytes)
            );
        } else {
            assert!(matches!(
                port.read(&agent, &file).await,
                Err(RuntimeError::NotFound { .. })
            ));
        }
        assert_eq!(
            port.file_at_revision(&agent, &file, &before)
                .await
                .expect("historical file"),
            content(&original)
        );
    }

    async fn assert_mutable_protected_commits() {
        let (_root, port, agent, before, _original) = protected_fixture(false).await;
        let file = path("system/protected.md");
        let changed = content("---\ndescription: protected\nread_only: false\n---\nchanged body");
        port.write(&agent, &file, &changed).await.expect("write");
        let revision = port
            .commit(&agent, &message("mutable protected"))
            .await
            .expect("commit");
        assert_ne!(revision, before);
        assert_eq!(port.read(&agent, &file).await.expect("changed"), changed);
    }

    #[tokio::test]
    async fn pre_commit_rejects_invalid_markdown() {
        for (name, relative, bytes) in [
            (
                "no frontmatter",
                "system/no-frontmatter.md",
                "no frontmatter",
            ),
            (
                "unclosed",
                "system/unclosed.md",
                "---\ndescription: test\nbody",
            ),
            ("missing description", "system/missing.md", "---\n---\nbody"),
            (
                "empty plain",
                "system/empty-plain.md",
                "---\ndescription:\n---\nbody",
            ),
            (
                "empty quoted",
                "system/empty-quoted.md",
                "---\ndescription: \"\"\n---\nbody",
            ),
            (
                "unknown key",
                "system/unknown.md",
                "---\ndescription: test\nunknown: value\n---\nbody",
            ),
            (
                "duplicate",
                "system/duplicate.md",
                "---\ndescription: one\ndescription: two\n---\nbody",
            ),
            (
                "malformed scalar",
                "system/malformed.md",
                "---\ndescription: \"bad\n---\nbody",
            ),
            (
                "flat skill",
                "skills/flat.md",
                "---\ndescription: test\n---\nbody",
            ),
            (
                "new read only false",
                "system/new-read-only.md",
                "---\ndescription: test\nread_only: false\n---\nbody",
            ),
        ] {
            let _ = name;
            assert_rejected_new(relative, bytes).await;
        }
        assert_valid_new("skills/name/SKILL.md", "---\ndescription: test\n---\nbody").await;
        assert_valid_new(
            "system/legacy.md",
            "---\ndescription: test\nlimit: 1\n---\nbody",
        )
        .await;
        assert_delete_compatible().await;
        assert_mutable_protected_commits().await;
        assert_protected_rejection(
            false,
            Some("---\ndescription: protected\nread_only: true\n---\nserver body"),
        )
        .await;
        assert_protected_rejection(false, Some("---\ndescription: protected\n---\nserver body"))
            .await;
        assert_protected_rejection(
            true,
            Some("---\ndescription: protected\nread_only: true\n---\nchanged body"),
        )
        .await;
        assert_protected_rejection(true, None).await;
    }
}
