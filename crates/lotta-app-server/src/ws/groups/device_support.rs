//! Confined git-branch operations and the local-backend secrets surface used
//! by the device command group.
//!
//! Branch operations run the pinned baseline's exact `git` invocations inside
//! the workspace root only: a caller-supplied cwd is canonicalized and must
//! remain under that root, output capture is byte-bounded, and every command
//! carries the pinned timeout budget. Secrets persist through the Task 52
//! local-backend store family — one JSON side file beside `providers/auth.json`,
//! written with the same restrictive provider-auth file mode and optimistic-
//! revision discipline. The location is a deliberate side-store: spec 04 keeps
//! "JSON, JSONL, `auth.json`, side stores, and Git" as the canonical store
//! family and does not name a secrets file, so this one sits in the same
//! mode-0700/0600 credential directory instead of inventing an unlisted
//! `.letta` layout entry. List responses carry `{key, value}` entries whose
//! plaintext values are intentionally exposed to the authenticated secrets
//! modal per the pinned protocol contract.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lotta_store::atomic::{WriteMode, atomic_write};
use lotta_store::side::{read_opaque, write_opaque_expected};
use serde::Serialize;
use tokio::process::Command;

/// Default branch-search result cap from the pinned `search_branches` command.
pub(crate) const BRANCH_RESULTS_DEFAULT_MAX: usize = 20;
/// Hard upper bound applied to any requested search result count.
pub(crate) const BRANCH_RESULTS_MAX: usize = 200;
/// Bounded wait for one branch search, matching the pinned five-second budget.
pub(crate) const BRANCH_SEARCH_TIMEOUT_MS: u64 = 5_000;
/// Bounded wait for one branch checkout, matching the pinned ten-second budget.
pub(crate) const BRANCH_CHECKOUT_TIMEOUT_MS: u64 = 10_000;
/// Maximum accepted branch-query bytes.
pub(crate) const BRANCH_QUERY_BYTES_MAX: usize = 1_024;
/// Maximum captured stdout/stderr bytes per bounded git invocation.
pub(crate) const GIT_OUTPUT_BYTES_MAX: usize = 1_048_576;
/// Maximum retained secrets per agent record.
pub(crate) const SECRETS_PER_AGENT_MAX: usize = 128;
/// Maximum agents holding at least one secret.
pub(crate) const SECRET_AGENTS_MAX: usize = 1_024;

/// One git branch in the pinned `GitBranchInfo` shape.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GitBranchInfo {
    /// Short refname of the branch.
    pub name: String,
    /// Whether this branch is currently checked out.
    pub is_current: bool,
    /// Whether the branch lives on a remote.
    pub is_remote: bool,
}

/// Runs one bounded `git` command under an absolute workspace-confined cwd.
///
/// # Errors
/// Returns a scrubbed failure text when the cwd escapes the workspace root,
/// the command exceeds its deadline, git exits non-zero, or either output
/// stream overflows the bounded capture buffer.
pub(crate) async fn run_git(
    workspace_root: &Path,
    cwd: Option<&String>,
    args: &[&str],
    timeout_ms: u64,
) -> Result<String, String> {
    let dir = confined_dir(workspace_root, cwd)?;
    let mut child = Command::new("git")
        .current_dir(&dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "failed to run git".to_owned())?;
    let stdout = child.stdout.take().ok_or("git output unavailable")?;
    let stderr = child.stderr.take().ok_or("git output unavailable")?;
    // Each stream drains to EOF under its own explicit byte bound; both
    // readers finish at or before process exit closes the pipes.
    let stdout_reader = tokio::spawn(bounded_stream(stdout));
    let stderr_reader = tokio::spawn(bounded_stream(stderr));
    let wait = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
        let status = child.wait().await.map_err(|_| "failed to run git")?;
        let _stderr = stderr_reader
            .await
            .map_err(|_| "failed to run git".to_owned())??;
        let stdout = stdout_reader
            .await
            .map_err(|_| "failed to run git".to_owned())??;
        Ok::<_, String>((status, stdout))
    });
    // kill_on_drop reaps the child when the timed-out future drops.
    let (status, stdout_bytes) = wait
        .await
        .map_err(|_| "git command timed out".to_owned())??;
    if !status.success() {
        return Err("git command failed".to_owned());
    }
    String::from_utf8(stdout_bytes).map_err(|_| "git produced invalid utf-8".to_owned())
}

/// Reads one output stream to EOF, refusing anything past the capture bound.
async fn bounded_stream(
    mut pipe: impl tokio::io::AsyncRead + Unpin + Send,
) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0_u8; 8192];
        let read = pipe
            .read(&mut chunk)
            .await
            .map_err(|_| "failed to run git".to_owned())?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > GIT_OUTPUT_BYTES_MAX {
            return Err("git output exceeded the capture bound".to_owned());
        }
    }
}

