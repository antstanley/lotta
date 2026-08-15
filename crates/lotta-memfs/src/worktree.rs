//! Opaque Git worktree lifecycle.

use crate::{fs as memfs, git, repo};
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{CommitMessage, RevisionId, WorktreeId};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

static WORKTREE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub(crate) struct WorktreeRecord {
    pub(crate) repository: PathBuf,
    pub(crate) directory: PathBuf,
    pub(crate) branch: String,
}

pub(crate) type Worktrees = Arc<Mutex<BTreeMap<WorktreeId, WorktreeRecord>>>;

fn adapter(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "memfs_worktree",
        context: context.into(),
    }
}

fn not_found() -> RuntimeError {
    RuntimeError::NotFound {
        context: "memfs worktree".into(),
    }
}

fn new_id() -> Result<WorktreeId, RuntimeError> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| adapter("worktree identifier"))?
        .as_nanos();
    let sequence = WORKTREE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    WorktreeId::new(format!(
        "wt-{stamp:032x}-{:08x}-{sequence:016x}",
        std::process::id()
    ))
}

fn worktree_root(repo: &Path) -> Result<PathBuf, RuntimeError> {
    let parent = repo.parent().ok_or_else(|| adapter("worktree parent"))?;
    let root = parent.join("memory-worktrees");
    memfs::ensure_confined_directory(parent, &root)?;
    Ok(root)
}

pub(crate) fn create(repo: &Path, records: &Worktrees) -> Result<WorktreeId, RuntimeError> {
    if fs::symlink_metadata(repo).is_err() {
        return Err(RuntimeError::NotFound {
            context: "memfs repository".into(),
        });
    }
    repo::head(repo)?;
    let root = worktree_root(repo)?;
    let id = new_id()?;
    let directory = root.join(id.as_str());
    if fs::symlink_metadata(&directory).is_ok() {
        return Err(RuntimeError::Conflict {
            context: "worktree directory".into(),
        });
    }
    let branch = format!("lotta-worktree-{}", id.as_str());
    let mut mapping = records.lock().map_err(|_| adapter("worktree mapping"))?;
    if mapping.contains_key(&id) {
        return Err(RuntimeError::Conflict {
            context: "worktree identifier".into(),
        });
    }
    add_worktree(repo, &directory, &branch)?;
    mapping.insert(
        id.clone(),
        WorktreeRecord {
            repository: repo.to_path_buf(),
            directory,
            branch,
        },
    );
    Ok(id)
}

fn add_worktree(repo: &Path, directory: &Path, branch: &str) -> Result<(), RuntimeError> {
    let path = directory.to_str().ok_or_else(|| adapter("worktree path"))?;
    git::checked(
        repo,
        ["worktree", "add", "-b", branch, path, "main"],
        "git worktree add",
    )?;
    Ok(())
}

pub(crate) fn merge(
    repo_path: &Path,
    id: &WorktreeId,
    message: &CommitMessage,
    records: &Worktrees,
) -> Result<RevisionId, RuntimeError> {
    let record = records
        .lock()
        .map_err(|_| adapter("worktree mapping"))?
        .get(id)
        .cloned()
        .ok_or_else(not_found)?;
    if record.repository != repo_path {
        return Err(not_found());
    }
    ensure_main_clean(repo_path)?;
    commit_reflection(&record, message)?;
    merge_main(&record, message)?;
    let revision = repo::head(&record.repository)?;
    consume(records, id, &record)?;
    repo::post_commit_push(&record.repository);
    cleanup(&record);
    Ok(revision)
}

fn ensure_main_clean(repo: &Path) -> Result<(), RuntimeError> {
    let status = git::checked(repo, ["status", "--porcelain=v1", "-z"], "git merge status")?;
    if !status.is_empty() {
        return Err(RuntimeError::Conflict {
            context: "dirty main worktree".into(),
        });
    }
    Ok(())
}

fn commit_reflection(record: &WorktreeRecord, message: &CommitMessage) -> Result<(), RuntimeError> {
    git::checked(
        &record.directory,
        ["add", "--all", "--"],
        "git worktree stage",
    )?;
    let branch = format!("refs/heads/{}", record.branch);
    repo::commit_staged(&record.directory, message.as_str(), &branch, true)?;
    Ok(())
}

fn merge_main(record: &WorktreeRecord, message: &CommitMessage) -> Result<(), RuntimeError> {
    let mut args: Vec<&str> = git::identity_args().into_iter().collect();
    args.extend([
        "merge",
        "--no-ff",
        record.branch.as_str(),
        "-m",
        message.as_str(),
    ]);
    if let Err(error) = git::checked(&record.repository, args, "git worktree merge") {
        drop(git::run(
            &record.repository,
            ["merge", "--abort"],
            "git merge abort",
        ));
        return Err(error);
    }
    Ok(())
}

fn consume(
    records: &Worktrees,
    id: &WorktreeId,
    record: &WorktreeRecord,
) -> Result<(), RuntimeError> {
    let mut mapping = records.lock().map_err(|_| adapter("worktree mapping"))?;
    if mapping
        .get(id)
        .is_none_or(|current| current.repository != record.repository)
    {
        return Err(not_found());
    }
    mapping.remove(id);
    Ok(())
}

fn cleanup(record: &WorktreeRecord) {
    if let Some(path) = record.directory.to_str() {
        drop(git::run(
            &record.repository,
            ["worktree", "remove", "--force", path],
            "git worktree remove",
        ));
    }
    drop(git::run(
        &record.repository,
        ["branch", "-D", record.branch.as_str()],
        "git worktree branch",
    ));
}

#[cfg(test)]
pub(crate) fn path_for_test(records: &Worktrees, id: &WorktreeId) -> Result<PathBuf, RuntimeError> {
    records
        .lock()
        .map_err(|_| adapter("worktree mapping"))?
        .get(id)
        .map(|record| record.directory.clone())
        .ok_or_else(not_found)
}
