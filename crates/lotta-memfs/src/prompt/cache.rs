use super::{CompiledPromptRecord, PromptCompiler, PromptInputs};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use lotta_runtime::RuntimeError;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

const CACHE_FILE: &str = "system-prompt.json";
const CACHE_RECORD_BYTES_MAX: usize = super::input::PROMPT_COMPILED_BYTES_MAX;
const CACHE_WRITE_RETRIES_MAX: usize = 3;
static CACHE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Provider capability controlling committed-memory delivery during an active conversation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryCapability {
    /// Deliver a one-shot system payload without replacing the stable base prompt.
    MidConversationSystem,
    /// Deliver a complete recompiled prompt at the current/next provider request boundary.
    RequestBoundaryOnly,
}

/// Prompt selected for provider delivery and the stable record persisted for later reuse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheDelivery {
    /// Provider-facing record; its optional mid field is transient.
    pub delivery: CompiledPromptRecord,
    /// Stable cache record after the operation.
    pub persisted: CompiledPromptRecord,
    /// Whether compilation rendered committed memory rather than reusing the cache.
    pub rendered: bool,
}

/// Validated absolute, existing, non-symlink conversation cache directory.
pub struct CacheRoot {
    directory: Arc<Dir>,
    #[cfg(test)]
    hooks: Option<Arc<TestHooks>>,
}