/// Resolves the effective working directory, refusing anything outside the root.
///
/// Canonicalization is fail-closed: a cwd that cannot be resolved on disk
/// (missing path or broken symlink) is rejected rather than accepted as-is,
/// so no unverified path ever reaches git.
fn confined_dir(root: &Path, cwd: Option<&String>) -> Result<PathBuf, String> {
    let Some(raw) = cwd else {
        return Ok(root.to_path_buf());
    };
    let candidate = PathBuf::from(raw);
    if !candidate.is_absolute() {
        return Err("cwd must be absolute".to_owned());
    }
    let resolved = candidate
        .canonicalize()
        .map_err(|_| "cwd unavailable".to_owned())?;
    if !resolved.starts_with(root) || resolved == root.parent().unwrap_or(root) {
        return Err("cwd escapes the workspace".to_owned());
    }
    Ok(resolved)
}

/// Validates one branch reference name before handing it to git.
///
/// Rejects option-looking leading dashes and any character outside the
/// git-check-ref-format alphabet (`A-Za-z0-9._/-`), plus the known dangerous
/// sequences: empty components (`..`, `//`, leading/trailing `/`),
/// `.lock` suffixes, leading `.`, trailing `.`, and `@{`.
pub(crate) fn valid_branch_name(branch: &str) -> bool {
    if branch.starts_with('-') || branch.is_empty() || branch.len() > BRANCH_QUERY_BYTES_MAX {
        return false;
    }
    if !branch.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '/' | '-')
    }) {
        return false;
    }
    // Every byte is ASCII from here, so suffix slicing stays boundary-safe.
    !branch.starts_with('.')
        && !branch.ends_with('.')
        && !branch.ends_with('/')
        && !has_lock_suffix(branch)
        && !branch.contains("..")
        && !branch.contains("//")
        && !branch.contains("@{")
}

/// Case-insensitive `.lock` suffix probe over already-validated ASCII input.
fn has_lock_suffix(branch: &str) -> bool {
    const LOCK: &str = ".lock";
    branch.len() >= LOCK.len() && branch[branch.len() - LOCK.len()..].eq_ignore_ascii_case(LOCK)
}

/// Parses bounded `git branch -a --format=%(refname:short)\t%(HEAD)` output.
pub(crate) fn parse_branches(stdout: &str, query: &str, max_results: usize) -> Vec<GitBranchInfo> {
    let needle = query.to_lowercase();
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(parse_branch_line)
        .filter(|branch| query.is_empty() || branch.name.to_lowercase().contains(&needle))
        .take(max_results)
        .collect()
}

fn parse_branch_line(line: &str) -> Option<GitBranchInfo> {
    let mut columns = line.split('\t');
    let name = columns.next()?.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    Some(GitBranchInfo {
        is_remote: name.starts_with("origin/"),
        is_current: columns.next().is_some_and(|mark| mark.trim() == "*"),
        name,
    })
}

/// Per-agent named secrets persisted in the Task 52 local-backend credential
/// directory.
///
/// The on-disk form is `{ "<agent_id>": { "<NAME>": "<plaintext>" } }`; the
/// wire `secret_list` surface exposes the pinned `{key, value}` entries to
/// authenticated secrets modals, while apply responses carry names only.
#[derive(Clone)]
pub struct AgentSecretsStore {
    path: PathBuf,
}

/// One stored agent secret map with its optimistic revision token.
struct StoredSecrets {
    agents: BTreeMap<String, BTreeMap<String, String>>,
    revision: Option<lotta_store::side::OpaqueFile>,
}

impl AgentSecretsStore {
    /// Creates the store over `<storage>/providers/secrets.json`.
    ///
    /// The file is a documented side-store beside `providers/auth.json`:
    /// spec 04 names no dedicated secrets path, so the store reuses the exact
    /// credential-directory discipline (mode 0700 dir, 0600 file,
    /// provider-auth write mode) already sanctioned for secret material.
    #[must_use]
    pub fn new(storage_dir: &Path) -> Self {
        Self {
            path: storage_dir.join("providers").join("secrets.json"),
        }
    }

    /// Applies `(set ∖ unset ∪ kept)` atomically and returns sorted names.
    ///
    /// Keys present in both `set` and `unset` resolve to removal, matching the
    /// pinned defensive rule.
    ///
    /// # Errors
    /// Returns a scrubbed failure text when the store cannot be read, the
    /// record bounds are exceeded, or the optimistic revision conflicts.
    pub(crate) fn apply(
        &self,
        agent_id: &str,
        set: BTreeMap<String, String>,
        unset: &[String],
    ) -> Result<Vec<String>, String> {
        let mut stored = self.load()?;
        let mut entry = stored.agents.remove(agent_id).unwrap_or_default();
        let mut set = set;
        for key in unset {
            entry.remove(key);
            set.remove(key);
        }
        for (key, value) in set {
            if entry.len() >= SECRETS_PER_AGENT_MAX && !entry.contains_key(&key) {
                return Err("too many secrets for this agent".to_owned());
            }
            entry.insert(key, value);
        }
        if !entry.is_empty() {
            if stored.agents.len() >= SECRET_AGENTS_MAX {
                return Err("too many agents hold secrets".to_owned());
            }
            stored.agents.insert(agent_id.to_owned(), entry);
        }
        let names = stored
            .agents
            .get(agent_id)
            .into_iter()
            .flat_map(BTreeMap::keys)
            .cloned()
            .collect::<Vec<_>>();
        self.commit(&stored)?;
        Ok(names)
    }

