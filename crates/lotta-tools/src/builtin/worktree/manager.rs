use super::{WorktreeManager, WorktreeOwner, provisioning};
use lotta_domain::{AgentId, ConversationId, RuntimeScope};
use lotta_runtime::boundary::{
    ConfinedPath, EnvironmentEntry, EnvironmentName, EnvironmentValue, ProcessArgument,
    ProcessArguments, ProcessEnvironment, ProcessOutputBytesMax, Program,
};
use lotta_runtime::ports::{ProcessEvent, ProcessRequest};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const GIT_OUTPUT_BYTES_MAX: usize = 1024 * 1024;
const GIT_TIMEOUT_MS: u64 = 120_000;
const LOCK_BYTES_MAX: u64 = 4_096;
const WORKTREES_MAX: usize = 1_024;

impl WorktreeManager {
    pub(super) async fn enter(
        &self,
        input: &Value,
        cancellation: CancellationToken,
    ) -> Result<String, ()> {
        let switch = input
            .get("switch_cwd")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let force = input.get("force").and_then(Value::as_bool).unwrap_or(false);
        let (path, branch, base, report, created) =
            if let Some(value) = input.get("path").and_then(Value::as_str) {
                if input.get("name").is_some()
                    || input.get("branch_name").is_some()
                    || input.get("base_ref").is_some()
                {
                    return Err(());
                }
                let repo = self.repo_root(input, &cancellation).await?;
                let path = self.registered_path(&repo, value, &cancellation).await?;
                (
                    path,
                    self.branch_for(&repo, value, &cancellation).await?,
                    String::new(),
                    provisioning::Report::default(),
                    false,
                )
            } else {
                self.create(input, &cancellation).await?
            };
        if self.acquire(&path, force).is_err() {
            if created {
                self.rollback(&path, &branch, &cancellation).await;
            }
            return Err(());
        }
        if switch && self.switch_to(&path).is_err() {
            let _ = self.release(&path);
            if created {
                self.rollback(&path, &branch, &cancellation).await;
            }
            return Err(());
        }
        let cwd = self.context.current_cwd().map_err(|_| ())?;
        bounded_result(&json!({
            "path": path,
            "cwd": cwd,
            "branch": branch,
            "base": base,
            "created": created,
            "switched": switch,
            "provision": report,
        }))
    }

    pub(super) async fn exit(
        &self,
        input: &Value,
        cancellation: &CancellationToken,
    ) -> Result<String, ()> {
        let current = self.context.current_cwd().map_err(|_| ())?;
        let action = input.get("action").and_then(Value::as_str).ok_or(())?;
        if !matches!(action, "keep" | "remove") {
            return Err(());
        }
        if current == self.primary_root {
            return bounded_result(&json!({"exited":false,"cwd":current,"removed":false}));
        }
        let removed = if action == "remove" {
            let discard = input
                .get("discard_changes")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            self.remove(&current, discard, cancellation).await?
        } else {
            false
        };
        let old = self.context.replace(self.primary_root.clone())?;
        if self.release(&old).is_err() {
            let _ = self.context.replace(old);
            return Err(());
        }
        bounded_result(&json!({
            "exited": true,
            "path": old,
            "cwd": self.primary_root,
            "removed": removed,
        }))
    }

    async fn create(
        &self,
        input: &Value,
        cancellation: &CancellationToken,
    ) -> Result<(PathBuf, String, String, provisioning::Report, bool), ()> {
        let name = input.get("name").and_then(Value::as_str).ok_or(())?;
        valid_name(name)?;
        let repo = self.repo_root(input, cancellation).await?;
        let managed = repo.join(".letta/worktrees");
        fs::create_dir_all(&managed).map_err(|_| ())?;
        let suffix = self
            .owner
            .start_nonce
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect::<String>();
        let branch = input
            .get("branch_name")
            .and_then(Value::as_str)
            .map_or_else(|| format!("letta/{name}-{suffix}"), str::to_owned);
        valid_git_token(&branch)?;
        let base = self.resolve_base(&repo, input, cancellation).await?;
        let path = collision_path(&managed, name, &suffix)?;
        let args = [
            "worktree",
            "add",
            "--no-track",
            "-b",
            &branch,
            path.to_str().ok_or(())?,
            &base,
        ];
        self.git(&repo, &args, cancellation).await?;
        let hooks_path = self.hooks_path(&repo, cancellation).await;
        let report = provisioning::provision(&repo, &path, input, hooks_path.as_deref());
        Ok((
            path.canonicalize().map_err(|_| ())?,
            branch,
            base,
            report,
            true,
        ))
    }