impl CacheRoot {
    /// Validates and retains an explicit cache authority directory.
    ///
    /// # Errors
    /// Rejects relative, missing, non-directory, symlink, or non-canonical roots.
    pub fn new(path: &Path) -> Result<Self, RuntimeError> {
        if !path.is_absolute() {
            return Err(invalid("absolute prompt cache root"));
        }
        let metadata = std::fs::symlink_metadata(path).map_err(|_| not_found())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("prompt cache root"));
        }
        let canonical = std::fs::canonicalize(path).map_err(|_| not_found())?;
        if canonical != path {
            return Err(invalid("prompt cache authority"));
        }
        let directory =
            Dir::open_ambient_dir(canonical, ambient_authority()).map_err(|_| permission())?;
        Ok(Self {
            directory: Arc::new(directory),
            #[cfg(test)]
            hooks: None,
        })
    }

    /// Loads the bounded exact compatibility record when it exists.
    ///
    /// # Errors
    /// Rejects symlinks, special files, oversized bytes, malformed JSON, and unknown fields.
    pub fn load(&self) -> Result<Option<CompiledPromptRecord>, RuntimeError> {
        #[cfg(test)]
        self.run_hook(HookPoint::Load)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(test)]
        self.run_hook(HookPoint::BeforeOpen)?;
        let file = match self.directory.open_with(CACHE_FILE, &options) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(invalid("prompt cache file")),
        };
        let metadata = file
            .metadata()
            .map_err(|_| adapter("prompt cache metadata"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(invalid("prompt cache file"));
        }
        let length = usize::try_from(metadata.len()).map_err(|_| limit())?;
        check_cache_bytes(length)?;
        let mut bytes = Vec::new();
        bytes.try_reserve(length).map_err(|_| limit())?;
        file.take((CACHE_RECORD_BYTES_MAX + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| adapter("prompt cache read"))?;
        check_cache_bytes(bytes.len())?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| invalid("prompt cache JSON"))
    }

    /// Atomically persists a stable record with temp-write-sync-rename-parent-sync durability.
    ///
    /// # Errors
    /// Rejects an existing symlink/special file, oversized JSON, allocation, or I/O failure.
    pub fn persist(&self, record: &CompiledPromptRecord) -> Result<(), RuntimeError> {
        #[cfg(test)]
        self.run_hook(HookPoint::Persist)?;
        validate_target(&self.directory)?;
        let stable = record.clone().stable();
        let mut bytes = BoundedCacheBytes::default();
        serde_json::to_writer_pretty(&mut bytes, &stable)
            .map_err(|_| invalid("prompt cache JSON"))?;
        bytes.write_all(b"\n").map_err(|_| limit())?;
        atomic_replace(&self.directory, &bytes.bytes)
    }

    /// Reuses or compiles against committed `MemFS` authority and persists stable delivery state.
    ///
    /// # Errors
    /// Returns typed cache, port, validation, limit, or cancellation failures.
    ///
    /// # Cancellation
    /// The token is observed while streaming the committed tree. Cancellation may discard a
    /// complete unpersisted render, but never exposes a partial cache record.
    pub async fn get_or_compile(
        &self,
        compiler: &PromptCompiler<'_>,
        inputs: &PromptInputs,
        capability: DeliveryCapability,
        cancellation: CancellationToken,
    ) -> Result<CacheDelivery, RuntimeError> {
        check_cancelled(&cancellation)?;
        let candidate_hash = super::compile::hash_raw_system(inputs.raw_system().as_str());
        check_cancelled(&cancellation)?;
        let revision = compiler.committed_revision(inputs.agent_id()).await?;
        check_cancelled(&cancellation)?;
        if let Some(existing) = self.load_async(cancellation.clone()).await? {
            if existing.raw_system_hash == candidate_hash
                && existing.memfs_revision == revision_string(revision.as_ref())
            {
                return Ok(reused(existing));
            }
            let same_hash = existing.raw_system_hash == candidate_hash;
            let fresh = compiler
                .compile_at_revision(inputs, revision.as_ref(), cancellation.clone())
                .await?;
            check_cancelled(&cancellation)?;
            return self
                .deliver_changed(existing, fresh, same_hash, capability, cancellation)
                .await;
        }
        let fresh = compiler
            .compile_at_revision(inputs, revision.as_ref(), cancellation.clone())
            .await?;
        check_cancelled(&cancellation)?;
        self.persist_async(fresh.clone(), cancellation).await?;
        Ok(rendered(fresh))
    }

    async fn load_async(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Option<CompiledPromptRecord>, RuntimeError> {
        check_cancelled(&cancellation)?;
        let root = Self {
            directory: Arc::clone(&self.directory),
            #[cfg(test)]
            hooks: self.hooks.clone(),
        };
        let result = tokio::task::spawn_blocking(move || root.load())
            .await
            .map_err(|_| adapter("prompt cache worker"))?;
        check_cancelled(&cancellation)?;
        result
    }

    async fn persist_async(
        &self,
        record: CompiledPromptRecord,
        cancellation: CancellationToken,
    ) -> Result<(), RuntimeError> {
        check_cancelled(&cancellation)?;
        let root = Self {
            directory: Arc::clone(&self.directory),
            #[cfg(test)]
            hooks: self.hooks.clone(),
        };
        let result = tokio::task::spawn_blocking(move || root.persist(&record))
            .await
            .map_err(|_| adapter("prompt cache worker"))?;
        check_cancelled(&cancellation)?;
        result
    }

    #[cfg(test)]
    fn with_hooks(mut self, hooks: Arc<TestHooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    #[cfg(test)]
    fn run_hook(&self, point: HookPoint) -> Result<(), RuntimeError> {
        if let Some(hooks) = &self.hooks {
            hooks.run(point)?;
        }
        Ok(())
    }

    async fn deliver_changed(
        &self,
        existing: CompiledPromptRecord,
        compiled: CompiledPromptRecord,
        same_hash: bool,
        capability: DeliveryCapability,
        cancellation: CancellationToken,
    ) -> Result<CacheDelivery, RuntimeError> {
        if same_hash && capability == DeliveryCapability::MidConversationSystem {
            let persisted = stable_memory_update(&existing, &compiled)?;
            self.persist_async(persisted.clone(), cancellation.clone())
                .await?;
            let mut delivery = existing;
            delivery.core_memory.clone_from(&compiled.core_memory);
            delivery.compiled_at = compiled.compiled_at;
            delivery.memfs_revision.clone_from(&compiled.memfs_revision);
            delivery.mid_conversation_system_prompt = Some(memory_update(&compiled)?);
            return Ok(CacheDelivery {
                delivery,
                persisted,
                rendered: true,
            });
        }
        self.persist_async(compiled.clone(), cancellation).await?;
        Ok(rendered(compiled))
    }
}

fn revision_string(revision: Option<&lotta_runtime::boundary::RevisionId>) -> Option<String> {
    revision.map(|value| value.as_str().to_owned())
}

fn reused(record: CompiledPromptRecord) -> CacheDelivery {
    CacheDelivery {
        delivery: record.clone(),
        persisted: record,
        rendered: false,
    }
}

fn rendered(record: CompiledPromptRecord) -> CacheDelivery {
    CacheDelivery {
        delivery: record.clone(),
        persisted: record,
        rendered: true,
    }
}

fn stable_memory_update(
    existing: &CompiledPromptRecord,
    compiled: &CompiledPromptRecord,
) -> Result<CompiledPromptRecord, RuntimeError> {
    checked_combined(existing.content.len(), compiled.core_memory.len())?;
    Ok(CompiledPromptRecord {
        content: existing.content.clone(),
        core_memory: compiled.core_memory.clone(),
        mid_conversation_system_prompt: None,
        compiled_at: compiled.compiled_at,
        raw_system_hash: existing.raw_system_hash.clone(),
        memfs_revision: compiled.memfs_revision.clone(),
    })
}

fn memory_update(compiled: &CompiledPromptRecord) -> Result<String, RuntimeError> {
    let revision = compiled
        .memfs_revision
        .as_ref()
        .map_or("unknown", String::as_str);
    let prefix =
        "<memory_update>\nThe local memory filesystem has been edited and committed at revision ";
    let guidance = concat!(
        ".\nThis updates part of your persona/system memory. Treat the following freshly ",
        "rendered memory context as authoritative from now on; where it conflicts with earlier ",
        "memory context, this newer memory context wins.\n\n",
    );
    let core = compiled.core_memory.trim_end();
    let mut required = checked_combined(prefix.len(), revision.len())?;
    required = checked_combined(required, guidance.len())?;
    required = checked_combined(required, core.len())?;
    required = checked_combined(required, "\n</memory_update>".len())?;
    let mut value = String::new();
    value.try_reserve(required).map_err(|_| limit())?;
    value.push_str(prefix);
    value.push_str(revision);
    value.push_str(guidance);
    value.push_str(core);
    value.push_str("\n</memory_update>");
    Ok(value)
}

fn validate_target(directory: &Dir) -> Result<(), RuntimeError> {
    match directory.symlink_metadata(CACHE_FILE) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(invalid("prompt cache file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(adapter("prompt cache metadata")),
    }
}

fn atomic_replace(directory: &Dir, bytes: &[u8]) -> Result<(), RuntimeError> {
    atomic_replace_core(directory, bytes, |attempt| {
        let sequence = CACHE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Ok(format!(
            ".lotta-prompt-{}-{sequence}-{attempt}",
            std::process::id()
        ))
    })
}

fn atomic_replace_core<F>(directory: &Dir, bytes: &[u8], candidate: F) -> Result<(), RuntimeError>
where
    F: FnMut(usize) -> Result<String, RuntimeError>,
{
    atomic_replace_observed(directory, bytes, candidate, |_, _, _, _| Ok(()))
}

fn atomic_replace_observed<F, H>(
    directory: &Dir,
    bytes: &[u8],
    mut candidate: F,
    mut hook: H,
) -> Result<(), RuntimeError>
where
    F: FnMut(usize) -> Result<String, RuntimeError>,
    H: FnMut(WriteStage, &Dir, &str, &mut cap_std::fs::File) -> Result<(), RuntimeError>,
{
    for attempt in 0..CACHE_WRITE_RETRIES_MAX {
        let temporary = candidate(attempt)?;
        validate_candidate(&temporary)?;
        let Some(mut file) = create_temporary(directory, &temporary)? else {
            continue;
        };
        let result = write_and_replace(directory, &temporary, &mut file, bytes, &mut hook);
        if result.is_err() {
            drop(directory.remove_file(&temporary));
        }
        return result;
    }
    Err(RuntimeError::Conflict {
        context: "prompt cache temporary collision".into(),
    })
}

fn create_temporary(
    directory: &Dir,
    temporary: &str,
) -> Result<Option<cap_std::fs::File>, RuntimeError> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    match directory.open_with(temporary, &options) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(_) => Err(adapter("prompt cache temporary file")),
    }
}

