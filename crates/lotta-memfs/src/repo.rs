//! Repository lifecycle, revision access, and semantic snapshots.

use crate::{fs, git, labels, validation};
use cap_std::fs::Dir;
use lotta_domain::AgentId;
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{InitialMemoryBlocks, MemoryFileContent, RepositoryPath, RevisionId};
use lotta_runtime::ports::{
    MemFsCommitAuthor, MemFsHistoryEntry, MemFsMutation, MemFsTransactionResult,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TRANSACTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const GIT_REVISION_HEX_BYTES: usize = 40;

fn conflict(context: &'static str) -> RuntimeError {
    RuntimeError::Conflict {
        context: context.into(),
    }
}

fn adapter(code: &'static str, context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code,
        context: context.into(),
    }
}

pub(crate) fn validate_agent_id(agent: &AgentId) -> Result<(), RuntimeError> {
    let value = agent.as_str();
    if value.is_empty()
        || value == "."
        || value == ".."
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && byte != b'-' && byte != b'_')
    {
        return Err(RuntimeError::InvalidData {
            context: "local agent identifier".into(),
        });
    }
    Ok(())
}

pub(crate) fn repository_path(root: &Path, agent: &AgentId) -> Result<PathBuf, RuntimeError> {
    validate_agent_id(agent)?;
    Ok(root.join("memfs").join(agent.as_str()).join("memory"))
}

fn sync_directory(directory: &Dir) -> Result<(), RuntimeError> {
    directory
        .try_clone()
        .map(Dir::into_std_file)
        .and_then(|file| file.sync_all())
        .map_err(|_| adapter("memfs_io", "repository parent"))
}

fn open_or_create_directory(parent: &Dir, name: &str) -> Result<Dir, RuntimeError> {
    match parent.symlink_metadata(name) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
        Ok(_) => {
            return Err(RuntimeError::InvalidData {
                context: "repository parent".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => parent
            .create_dir(name)
            .map_err(|_| adapter("memfs_io", "repository parent"))?,
        Err(_) => return Err(adapter("memfs_io", "repository parent")),
    }
    parent
        .open_dir(name)
        .map_err(|_| adapter("memfs_io", "repository parent"))
}

fn transaction_name() -> Result<String, RuntimeError> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| adapter("memfs_io", "repository transaction"))?
        .as_nanos();
    let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(format!(
        ".memory-init-{}-{stamp}-{sequence}",
        std::process::id()
    ))
}