    async fn repo_root(
        &self,
        input: &Value,
        cancellation: &CancellationToken,
    ) -> Result<PathBuf, ()> {
        let candidate = input
            .get("repo_path")
            .and_then(Value::as_str)
            .map_or_else(|| self.primary_root.clone(), PathBuf::from);
        let canonical = candidate.canonicalize().map_err(|_| ())?;
        let output = self
            .git_output(&canonical, &["rev-parse", "--show-toplevel"], cancellation)
            .await?;
        let root = PathBuf::from(String::from_utf8(output).map_err(|_| ())?.trim());
        let root = root.canonicalize().map_err(|_| ())?;
        if root != canonical
            || fs::symlink_metadata(&root)
                .map_err(|_| ())?
                .file_type()
                .is_symlink()
        {
            return Err(());
        }
        Ok(root)
    }

    async fn resolve_base(
        &self,
        repo: &Path,
        input: &Value,
        cancellation: &CancellationToken,
    ) -> Result<String, ()> {
        if let Some(base) = input.get("base_ref").and_then(Value::as_str) {
            valid_git_token(base)?;
            return Ok(base.into());
        }
        let remote = String::from_utf8(self.git_output(repo, &["remote"], cancellation).await?)
            .map_err(|_| ())?
            .lines()
            .next()
            .map(str::to_owned);
        if input
            .get("refresh_base")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            && let Some(remote) = &remote
        {
            self.git(repo, &["fetch", "--prune", remote], cancellation)
                .await?;
        }
        if let Some(remote) = remote
            && let Ok(bytes) = self
                .git_output(
                    repo,
                    &["symbolic-ref", &format!("refs/remotes/{remote}/HEAD")],
                    cancellation,
                )
                .await
        {
            let value = String::from_utf8(bytes).map_err(|_| ())?;
            let value = value
                .trim()
                .strip_prefix("refs/remotes/")
                .ok_or(())?
                .to_owned();
            valid_git_token(&value)?;
            return Ok(value);
        }
        for candidate in ["main", "master", "HEAD"] {
            if self
                .git(repo, &["rev-parse", "--verify", candidate], cancellation)
                .await
                .is_ok()
            {
                return Ok(candidate.into());
            }
        }
        Err(())
    }

    async fn registered_path(
        &self,
        repo: &Path,
        value: &str,
        cancellation: &CancellationToken,
    ) -> Result<PathBuf, ()> {
        let canonical = PathBuf::from(value).canonicalize().map_err(|_| ())?;
        let managed = repo
            .join(".letta/worktrees")
            .canonicalize()
            .map_err(|_| ())?;
        if canonical == repo
            || !canonical.starts_with(&managed)
            || fs::symlink_metadata(value)
                .map_err(|_| ())?
                .file_type()
                .is_symlink()
        {
            return Err(());
        }
        let records = parse_worktrees(
            &self
                .git_output(
                    repo,
                    &["worktree", "list", "--porcelain", "-z"],
                    cancellation,
                )
                .await?,
        )?;
        records
            .into_iter()
            .find(|record| record.path == canonical && !record.prunable && !record.bare)
            .map(|record| record.path)
            .ok_or(())
    }