fn validate_candidate(value: &str) -> Result<(), RuntimeError> {
    let mut components = Path::new(value).components();
    let single = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if single && !value.contains(['/', '\\', '\0']) && value != "." && value != ".." {
        Ok(())
    } else {
        Err(invalid("prompt cache temporary name"))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum WriteStage {
    Write,
    Sync,
    Rename,
}

fn write_and_replace<H>(
    directory: &Dir,
    temporary: &str,
    file: &mut cap_std::fs::File,
    bytes: &[u8],
    hook: &mut H,
) -> Result<(), RuntimeError>
where
    H: FnMut(WriteStage, &Dir, &str, &mut cap_std::fs::File) -> Result<(), RuntimeError>,
{
    hook(WriteStage::Write, directory, temporary, file)?;
    file.write_all(bytes)
        .map_err(|_| adapter("prompt cache write"))?;
    hook(WriteStage::Sync, directory, temporary, file)?;
    file.sync_all().map_err(|_| adapter("prompt cache sync"))?;
    validate_target(directory)?;
    hook(WriteStage::Rename, directory, temporary, file)?;
    Dir::rename(directory, temporary, directory, CACHE_FILE)
        .map_err(|_| adapter("prompt cache replace"))?;
    directory
        .try_clone()
        .map(Dir::into_std_file)
        .and_then(|parent| parent.sync_all())
        .map_err(|_| adapter("prompt cache parent sync"))
}

struct BoundedCacheBytes {
    bytes: Vec<u8>,
    limit: usize,
}

impl Default for BoundedCacheBytes {
    fn default() -> Self {
        Self {
            bytes: Vec::new(),
            limit: CACHE_RECORD_BYTES_MAX,
        }
    }
}

impl BoundedCacheBytes {
    #[cfg(test)]
    fn with_limit(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
}

impl Write for BoundedCacheBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let required = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(limit_io)?;
        if required > self.limit {
            return Err(limit_io());
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| limit_io())?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn limit_io() -> std::io::Error {
    std::io::Error::other("prompt cache bytes")
}

fn checked_combined(left: usize, right: usize) -> Result<usize, RuntimeError> {
    let total = left.checked_add(right).ok_or_else(limit)?;
    check_cache_bytes(total)?;
    Ok(total)
}

fn check_cache_bytes(length: usize) -> Result<(), RuntimeError> {
    if length > CACHE_RECORD_BYTES_MAX {
        Err(limit())
    } else {
        Ok(())
    }
}
fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}
fn not_found() -> RuntimeError {
    RuntimeError::NotFound {
        context: "prompt cache root".into(),
    }
}
fn permission() -> RuntimeError {
    RuntimeError::PermissionDenied {
        context: "prompt cache root".into(),
    }
}
fn limit() -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: "prompt cache bytes".into(),
    }
}
fn adapter(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "prompt_cache",
        context: context.into(),
    }
}

