use super::CompiledPromptRecord;
use super::input::{PromptInputs, PromptText, validate_inputs};
use super::render::{MemoryFile, render_record};
use super::util::{cancelled, check_cancelled, checked_add, limit, reserve_one};
use lotta_domain::AgentId;
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{RepositoryPath, RevisionId};
use lotta_runtime::ports::MemFsPort;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const TREE_CHANNEL_ITEMS_MAX: usize = 1;
const PROMPT_PATH_ITEMS_MAX: usize = 100_000;

/// Stateless compiler reading only a status-selected committed revision.
pub struct PromptCompiler<'a> {
    memfs: &'a dyn MemFsPort,
    render_counter: Option<&'a std::sync::atomic::AtomicUsize>,
}

impl<'a> PromptCompiler<'a> {
    /// Borrows the memory port.
    #[must_use]
    pub const fn new(memfs: &'a dyn MemFsPort) -> Self {
        Self {
            memfs,
            render_counter: None,
        }
    }

    #[cfg(test)]
    pub(crate) const fn with_render_counter(
        memfs: &'a dyn MemFsPort,
        counter: &'a std::sync::atomic::AtomicUsize,
    ) -> Self {
        Self {
            memfs,
            render_counter: Some(counter),
        }
    }

    /// Returns the current committed revision, if present.
    ///
    /// # Errors
    /// Propagates status errors other than a missing repository.
    pub async fn committed_revision(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<RevisionId>, RuntimeError> {
        match self.memfs.status(agent_id).await {
            Ok(value) => Ok(value.revision),
            Err(RuntimeError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Returns the raw prompt hash and current revision without rendering.
    ///
    /// # Errors
    /// Propagates status errors from the memory port.
    pub async fn candidate(
        &self,
        agent_id: &AgentId,
        raw_system: &PromptText,
    ) -> Result<(String, Option<RevisionId>), RuntimeError> {
        Ok((
            hash_raw_system(raw_system.as_str()),
            self.committed_revision(agent_id).await?,
        ))
    }

    /// Compiles bounded prompt content from committed memory.
    ///
    /// # Errors
    /// Returns typed cancellation, port, validation, or limit errors.
    pub async fn compile(
        &self,
        input: &PromptInputs,
        cancellation: CancellationToken,
    ) -> Result<CompiledPromptRecord, RuntimeError> {
        check_cancelled(&cancellation)?;
        let revision = self.committed_revision(input.agent_id()).await?;
        check_cancelled(&cancellation)?;
        self.compile_at_revision(input, revision.as_ref(), cancellation)
            .await
    }

    pub(crate) async fn compile_at_revision(
        &self,
        input: &PromptInputs,
        revision: Option<&RevisionId>,
        cancellation: CancellationToken,
    ) -> Result<CompiledPromptRecord, RuntimeError> {
        check_cancelled(&cancellation)?;
        validate_inputs(input)?;
        if let Some(counter) = self.render_counter {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let files = self
            .load_files(input.agent_id(), revision, cancellation.clone())
            .await?;
        check_cancelled(&cancellation)?;
        render_record(input, revision.cloned(), &files)
    }

    async fn load_files(
        &self,
        agent: &AgentId,
        revision: Option<&RevisionId>,
        cancellation: CancellationToken,
    ) -> Result<Vec<MemoryFile>, RuntimeError> {
        let Some(revision) = revision else {
            return Ok(Vec::new());
        };
        let paths = collect_paths(self.memfs, agent, revision, cancellation.clone()).await?;
        let mut files = Vec::new();
        let mut retained = 0usize;
        for path in paths {
            check_cancelled(&cancellation)?;
            let value = match self.memfs.file_at_revision(agent, &path, revision).await {
                Ok(value) => value,
                Err(RuntimeError::NotFound { .. }) => continue,
                Err(error) => return Err(error),
            };
            check_cancelled(&cancellation)?;
            let file = match MemoryFile::parse(&path, &value) {
                Ok(file) => file,
                Err(RuntimeError::InvalidData { .. }) => continue,
                Err(error) => return Err(error),
            };
            retained = retain_with_limit(
                retained,
                file.retained_bytes()?,
                super::input::PROMPT_COMPILED_BYTES_MAX,
            )?;
            reserve_one(&mut files, "prompt memory files")?;
            files.push(file);
        }
        files.sort_by(|left, right| left.label().cmp(right.label()));
        Ok(files)
    }
}

async fn collect_paths(
    memfs: &dyn MemFsPort,
    agent: &AgentId,
    revision: &RevisionId,
    cancellation: CancellationToken,
) -> Result<Vec<RepositoryPath>, RuntimeError> {
    collect_paths_with_limits(
        memfs,
        agent,
        revision,
        cancellation,
        PROMPT_PATH_ITEMS_MAX,
        super::input::PROMPT_COMPILED_BYTES_MAX,
    )
    .await
}

pub(crate) async fn collect_paths_with_limits(
    memfs: &dyn MemFsPort,
    agent: &AgentId,
    revision: &RevisionId,
    cancellation: CancellationToken,
    item_limit: usize,
    byte_limit: usize,
) -> Result<Vec<RepositoryPath>, RuntimeError> {
    let (sender, mut receiver) = mpsc::channel(TREE_CHANNEL_ITEMS_MAX);
    let listing = memfs.tree(agent, Some(revision), sender, cancellation.clone());
    tokio::pin!(listing);
    let mut paths = Vec::new();
    let mut path_bytes = 0usize;
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled()),
            result = &mut listing => {
                result?;
                while let Some(entry) = receiver.recv().await {
                    push_path(
                        &mut paths,
                        entry.path,
                        &mut path_bytes,
                        item_limit,
                        byte_limit,
                    )?;
                }
                paths.sort();
                return Ok(paths);
            }
            entry = receiver.recv() => if let Some(entry) = entry {
                push_path(
                    &mut paths,
                    entry.path,
                    &mut path_bytes,
                    item_limit,
                    byte_limit,
                )?;
            }
        }
    }
}

fn push_path(
    paths: &mut Vec<RepositoryPath>,
    path: RepositoryPath,
    path_bytes: &mut usize,
    item_limit: usize,
    byte_limit: usize,
) -> Result<(), RuntimeError> {
    if path
        .as_path()
        .extension()
        .is_some_and(|value| value == "md")
    {
        if paths.len() >= item_limit {
            return Err(limit("prompt memory path items"));
        }
        let bytes = checked_add(
            path.as_path().as_os_str().len(),
            1,
            "prompt memory path bytes",
        )?;
        *path_bytes =
            budget_with_limit(*path_bytes, bytes, byte_limit, "prompt memory path bytes")?;
        reserve_one(paths, "prompt memory paths")?;
        paths.push(path);
    }
    Ok(())
}

pub(crate) fn retain_with_limit(
    retained: usize,
    added: usize,
    maximum: usize,
) -> Result<usize, RuntimeError> {
    budget_with_limit(retained, added, maximum, "prompt memory bytes")
}

fn budget_with_limit(
    current: usize,
    added: usize,
    maximum: usize,
    context: &'static str,
) -> Result<usize, RuntimeError> {
    let total = checked_add(current, added, context)?;
    if total > maximum {
        Err(limit(context))
    } else {
        Ok(total)
    }
}

pub(crate) fn hash_raw_system(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