pub(crate) fn initialize(
    root: &Path,
    backend: &Dir,
    agent: &AgentId,
    blocks: &InitialMemoryBlocks,
) -> Result<RevisionId, RuntimeError> {
    validate_agent_id(agent)?;
    let memfs = open_or_create_directory(backend, "memfs")?;
    let agent_dir = open_or_create_directory(&memfs, agent.as_str())?;
    match agent_dir.symlink_metadata("memory") {
        Ok(_) => return Err(conflict("memfs repository")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(adapter("memfs_io", "memfs repository")),
    }
    let transaction_name = transaction_name()?;
    agent_dir
        .create_dir(&transaction_name)
        .map_err(|_| adapter("memfs_io", "repository transaction"))?;
    let transaction_dir = agent_dir
        .open_dir(&transaction_name)
        .map_err(|_| adapter("memfs_io", "repository transaction"))?;
    let transaction_path = root
        .join("memfs")
        .join(agent.as_str())
        .join(&transaction_name);
    let result = initialize_transaction(&transaction_path, &transaction_dir, blocks);
    if let Err(error) = result {
        drop(agent_dir.remove_dir_all(&transaction_name));
        return Err(error);
    }
    if agent_dir.symlink_metadata("memory").is_ok() {
        drop(agent_dir.remove_dir_all(&transaction_name));
        return Err(conflict("memfs repository"));
    }
    if Dir::rename(&agent_dir, &transaction_name, &agent_dir, "memory").is_err() {
        drop(agent_dir.remove_dir_all(&transaction_name));
        return Err(conflict("memfs repository"));
    }
    sync_directory(&agent_dir)?;
    result
}

fn initialize_transaction(
    repo: &Path,
    directory: &Dir,
    blocks: &InitialMemoryBlocks,
) -> Result<RevisionId, RuntimeError> {
    git::checked(repo, ["init", "-b", "main"], "git initialize")?;
    for name in ["system", "skills", "mods"] {
        directory
            .create_dir(name)
            .map_err(|_| adapter("memfs_io", "memory layout"))?;
    }
    let mut seen = BTreeSet::new();
    for block in blocks.as_slice() {
        let path = labels::normalize_label(block.label().as_str())?;
        if !seen.insert(path.clone()) {
            return Err(conflict("normalized memory label"));
        }
        let rendered = labels::render_block(block)?;
        let contents = MemoryFileContent::new(rendered)?;
        fs::write_file(directory, &path, &contents)?;
    }
    git::checked(repo, ["add", "--all", "--"], "git stage")?;
    commit_staged(repo, "initialize", "refs/heads/main", true)
}

pub(crate) fn head(repo: &Path) -> Result<RevisionId, RuntimeError> {
    let output = git::checked(repo, ["rev-parse", "--verify", "HEAD"], "git head")?;
    revision_from_output(&output)
}

pub(crate) fn revision_from_output(output: &[u8]) -> Result<RevisionId, RuntimeError> {
    let text = std::str::from_utf8(output).map_err(|_| RuntimeError::InvalidData {
        context: "git revision".into(),
    })?;
    let revision = text.trim();
    validate_revision_text(revision)?;
    RevisionId::new(revision.to_owned())
}

pub(crate) fn validate_revision_text(value: &str) -> Result<(), RuntimeError> {
    if value.len() != GIT_REVISION_HEX_BYTES || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RuntimeError::NotFound {
            context: "memfs revision".into(),
        });
    }
    Ok(())
}

pub(crate) fn resolve_revision(repo: &Path, revision: &RevisionId) -> Result<String, RuntimeError> {
    validate_revision_text(revision.as_str())?;
    let expression = format!("{}^{{commit}}", revision.as_str());
    let output = git::run(
        repo,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            expression.as_str(),
        ],
        "git revision",
    )?;
    if !output.status.success() {
        return Err(RuntimeError::NotFound {
            context: "memfs revision".into(),
        });
    }
    Ok(revision_from_output(&output.stdout)?.into_string())
}

#[derive(Clone)]
pub(crate) enum Snapshot {
    Working,
    Committed(String),
}

pub(crate) fn snapshot_descriptor(
    repo: &Path,
    revision: Option<&RevisionId>,
) -> Result<Snapshot, RuntimeError> {
    match revision {
        None => Ok(Snapshot::Working),
        Some(value) => Ok(Snapshot::Committed(resolve_revision(repo, value)?)),
    }
}

pub(crate) fn paths(
    repo: &Path,
    directory: &Dir,
    revision: Option<&RevisionId>,
) -> Result<Vec<RepositoryPath>, RuntimeError> {
    let snapshot = snapshot_descriptor(repo, revision)?;
    snapshot_paths(repo, directory, &snapshot)
}

pub(crate) fn snapshot_paths(
    repo: &Path,
    directory: &Dir,
    snapshot: &Snapshot,
) -> Result<Vec<RepositoryPath>, RuntimeError> {
    match snapshot {
        Snapshot::Working => fs::scan_working(directory),
        Snapshot::Committed(revision) => committed_paths(repo, revision),
    }
}

fn committed_paths(repo: &Path, revision: &str) -> Result<Vec<RepositoryPath>, RuntimeError> {
    let output = git::checked(
        repo,
        ["ls-tree", "-rz", "--name-only", "--full-tree", revision],
        "git tree",
    )?;
    let mut paths = Vec::new();
    for raw in output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
    {
        let text = std::str::from_utf8(raw).map_err(|_| RuntimeError::InvalidData {
            context: "git tree path".into(),
        })?;
        let path = RepositoryPath::new(text.into())?;
        fs::validate_repository_path(&path)?;
        paths
            .try_reserve(1)
            .map_err(|_| RuntimeError::LimitExceeded {
                context: crate::MEMORY_FILES_MAX.name.into(),
            })?;
        paths.push(path);
        fs::validate_file_count(paths.len())?;
    }
    paths.sort();
    Ok(paths)
}

