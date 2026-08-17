//! Production external process launcher for the pinned mod bridge.

use super::host::{KillSwitch, ModHostChild, ModHostLauncher};
use super::types::{ModError, ModOwner};
use crate::sidecar::SidecarOwnerIdentity;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;

const BRIDGE_BYTES: &[u8] = include_bytes!("../../assets/mod-host-bridge.mjs");
const BRIDGE_SHA256: &str = "ab56f395b5a845777be7005df9728f8a9f21d4c40d8de2436a239a7005f0e5ab";
const BRIDGE_FILE_NAME: &str = "mod-host-bridge.mjs";
const MOD_HOST_STDERR_BYTES_MAX: usize = 64 * 1024;

/// Validated process inputs for one external mod generation.
#[derive(Clone, Debug)]
pub struct ModHostSpec {
    /// Absolute executable path, typically an installed Node or Bun binary.
    pub runtime_executable: PathBuf,
    /// Absolute `.mjs`, `.js`, or `.ts` mod entry.
    pub mod_entry: PathBuf,
    /// Absolute directory used as the child working directory.
    pub cwd: PathBuf,
    /// Explicit bridge cache root.
    pub cache_root: PathBuf,
    /// Exact owner passed to the bridge.
    pub owner: ModOwner,
    /// Task 42 owner identity passed to the bridge.
    pub sidecar_owner: SidecarOwnerIdentity,
    /// Explicit child environment; ambient variables are never inherited.
    pub env: BTreeMap<String, String>,
}

/// Direct, shell-free production launcher.
pub struct OsModHostLauncher {
    spec: ModHostSpec,
}
impl OsModHostLauncher {
    /// Creates a launcher after validating all filesystem and environment inputs.
    pub fn new(spec: ModHostSpec) -> Result<Self, ModError> {
        validate_spec(&spec)?;
        Ok(Self { spec })
    }
}

/// Owned OS child and exact duplex process pipes.
pub struct OsModHostChild {
    child: Arc<OwnedChild>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr_task: Option<JoinHandle<Vec<u8>>>,
}

struct OwnedChild {
    child: Mutex<Option<Child>>,
}
impl OwnedChild {
    fn new(child: Child) -> Self {
        Self {
            child: Mutex::new(Some(child)),
        }
    }

    fn terminate(&self) {
        let Ok(mut guard) = self.child.lock() else {
            return;
        };
        let Some(child) = guard.as_mut() else {
            return;
        };
        #[cfg(unix)]
        if let Some(group) = child.id().and_then(|id| i32::try_from(id).ok()) {
            signal_group(group);
        }
        let _ = child.start_kill();
    }

    fn take(&self) -> Result<Child, ModError> {
        self.child
            .lock()
            .map_err(|_| ModError::Unavailable)?
            .take()
            .ok_or(ModError::Unavailable)
    }
}

impl ModHostLauncher for OsModHostLauncher {
    type Child = OsModHostChild;
    type Launch<'a> = Pin<Box<dyn Future<Output = Result<Self::Child, ModError>> + Send + 'a>>;

    fn launch(&mut self) -> Self::Launch<'_> {
        Box::pin(async move {
            let bridge = materialize_bridge(&self.spec.cache_root).await?;
            let owner = serde_json::to_string(&self.spec.owner).map_err(|_| ModError::Protocol)?;
            let sidecar_owner =
                serde_json::to_string(&self.spec.sidecar_owner).map_err(|_| ModError::Protocol)?;
            let mut command = Command::new(&self.spec.runtime_executable);
            command
                .arg(bridge)
                .arg(&self.spec.mod_entry)
                .arg(owner)
                .arg(sidecar_owner)
                .current_dir(&self.spec.cwd)
                .env_clear()
                .envs(&self.spec.env)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            command.process_group(0);
            let mut child = command.spawn().map_err(|_| ModError::Unavailable)?;
            let stdin = child.stdin.take().ok_or(ModError::Unavailable)?;
            let stdout = child.stdout.take().ok_or(ModError::Unavailable)?;
            let stderr = child.stderr.take().ok_or(ModError::Unavailable)?;
            let stderr_task = tokio::spawn(read_bounded_stderr(stderr));
            Ok(OsModHostChild {
                child: Arc::new(OwnedChild::new(child)),
                stdin: Some(stdin),
                stdout: Some(stdout),
                stderr_task: Some(stderr_task),
            })
        })
    }
}