    async fn branch_for(
        &self,
        repo: &Path,
        path: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, ()> {
        let canonical = PathBuf::from(path).canonicalize().map_err(|_| ())?;
        parse_worktrees(
            &self
                .git_output(
                    repo,
                    &["worktree", "list", "--porcelain", "-z"],
                    cancellation,
                )
                .await?,
        )?
        .into_iter()
        .find(|record| record.path == canonical)
        .and_then(|record| record.branch)
        .ok_or(())
    }

    fn switch_to(&self, path: &Path) -> Result<(), ()> {
        let old = self.context.replace(path.to_owned())?;
        if old != self.primary_root && old != path && self.release(&old).is_err() {
            let _ = self.context.replace(old);
            return Err(());
        }
        Ok(())
    }

    fn lock_path(path: &Path) -> PathBuf {
        path.join(".git").join("letta-enter.lock")
    }

    pub(super) fn acquire(&self, path: &Path, force: bool) -> Result<(), ()> {
        let lock = Self::lock_path(path);
        for _ in 0..8 {
            if write_new_lock(&lock, &self.owner).is_ok() {
                self.owned.lock().map_err(|_| ())?.insert(path.to_owned());
                return Ok(());
            }
            let existing = read_lock(&lock)?;
            if existing == self.owner {
                self.owned.lock().map_err(|_| ())?.insert(path.to_owned());
                return Ok(());
            }
            let stale = existing.hostname == self.owner.hostname
                && !self
                    .liveness
                    .is_alive(existing.process_id, &existing.start_nonce);
            if !force && !stale {
                return Err(());
            }
            let quarantine = lock.with_extension(format!("takeover.{}", self.owner.start_nonce));
            if fs::rename(&lock, &quarantine).is_err() {
                continue;
            }
            if read_lock(&quarantine)? != existing {
                let _ = fs::rename(&quarantine, &lock);
                continue;
            }
            let _ = fs::remove_file(quarantine);
        }
        Err(())
    }

    pub(super) fn release(&self, path: &Path) -> Result<(), ()> {
        let lock = Self::lock_path(path);
        if read_lock(&lock)? != self.owner {
            return Err(());
        }
        fs::remove_file(lock).map_err(|_| ())?;
        self.owned.lock().map_err(|_| ())?.remove(path);
        Ok(())
    }

    async fn hooks_path(&self, repo: &Path, cancellation: &CancellationToken) -> Option<PathBuf> {
        let output = self
            .git_output(repo, &["config", "--get", "core.hooksPath"], cancellation)
            .await
            .ok()?;
        let value = String::from_utf8(output).ok()?;
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            provisioning::safe_hooks_path(value)
        }
    }

    async fn remove(
        &self,
        path: &Path,
        discard: bool,
        cancellation: &CancellationToken,
    ) -> Result<bool, ()> {
        let value = path.to_str().ok_or(())?;
        let branch = self
            .branch_for(&self.primary_root, value, cancellation)
            .await?;
        if !discard {
            let status = self
                .git_output(path, &["status", "--porcelain"], cancellation)
                .await?;
            if !status.is_empty() {
                return Err(());
            }
            let unmerged = self
                .git_output(
                    &self.primary_root,
                    &["rev-list", "--count", &format!("HEAD..{branch}")],
                    cancellation,
                )
                .await?;
            if String::from_utf8(unmerged)
                .map_err(|_| ())?
                .trim()
                .parse::<u64>()
                .map_err(|_| ())?
                != 0
            {
                return Err(());
            }
        }
        let mut args = vec!["worktree", "remove"];
        if discard {
            args.push("--force");
        }
        args.push(value);
        self.git(&self.primary_root, &args, cancellation).await?;
        let delete = if discard { "-D" } else { "-d" };
        let _ = self
            .git(
                &self.primary_root,
                &["branch", delete, &branch],
                cancellation,
            )
            .await;
        Ok(true)
    }

    async fn rollback(&self, path: &Path, branch: &str, cancellation: &CancellationToken) {
        let _ = self.release(path);
        if let Some(value) = path.to_str() {
            let _ = self
                .git(
                    &self.primary_root,
                    &["worktree", "remove", "--force", value],
                    cancellation,
                )
                .await;
        }
        let _ = self
            .git(&self.primary_root, &["branch", "-D", branch], cancellation)
            .await;
    }

    async fn git(
        &self,
        root: &Path,
        args: &[&str],
        cancellation: &CancellationToken,
    ) -> Result<(), ()> {
        self.git_output(root, args, cancellation).await.map(drop)
    }

    async fn git_output(
        &self,
        root: &Path,
        args: &[&str],
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, ()> {
        let request = process_request(root, args)?;
        let (sender, mut receiver) = mpsc::channel(8);
        let future = self.process.run(request, sender, cancellation.clone());
        let drain = async {
            let mut output = Vec::new();
            while let Some(event) = receiver.recv().await {
                if let ProcessEvent::Stdout(bytes) = event {
                    let bytes = bytes.into_vec();
                    if output.len().saturating_add(bytes.len()) > GIT_OUTPUT_BYTES_MAX {
                        return Err(());
                    }
                    output.extend_from_slice(&bytes);
                }
            }
            Ok(output)
        };
        let (outcome, output) = tokio::join!(future, drain);
        let outcome = outcome.map_err(|_| ())?;
        if outcome.exit_code == Some(0) && !outcome.timed_out && !cancellation.is_cancelled() {
            output
        } else {
            Err(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct LockRecord {
    agent_id: String,
    conversation_id: String,
    process_id: u32,
    hostname: String,
    start_nonce: String,
}
impl From<&WorktreeOwner> for LockRecord {
    fn from(value: &WorktreeOwner) -> Self {
        Self {
            agent_id: value.agent_id.clone(),
            conversation_id: value.conversation_id.clone(),
            process_id: value.process_id,
            hostname: value.hostname.clone(),
            start_nonce: value.start_nonce.clone(),
        }
    }
}
impl PartialEq<WorktreeOwner> for LockRecord {
    fn eq(&self, value: &WorktreeOwner) -> bool {
        self == &Self::from(value)
    }
}

fn write_new_lock(path: &Path, owner: &WorktreeOwner) -> Result<(), ()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| ())?;
    let bytes = serde_json::to_vec(&LockRecord::from(owner)).map_err(|_| ())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| ())
}
fn read_lock(path: &Path) -> Result<LockRecord, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > LOCK_BYTES_MAX {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(());
        }
    }
    let bytes = fs::read(path).map_err(|_| ())?;
    serde_json::from_slice(&bytes).map_err(|_| ())
}