pub(crate) fn file_in_snapshot(
    repo: &Path,
    directory: &Dir,
    snapshot: &Snapshot,
    path: &RepositoryPath,
) -> Result<Option<MemoryFileContent>, RuntimeError> {
    match snapshot {
        Snapshot::Working => match fs::read_file(directory, path) {
            Ok(value) => Ok(Some(value)),
            Err(RuntimeError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        },
        Snapshot::Committed(revision) => file_at_revision_resolved_optional(repo, path, revision),
    }
}

pub(crate) fn file_at_revision(
    repo: &Path,
    path: &RepositoryPath,
    revision: &RevisionId,
) -> Result<MemoryFileContent, RuntimeError> {
    fs::validate_repository_path(path)?;
    let resolved = resolve_revision(repo, revision)?;
    file_at_revision_resolved(repo, path, &resolved)
}

fn file_at_revision_resolved(
    repo: &Path,
    path: &RepositoryPath,
    revision: &str,
) -> Result<MemoryFileContent, RuntimeError> {
    file_at_revision_resolved_optional(repo, path, revision)?.ok_or_else(|| {
        RuntimeError::NotFound {
            context: "memory revision file".into(),
        }
    })
}

fn file_at_revision_resolved_optional(
    repo: &Path,
    path: &RepositoryPath,
    revision: &str,
) -> Result<Option<MemoryFileContent>, RuntimeError> {
    let text = path
        .as_path()
        .to_str()
        .ok_or_else(|| RuntimeError::InvalidData {
            context: "git tree path".into(),
        })?;
    let spec = format!("{revision}:{text}");
    let output = git::run(repo, ["show", "--no-textconv", spec.as_str()], "git file")?;
    if !output.status.success() {
        return Ok(None);
    }
    fs::validate_file_bytes(output.stdout.len())?;
    Ok(Some(MemoryFileContent::new(output.stdout)?))
}

pub(crate) fn commit(repo: &Path, message: &str) -> Result<RevisionId, RuntimeError> {
    git::checked(repo, ["add", "--all", "--"], "git stage")?;
    let branch = symbolic_branch(repo)?;
    let revision = commit_staged(repo, message, &branch, true)?;
    post_commit_push(repo);
    Ok(revision)
}

pub(crate) fn transact(
    repo: &Path,
    directory: &Dir,
    mutations: &[MemFsMutation],
    message: &str,
    author: &MemFsCommitAuthor,
) -> Result<MemFsTransactionResult, RuntimeError> {
    ensure_clean(repo)?;
    let snapshots = snapshot_mutations(directory, mutations)?;
    let result = apply_and_commit(repo, directory, mutations, message, author);
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            rollback_transaction(repo, directory, &snapshots)?;
            Err(error)
        }
    }
}

fn ensure_clean(repo: &Path) -> Result<(), RuntimeError> {
    let status = git::checked(repo, ["status", "--porcelain=v1", "-z"], "git status")?;
    if status.is_empty() {
        Ok(())
    } else {
        Err(conflict("dirty memory repository"))
    }
}

fn snapshot_mutations(
    directory: &Dir,
    mutations: &[MemFsMutation],
) -> Result<BTreeMap<RepositoryPath, Option<MemoryFileContent>>, RuntimeError> {
    let mut snapshots = BTreeMap::new();
    for mutation in mutations {
        let paths: &[RepositoryPath] = match mutation {
            MemFsMutation::Write { path, .. } | MemFsMutation::Delete { path } => {
                std::slice::from_ref(path)
            }
            MemFsMutation::Rename { source, target } => {
                for path in [source, target] {
                    snapshot_path(directory, path, &mut snapshots)?;
                }
                continue;
            }
        };
        snapshot_path(directory, &paths[0], &mut snapshots)?;
    }
    Ok(snapshots)
}