    /// Returns sorted `{key, value}` entries held for one agent.
    ///
    /// # Errors
    /// Returns a scrubbed failure text when the store cannot be read.
    pub(crate) fn entries(&self, agent_id: &str) -> Result<Vec<(String, String)>, String> {
        let stored = self.load()?;
        Ok(stored
            .agents
            .get(agent_id)
            .into_iter()
            .flat_map(BTreeMap::iter)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    }

    fn load(&self) -> Result<StoredSecrets, String> {
        let opaque = match read_opaque(&self.path) {
            Ok(opaque) => opaque,
            Err(error) if error.kind() == lotta_store::StoreErrorKind::NotFound => {
                return Ok(StoredSecrets {
                    agents: BTreeMap::new(),
                    revision: None,
                });
            }
            Err(_) => return Err("secrets store unreadable".to_owned()),
        };
        let bytes = opaque.bytes();
        if bytes.len() > lotta_store::ATOMIC_WRITE_BYTES_MAX {
            return Err("secrets store oversized".to_owned());
        }
        let agents: BTreeMap<String, BTreeMap<String, String>> =
            serde_json::from_slice(bytes).map_err(|_| "secrets store malformed".to_owned())?;
        Ok(StoredSecrets {
            agents,
            revision: Some(opaque),
        })
    }

    fn commit(&self, stored: &StoredSecrets) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(&stored.agents)
            .map_err(|_| "secrets store encoding failed".to_owned())?;
        bytes.push(b'\n');
        let outcome = match &stored.revision {
            Some(revision) => write_opaque_expected(
                &self.path,
                &bytes,
                &revision.revision().clone(),
                WriteMode::ProviderAuth,
            ),
            None => atomic_write(&self.path, &bytes, WriteMode::ProviderAuth),
        };
        outcome.map_err(|_| "secrets store write conflicted".to_owned())
    }
}

/// Agent-scoped [`lotta_tools::pipeline::SecretResolver`] over one
/// [`AgentSecretsStore`]: resolves applied secret plaintext for tool
/// pipelines executing on that agent's behalf.
pub struct AgentSecretResolver {
    store: AgentSecretsStore,
    agent_id: String,
}

impl AgentSecretResolver {
    /// Creates a resolver reading the secret record of exactly one agent.
    #[must_use]
    pub fn new(store: AgentSecretsStore, agent_id: impl Into<String>) -> Self {
        Self {
            store,
            agent_id: agent_id.into(),
        }
    }

    /// Creates a resolver over `<storage>/providers/secrets.json` for one agent.
    #[must_use]
    pub fn over_storage(storage_dir: &Path, agent_id: impl Into<String>) -> Self {
        Self::new(AgentSecretsStore::new(storage_dir), agent_id)
    }
}

impl lotta_tools::pipeline::SecretResolver for AgentSecretResolver {
    fn resolve(&self, name: &str) -> Result<Option<String>, lotta_tools::PipelineError> {
        match self.store.entries(&self.agent_id) {
            Ok(entries) => Ok(entries
                .into_iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value)),
            Err(_) => Err(lotta_tools::PipelineError::SecretDelivery),
        }
    }
}

/// Rejects secret names outside the pinned `[A-Z_][A-Z0-9_]*` alphabet.
pub(crate) fn valid_secret_name(key: &str) -> bool {
    let upper = key.to_uppercase();
    let mut chars = upper.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_uppercase() || first == '_')
        && chars.all(|rest| rest.is_ascii_uppercase() || rest.is_ascii_digit() || rest == '_')
}

/// Normalizes one secret key the way the pinned listener does before storage.
pub(crate) fn normalize_secret_name(key: &str) -> String {
    key.to_uppercase()
}

#[cfg(test)]
mod round_trip {
    use super::*;

    #[test]
    fn branch_lines_round_trip_through_the_parser() {
        let stdout = "main\t*\norigin/feature\t \norigin/main\t \n";
        let branches = parse_branches(stdout, "", BRANCH_RESULTS_DEFAULT_MAX);
        assert_eq!(
            branches,
            vec![
                GitBranchInfo {
                    name: "main".to_owned(),
                    is_current: true,
                    is_remote: false,
                },
                GitBranchInfo {
                    name: "origin/feature".to_owned(),
                    is_current: false,
                    is_remote: true,
                },
                GitBranchInfo {
                    name: "origin/main".to_owned(),
                    is_current: false,
                    is_remote: true,
                },
            ]
        );
    }

    #[test]
    fn secret_names_validate_like_the_pinned_listener() {
        assert!(valid_secret_name("API_KEY"));
        assert!(valid_secret_name("_PRIVATE"));
        assert!(!valid_secret_name("1ST_KEY"));
        assert!(!valid_secret_name("has space"));
    }
}