struct WorktreeRecord {
    path: PathBuf,
    branch: Option<String>,
    prunable: bool,
    bare: bool,
}
fn parse_worktrees(bytes: &[u8]) -> Result<Vec<WorktreeRecord>, ()> {
    if bytes.len() > GIT_OUTPUT_BYTES_MAX {
        return Err(());
    }
    let fields = bytes.split(|byte| *byte == 0);
    let mut records = Vec::new();
    let mut current = None;
    for field in fields {
        if field.is_empty() {
            continue;
        }
        let text = std::str::from_utf8(field).map_err(|_| ())?;
        if let Some(path) = text.strip_prefix("worktree ") {
            if let Some(record) = current.take() {
                records.push(record);
            }
            current = Some(WorktreeRecord {
                path: PathBuf::from(path).canonicalize().map_err(|_| ())?,
                branch: None,
                prunable: false,
                bare: false,
            });
        } else if let Some(record) = current.as_mut() {
            if let Some(branch) = text.strip_prefix("branch refs/heads/") {
                record.branch = Some(branch.into());
            } else if text == "prunable" || text.starts_with("prunable ") {
                record.prunable = true;
            } else if text == "bare" {
                record.bare = true;
            }
        }
        if records.len() > WORKTREES_MAX {
            return Err(());
        }
    }
    if let Some(record) = current {
        records.push(record);
    }
    Ok(records)
}
fn collision_path(root: &Path, name: &str, suffix: &str) -> Result<PathBuf, ()> {
    for index in 0..1000 {
        let leaf = if index == 0 {
            format!("{name}-{suffix}")
        } else {
            format!("{name}-{suffix}-{index}")
        };
        let path = root.join(leaf);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(path),
            Ok(_) => {}
            Err(_) => return Err(()),
        }
    }
    Err(())
}
fn bounded_result(value: &Value) -> Result<String, ()> {
    let text = serde_json::to_string(value).map_err(|_| ())?;
    if text.len() > 64 * 1024 {
        Err(())
    } else {
        Ok(text)
    }
}
fn valid_name(value: &str) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > 128
        || value.contains(['/', '\\', '\0'])
        || matches!(value, "." | "..")
    {
        Err(())
    } else {
        Ok(())
    }
}
fn valid_git_token(value: &str) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > 256
        || value.starts_with('-')
        || value.chars().any(char::is_control)
    {
        Err(())
    } else {
        Ok(())
    }
}
fn process_request(root: &Path, args: &[&str]) -> Result<ProcessRequest, ()> {
    let arguments = ProcessArguments::new(
        args.iter()
            .map(|value| ProcessArgument::new((*value).into()).map_err(|_| ()))
            .collect::<Result<Vec<_>, _>>()?,
    )
    .map_err(|_| ())?;
    let environment = ProcessEnvironment::new(vec![
        EnvironmentEntry::new(
            EnvironmentName::new("GIT_TERMINAL_PROMPT".into()).map_err(|_| ())?,
            EnvironmentValue::new("0".into()).map_err(|_| ())?,
        )
        .map_err(|_| ())?,
    ])
    .map_err(|_| ())?;
    ProcessRequest::new(
        RuntimeScope::new(
            AgentId::accept("worktree").map_err(|_| ())?,
            ConversationId::accept("worktree").map_err(|_| ())?,
            None,
        ),
        Program::new("git".into()).map_err(|_| ())?,
        arguments,
        ConfinedPath::new(root.into(), root.into()).map_err(|_| ())?,
        environment,
        None,
        ProcessOutputBytesMax::new(GIT_OUTPUT_BYTES_MAX).map_err(|_| ())?,
        Duration::from_millis(GIT_TIMEOUT_MS),
    )
    .map_err(|_| ())
}