fn check_cancelled(token: &CancellationToken) -> Result<(), RuntimeError> {
    if token.is_cancelled() {
        Err(RuntimeError::Cancelled {
            context: "prompt cache".into(),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Eq, PartialEq)]
enum HookPoint {
    Load,
    Persist,
    BeforeOpen,
}

#[cfg(test)]
struct TestHooks {
    point: HookPoint,
    entered: AtomicBool,
    released: AtomicBool,
    calls: AtomicUsize,
    action: Option<Arc<dyn Fn() -> Result<(), RuntimeError> + Send + Sync>>,
}

#[cfg(test)]
impl TestHooks {
    fn blocking(point: HookPoint) -> Arc<Self> {
        Arc::new(Self {
            point,
            entered: AtomicBool::new(false),
            released: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            action: None,
        })
    }

    fn action(
        point: HookPoint,
        action: impl Fn() -> Result<(), RuntimeError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            point,
            entered: AtomicBool::new(false),
            released: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            action: Some(Arc::new(action)),
        })
    }

    fn run(&self, point: HookPoint) -> Result<(), RuntimeError> {
        if point != self.point {
            return Ok(());
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.store(true, Ordering::SeqCst);
        while !self.released.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        if let Some(action) = &self.action {
            action()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::test_support::*;
    use crate::tests::{message, path, valid};
    use lotta_runtime::ports::MemFsPort;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn wait_entered(hooks: &TestHooks) {
        while !hooks.entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    }

    async fn nonblocking_case(point: HookPoint, preload: bool) {
        let (root, port, _) = setup().await;
        let name = if preload {
            "nonblocking-load"
        } else {
            "nonblocking-persist"
        };
        let store = cache(&root, name);
        if preload {
            store.persist(&record("raw")).expect("preload");
        }
        let hooks = TestHooks::blocking(point);
        let store = store.with_hooks(Arc::clone(&hooks));
        let renders = AtomicUsize::new(0);
        let progress = Arc::new(AtomicUsize::new(0));
        let work = async {
            store
                .get_or_compile(
                    &compiler(&port, &renders),
                    &inputs("raw", "2000-01-01T00:00:00Z"),
                    DeliveryCapability::RequestBoundaryOnly,
                    CancellationToken::new(),
                )
                .await
        };
        let unrelated = async {
            wait_entered(&hooks).await;
            progress.fetch_add(1, Ordering::SeqCst);
            hooks.released.store(true, Ordering::SeqCst);
        };
        let (result, ()) = tokio::join!(work, unrelated);
        result.expect("operation");
        assert_eq!(progress.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.calls.load(Ordering::SeqCst), 1);
    }

    fn record(raw: &str) -> CompiledPromptRecord {
        CompiledPromptRecord {
            content: raw.to_owned(),
            core_memory: String::new(),
            mid_conversation_system_prompt: None,
            compiled_at: inputs(raw, "2000-01-01T00:00:00Z").compiled_at(),
            raw_system_hash: super::super::compile::hash_raw_system(raw),
            memfs_revision: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_and_persist_hooks_are_nonblocking() {
        nonblocking_case(HookPoint::Load, true).await;
        nonblocking_case(HookPoint::Persist, false).await;
    }

    #[test]
    fn deterministic_temp_collisions_retry_then_succeed() {
        let root = crate::tests::TestRoot::new();
        let directory = root.0.join("collision-success");
        std::fs::create_dir(&directory).expect("directory");
        let dir = Dir::open_ambient_dir(&directory, ambient_authority()).expect("open");
        for name in ["temp-0", "temp-1"] {
            std::fs::write(directory.join(name), b"occupied").expect("collision");
        }
        atomic_replace_core(&dir, b"exact", |attempt| Ok(format!("temp-{attempt}")))
            .expect("replace");
        assert_eq!(
            std::fs::read(directory.join(CACHE_FILE)).expect("target"),
            b"exact"
        );
    }

    #[test]
    fn deterministic_temp_collision_exhaustion_preserves_target() {
        let root = crate::tests::TestRoot::new();
        let directory = root.0.join("collision-exhausted");
        std::fs::create_dir(&directory).expect("directory");
        std::fs::write(directory.join(CACHE_FILE), b"prior").expect("prior");
        for attempt in 0..CACHE_WRITE_RETRIES_MAX {
            std::fs::write(directory.join(format!("temp-{attempt}")), b"occupied")
                .expect("collision");
        }
        let dir = Dir::open_ambient_dir(&directory, ambient_authority()).expect("open");
        let result = atomic_replace_core(&dir, b"new", |attempt| Ok(format!("temp-{attempt}")));
        assert!(matches!(result, Err(RuntimeError::Conflict { .. })));
        assert_eq!(
            std::fs::read(directory.join(CACHE_FILE)).expect("target"),
            b"prior"
        );
    }

    #[test]
    fn injected_write_sync_and_rename_failures_cleanup_temps() {
        for stage in [WriteStage::Write, WriteStage::Sync, WriteStage::Rename] {
            let root = crate::tests::TestRoot::new();
            let directory = root.0.join(format!("failure-{}", stage as u8));
            std::fs::create_dir(&directory).expect("directory");
            std::fs::write(directory.join(CACHE_FILE), b"prior").expect("prior");
            let dir = Dir::open_ambient_dir(&directory, ambient_authority()).expect("open");
            let result = atomic_replace_observed(
                &dir,
                b"new",
                |_| Ok("temporary".to_owned()),
                |point, _, _, _| {
                    if point == stage {
                        Err(adapter("injected failure"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(matches!(result, Err(RuntimeError::AdapterFailure { .. })));
            assert!(!directory.join("temporary").exists());
            assert_eq!(
                std::fs::read(directory.join(CACHE_FILE)).expect("target"),
                b"prior"
            );
        }
    }

    #[test]
    fn candidate_names_reject_structural_components() {
        for value in ["", ".", "..", "a/b", "a\\b", "/absolute", "nul\0name"] {
            assert!(matches!(
                validate_candidate(value),
                Err(RuntimeError::InvalidData { .. })
            ));
        }
        assert!(validate_candidate("single-name").is_ok());
    }

    #[test]
    fn bounded_json_sink_below_at_and_above() {
        let mut below = BoundedCacheBytes::with_limit(3);
        assert_eq!(below.write(b"ab").expect("below"), 2);
        let mut at = BoundedCacheBytes::with_limit(3);
        assert_eq!(at.write(b"abc").expect("at"), 3);
        let mut above = BoundedCacheBytes::with_limit(3);
        assert!(above.write(b"abcd").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn nofollow_swap_immediately_before_open_is_rejected() {
        use std::os::unix::fs::symlink;
        let root = crate::tests::TestRoot::new();
        let directory = root.0.join("swap-cache");
        std::fs::create_dir(&directory).expect("directory");
        let target = directory.join(CACHE_FILE);
        std::fs::write(&target, b"ordinary").expect("ordinary");
        let outside = root.0.join("outside.json");
        std::fs::write(&outside, b"sentinel").expect("outside");
        let moved = directory.join("ordinary.json");
        let target_for_hook = target.clone();
        let outside_for_hook = outside.clone();
        let hook = TestHooks::action(HookPoint::BeforeOpen, move || {
            std::fs::rename(&target_for_hook, &moved).map_err(|_| adapter("swap"))?;
            symlink(&outside_for_hook, &target_for_hook).map_err(|_| adapter("symlink"))
        });
        let store = CacheRoot::new(&std::fs::canonicalize(&directory).expect("canonical"))
            .expect("cache")
            .with_hooks(hook);
        assert!(matches!(
            store.load(),
            Err(RuntimeError::InvalidData { .. })
        ));
        assert_eq!(std::fs::read(outside).expect("outside read"), b"sentinel");
    }

    #[cfg(unix)]
    #[test]
    fn retained_root_survives_ambient_path_symlink_swap() {
        use std::os::unix::fs::symlink;
        let root = crate::tests::TestRoot::new();
        let directory = root.0.join("retained-cache");
        let moved = root.0.join("retained-moved");
        let outside = root.0.join("outside-dir");
        std::fs::create_dir(&directory).expect("directory");
        std::fs::create_dir(&outside).expect("outside");
        std::fs::write(outside.join("sentinel"), b"safe").expect("sentinel");
        let store =
            CacheRoot::new(&std::fs::canonicalize(&directory).expect("canonical")).expect("cache");
        std::fs::rename(&directory, &moved).expect("move root");
        symlink(&outside, &directory).expect("replace with symlink");
        let expected = record("retained");
        store.persist(&expected).expect("persist retained");
        assert_eq!(store.load().expect("load"), Some(expected.clone()));
        assert_eq!(
            serde_json::from_slice::<CompiledPromptRecord>(
                &std::fs::read(moved.join(CACHE_FILE)).expect("moved record")
            )
            .expect("record"),
            expected
        );
        assert!(!outside.join(CACHE_FILE).exists());
        assert_eq!(
            std::fs::read(outside.join("sentinel")).expect("sentinel"),
            b"safe"
        );
    }

    #[tokio::test]
    async fn unchanged_pair_reuses() {
        let (root, port, _) = setup().await;
        let store = cache(&root, "owned-cache-one");
        let renders = AtomicUsize::new(0);
        for at in ["2000-01-01T00:00:00Z", "2001-01-01T00:00:00Z"] {
            store
                .get_or_compile(
                    &compiler(&port, &renders),
                    &inputs("raw", at),
                    DeliveryCapability::RequestBoundaryOnly,
                    CancellationToken::new(),
                )
                .await
                .expect("cache");
        }
        assert_eq!(renders.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn changed_hash_recompiles() {
        let (root, port, _) = setup().await;
        let store = cache(&root, "owned-cache-two");
        let renders = AtomicUsize::new(0);
        for raw in ["one", "two"] {
            store
                .get_or_compile(
                    &compiler(&port, &renders),
                    &inputs(raw, "2000-01-01T00:00:00Z"),
                    DeliveryCapability::RequestBoundaryOnly,
                    CancellationToken::new(),
                )
                .await
                .expect("cache");
        }
        assert_eq!(renders.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn changed_revision_recompiles() {
        let (root, port, agent) = setup().await;
        let store = cache(&root, "owned-cache-three");
        let renders = AtomicUsize::new(0);
        store
            .get_or_compile(
                &compiler(&port, &renders),
                &inputs("raw", "2000-01-01T00:00:00Z"),
                DeliveryCapability::RequestBoundaryOnly,
                CancellationToken::new(),
            )
            .await
            .expect("first");
        port.write(&agent, &path("system/persona.md"), &valid("changed"))
            .await
            .expect("write");
        port.commit(&agent, &message("changed"))
            .await
            .expect("commit");
        store
            .get_or_compile(
                &compiler(&port, &renders),
                &inputs("raw", "2001-01-01T00:00:00Z"),
                DeliveryCapability::RequestBoundaryOnly,
                CancellationToken::new(),
            )
            .await
            .expect("second");
        assert_eq!(renders.load(Ordering::SeqCst), 2);
    }
}