fn snapshot_path(
    directory: &Dir,
    path: &RepositoryPath,
    snapshots: &mut BTreeMap<RepositoryPath, Option<MemoryFileContent>>,
) -> Result<(), RuntimeError> {
    if snapshots.contains_key(path) {
        return Ok(());
    }
    let value = match fs::read_file(directory, path) {
        Ok(value) => Some(value),
        Err(RuntimeError::NotFound { .. }) => None,
        Err(error) => return Err(error),
    };
    snapshots.insert(path.clone(), value);
    Ok(())
}

fn apply_and_commit(
    repo: &Path,
    directory: &Dir,
    mutations: &[MemFsMutation],
    message: &str,
    author: &MemFsCommitAuthor,
) -> Result<MemFsTransactionResult, RuntimeError> {
    for mutation in mutations {
        match mutation {
            MemFsMutation::Write { path, contents } => fs::write_file(directory, path, contents)?,
            MemFsMutation::Delete { path } => fs::delete_file(directory, path)?,
            MemFsMutation::Rename { source, target } => {
                fs::rename_file(directory, source, target)?;
            }
        }
    }
    git::checked(repo, ["add", "--all", "--"], "git stage")?;
    if git::checked(repo, ["diff", "--cached", "--quiet"], "git staged diff").is_ok() {
        return Ok(MemFsTransactionResult::NoChange);
    }
    let branch = symbolic_branch(repo)?;
    let revision = commit_staged_as(repo, message, &branch, true, author)?;
    ensure_clean(repo)?;
    post_commit_push(repo);
    Ok(MemFsTransactionResult::Committed(revision))
}

fn rollback_transaction(
    repo: &Path,
    directory: &Dir,
    snapshots: &BTreeMap<RepositoryPath, Option<MemoryFileContent>>,
) -> Result<(), RuntimeError> {
    for (path, value) in snapshots {
        match value {
            Some(contents) => fs::write_file(directory, path, contents)?,
            None => match fs::delete_file(directory, path) {
                Ok(()) | Err(RuntimeError::NotFound { .. }) => (),
                Err(error) => return Err(error),
            },
        }
    }
    git::checked(
        repo,
        ["reset", "--mixed", "HEAD", "--"],
        "git rollback index",
    )?;
    ensure_clean(repo)
}

fn symbolic_branch(repo: &Path) -> Result<String, RuntimeError> {
    let output = git::checked(
        repo,
        ["symbolic-ref", "--quiet", "HEAD"],
        "git symbolic branch",
    )?;
    let branch = std::str::from_utf8(&output).map_err(|_| RuntimeError::InvalidData {
        context: "git branch reference".into(),
    })?;
    let branch = branch.trim();
    if !branch.starts_with("refs/heads/") || branch.contains(char::is_whitespace) {
        return Err(RuntimeError::InvalidData {
            context: "git branch reference".into(),
        });
    }
    Ok(branch.to_owned())
}

fn optional_head(repo: &Path) -> Result<Option<RevisionId>, RuntimeError> {
    let output = git::run(repo, ["rev-parse", "--verify", "HEAD"], "git head")?;
    if output.status.success() {
        return revision_from_output(&output.stdout).map(Some);
    }
    Ok(None)
}

fn staged_tree(repo: &Path) -> Result<RevisionId, RuntimeError> {
    let output = git::checked(repo, ["write-tree"], "git staged tree")?;
    revision_from_output(&output)
}

pub(crate) fn commit_staged(
    repo: &Path,
    message: &str,
    branch: &str,
    validate: bool,
) -> Result<RevisionId, RuntimeError> {
    let author = MemFsCommitAuthor {
        name: lotta_runtime::boundary::CommitMessage::new("Letta Agent".into())?,
        email: lotta_runtime::boundary::CommitMessage::new("lotta-memfs@localhost".into())?,
    };
    commit_staged_as(repo, message, branch, validate, &author)
}