impl ModHostChild for OsModHostChild {
    type Reader = ChildStdout;
    type Writer = ChildStdin;
    type Stop<'a> = Pin<Box<dyn Future<Output = Result<(), ModError>> + Send + 'a>>;
    type Join<'a> = Pin<Box<dyn Future<Output = Result<(), ModError>> + Send + 'a>>;

    fn take_pipes(&mut self) -> Result<(Self::Reader, Self::Writer), ModError> {
        Ok((
            self.stdout.take().ok_or(ModError::Unavailable)?,
            self.stdin.take().ok_or(ModError::Unavailable)?,
        ))
    }

    fn stop(&mut self) -> Self::Stop<'_> {
        Box::pin(async move {
            self.stdin.take();
            self.child.terminate();
            Ok(())
        })
    }

    fn join(&mut self) -> Self::Join<'_> {
        Box::pin(async move {
            let mut child = self.child.take()?;
            child.wait().await.map_err(|_| ModError::Unavailable)?;
            if let Some(task) = self.stderr_task.take() {
                let _ = task.await;
            }
            Ok(())
        })
    }

    fn kill_switch(&self) -> KillSwitch {
        let child = Arc::clone(&self.child);
        KillSwitch::new(move || child.terminate())
    }
}

impl Drop for OsModHostChild {
    fn drop(&mut self) {
        self.child.terminate();
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

fn validate_spec(spec: &ModHostSpec) -> Result<(), ModError> {
    if !absolute_file(&spec.runtime_executable)
        || !absolute_file(&spec.mod_entry)
        || !spec.cwd.is_absolute()
        || !spec.cwd.is_dir()
        || !spec.cache_root.is_absolute()
        || spec.env.iter().any(|(key, value)| invalid_env(key, value))
    {
        return Err(ModError::InvalidScope);
    }
    let extension = spec.mod_entry.extension().and_then(|value| value.to_str());
    if !matches!(extension, Some("mjs" | "js" | "ts")) {
        return Err(ModError::InvalidScope);
    }
    Ok(())
}

fn absolute_file(path: &Path) -> bool {
    path.is_absolute() && path.is_file()
}

fn invalid_env(key: &str, value: &str) -> bool {
    key.is_empty() || key.contains(['=', '\0']) || value.contains('\0')
}

async fn materialize_bridge(root: &Path) -> Result<PathBuf, ModError> {
    if bridge_digest(BRIDGE_BYTES) != BRIDGE_SHA256 {
        return Err(ModError::Protocol);
    }
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|_| ModError::Unavailable)?;
    let path = root.join(BRIDGE_FILE_NAME);
    if let Ok(existing) = tokio::fs::read(&path).await
        && bridge_digest(&existing) == BRIDGE_SHA256
    {
        return Ok(path);
    }
    let temporary = root.join(format!(".{BRIDGE_FILE_NAME}.{}.tmp", std::process::id()));
    tokio::fs::write(&temporary, BRIDGE_BYTES)
        .await
        .map_err(|_| ModError::Unavailable)?;
    tokio::fs::rename(&temporary, &path)
        .await
        .map_err(|_| ModError::Unavailable)?;
    Ok(path)
}

fn bridge_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

async fn read_bounded_stderr(stderr: tokio::process::ChildStderr) -> Vec<u8> {
    let mut reader = BufReader::new(stderr).take(MOD_HOST_STDERR_BYTES_MAX as u64);
    let mut bytes = Vec::new();
    let _ = reader.read_to_end(&mut bytes).await;
    bytes
}

#[cfg(unix)]
fn signal_group(group: i32) {
    use rustix::process::{Pid, Signal, kill_process_group};
    if let Some(group) = Pid::from_raw(group) {
        let _ = kill_process_group(group, Signal::KILL);
    }
}
#[cfg(not(unix))]
fn signal_group(_: i32) {}
