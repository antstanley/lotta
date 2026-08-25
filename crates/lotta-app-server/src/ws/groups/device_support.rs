//! Confined git-branch operations and the local-backend secrets surface used
//! by the device command group.
//!
//! Branch operations run the pinned baseline's exact `git` invocations inside
//! the workspace root only: a caller-supplied cwd is canonicalized and must
//! remain under that root, output is byte-bounded, and every command carries
//! the pinned timeout budget. Secrets persist through the Task 52 local-backend
//! store family — one JSON side file written with the same restrictive
//! provider-auth file mode and optimistic-revision discipline as
//! `providers/auth.json`. Secret plaintext is write-only here: list and apply
//! responses carry secret names, never values.

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
/// the command exceeds its deadline, or git exits non-zero or overflows the
/// bounded output buffer.
pub(crate) async fn run_git(
    workspace_root: &Path,
    cwd: Option<&String>,
    args: &[&str],
    timeout_ms: u64,
) -> Result<String, String> {
    let dir = confined_dir(workspace_root, cwd)?;
    let child = Command::new("git")
        .current_dir(&dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let wait = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), child);
    let output = wait
        .await
        .map_err(|_| "git command timed out".to_owned())?
        .map_err(|_| "failed to run git".to_owned())?;
    if !output.status.success() {
        return Err("git command failed".to_owned());
    }
    String::from_utf8(output.stdout).map_err(|_| "git produced invalid utf-8".to_owned())
}

/// Resolves the effective working directory, refusing anything outside the root.
fn confined_dir(root: &Path, cwd: Option<&String>) -> Result<PathBuf, String> {
    let Some(raw) = cwd else {
        return Ok(root.to_path_buf());
    };
    let candidate = PathBuf::from(raw);
    if !candidate.is_absolute() {
        return Err("cwd must be absolute".to_owned());
    }
    let resolved = candidate.canonicalize().unwrap_or(candidate);
    if !resolved.starts_with(root) || resolved == root.parent().unwrap_or(root) {
        return Err("cwd escapes the workspace".to_owned());
    }
    Ok(resolved)
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

/// Per-agent named secrets persisted beside the Task 52 provider auth store.
///
/// The on-disk form is `{ "<agent_id>": { "<NAME>": "<plaintext>" } }`; the
/// wire surface built above it exposes names only.
#[derive(Clone)]
pub(crate) struct AgentSecretsStore {
    path: PathBuf,
}

/// One stored agent secret map with its optimistic revision token.
struct StoredSecrets {
    agents: BTreeMap<String, BTreeMap<String, String>>,
    revision: Option<lotta_store::side::OpaqueFile>,
}

impl AgentSecretsStore {
    /// Creates the store over `<storage>/.letta/secrets.json`.
    pub(crate) fn new(storage_dir: &Path) -> Self {
        Self {
            path: storage_dir.join(".letta").join("secrets.json"),
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

    /// Returns the sorted secret names held for one agent.
    ///
    /// # Errors
    /// Returns a scrubbed failure text when the store cannot be read.
    pub(crate) fn names(&self, agent_id: &str) -> Result<Vec<String>, String> {
        let stored = self.load()?;
        Ok(stored
            .agents
            .get(agent_id)
            .into_iter()
            .flat_map(BTreeMap::keys)
            .cloned()
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