fn commit_staged_as(
    repo: &Path,
    message: &str,
    branch: &str,
    validate: bool,
    author: &MemFsCommitAuthor,
) -> Result<RevisionId, RuntimeError> {
    let tree = staged_tree(repo)?;
    if validate {
        validation::validate_staged(repo)?;
        if staged_tree(repo)? != tree {
            return Err(conflict("staged tree changed during validation"));
        }
    }
    let old = optional_head(repo)?;
    let mut args = Vec::new();
    let name = format!("user.name={}", author.name.as_str());
    let email = format!("user.email={}", author.email.as_str());
    args.extend([
        "-c",
        name.as_str(),
        "-c",
        email.as_str(),
        "-c",
        "commit.gpgSign=false",
        "-c",
        "core.hooksPath=/dev/null",
    ]);
    args.extend(["commit-tree", tree.as_str()]);
    if let Some(parent) = old.as_ref() {
        args.extend(["-p", parent.as_str()]);
    }
    args.extend(["-m", message]);
    let output = git::checked(repo, args, "git commit tree")?;
    let revision = revision_from_output(&output)?;
    update_branch(repo, branch, &revision, old.as_ref())?;
    Ok(revision)
}

fn update_branch(
    repo: &Path,
    branch: &str,
    revision: &RevisionId,
    old: Option<&RevisionId>,
) -> Result<(), RuntimeError> {
    let mut args = vec!["update-ref", branch, revision.as_str()];
    if let Some(expected) = old {
        args.push(expected.as_str());
    } else {
        args.push("");
    }
    let output = git::run(repo, args, "git update branch")?;
    if !output.status.success() {
        return Err(conflict("concurrent repository commit"));
    }
    Ok(())
}

pub(crate) fn post_commit_push(repo: &Path) {
    if let Err(error) = try_post_commit_push(repo) {
        // Warn-and-continue: a failed mirror push never undoes the commit.
        tracing::warn!(error = %error, "memory post-commit push failed");
    }
}

pub(crate) fn try_post_commit_push(repo: &Path) -> Result<(), RuntimeError> {
    let configured = git::run(
        repo,
        ["config", "--local", "--get", "letta.memoryRepository.url"],
        "git memory repository",
    )?;
    if !configured.status.success() {
        return Ok(());
    }
    let url = std::str::from_utf8(&configured.stdout)
        .map_err(|_| RuntimeError::InvalidData {
            context: "memory repository URL".into(),
        })?
        .trim();
    if url.is_empty() {
        return Ok(());
    }
    git::checked(repo, ["push", url, "main:main"], "git post-commit push")?;
    Ok(())
}

pub(crate) fn history(repo: &Path) -> Result<Vec<MemFsHistoryEntry>, RuntimeError> {
    let output = git::checked(
        repo,
        ["log", "--format=%H%x00%s%x00", "--max-count=100000"],
        "git history",
    )?;
    let mut fields = output
        .split(|byte| *byte == 0)
        .map(|part| match part.strip_prefix(b"\n") {
            Some(value) => value,
            None => part,
        })
        .filter(|part| !part.is_empty());
    let mut values = Vec::new();
    while let Some(revision) = fields.next() {
        let summary = fields.next().ok_or_else(|| RuntimeError::InvalidData {
            context: "git history fields".into(),
        })?;
        let revision = revision_from_output(revision)?;
        let summary = std::str::from_utf8(summary).map_err(|_| RuntimeError::InvalidData {
            context: "git history summary".into(),
        })?;
        values
            .try_reserve(1)
            .map_err(|_| RuntimeError::LimitExceeded {
                context: crate::MEMORY_FILES_MAX.name.into(),
            })?;
        values.push(MemFsHistoryEntry {
            revision,
            summary: lotta_runtime::boundary::CommitMessage::new(summary.to_owned())?,
        });
        fs::validate_file_count(values.len())?;
    }
    Ok(values)
}
