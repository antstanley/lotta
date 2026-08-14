use crate::TestkitError;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const ROOT_PREFIX: &str = "lotta-testkit-";
const MARKER_NAME: &str = ".lotta-testkit-owner";
const CREATE_RETRIES_MAX: u64 = 64;
const LABEL_BYTES_MAX: usize = 64;
const IDENTITY_BYTES_MAX: u64 = 64;

/// Owned OS-temporary directory protected by a private identity marker.
#[derive(Debug)]
pub struct TemporaryRoot {
    path: PathBuf,
    parent: PathBuf,
    identity: String,
    label: String,
    cleaned: bool,
}

impl TemporaryRoot {
    /// Atomically creates a process-unique root without randomness or wall time.
    ///
    /// # Errors
    /// Rejects malformed labels, exhausted counters, and local filesystem failures.
    pub fn new(label: &str) -> Result<Self, TestkitError> {
        validate_label(label)?;
        let parent = canonical_temp_parent(label)?;
        for _ in 0..CREATE_RETRIES_MAX {
            let sequence = next_sequence()?;
            let identity = format!("{}-{sequence}", std::process::id());
            let name = format!("{ROOT_PREFIX}{label}-{identity}");
            let path = parent.join(name);
            match std::fs::create_dir(&path) {
                Ok(()) => return Self::finish_create(path, parent, identity, label),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(filesystem_error(label)),
            }
        }
        Err(TestkitError::LimitExceeded {
            context: "temporary_root_create_retries_max",
        })
    }

    fn finish_create(
        path: PathBuf,
        parent: PathBuf,
        identity: String,
        label: &str,
    ) -> Result<Self, TestkitError> {
        let marker = path.join(MARKER_NAME);
        let result = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)
            .and_then(|mut file| file.write_all(identity.as_bytes()));
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&path);
            return Err(filesystem_error(label));
        }
        Ok(Self {
            path,
            parent,
            identity,
            label: label.into(),
            cleaned: false,
        })
    }

    /// Borrows the owned root path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes the original root if its private identity marker still matches.
    ///
    /// Repeated calls succeed. Replaced or modified roots are deliberately left untouched.
    ///
    /// # Errors
    /// Returns a confined local filesystem error if marker inspection or removal fails.
    pub fn cleanup(&mut self) -> Result<(), TestkitError> {
        if self.cleaned {
            return Ok(());
        }
        if !self
            .path
            .try_exists()
            .map_err(|_| filesystem_error(&self.label))?
        {
            self.cleaned = true;
            return Ok(());
        }
        if !self.path.starts_with(&self.parent) || self.path.parent() != Some(self.parent.as_path())
        {
            return Ok(());
        }
        if !marker_matches(&self.path, &self.identity, &self.label)? {
            return Ok(());
        }
        std::fs::remove_dir_all(&self.path).map_err(|_| filesystem_error(&self.label))?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for TemporaryRoot {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn marker_matches(path: &Path, identity: &str, label: &str) -> Result<bool, TestkitError> {
    let marker = path.join(MARKER_NAME);
    let mut file = match OpenOptions::new().read(true).open(marker) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(filesystem_error(label)),
    };
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(IDENTITY_BYTES_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| filesystem_error(label))?;
    Ok(bytes == identity.as_bytes())
}

fn canonical_temp_parent(label: &str) -> Result<PathBuf, TestkitError> {
    std::env::temp_dir()
        .canonicalize()
        .map_err(|_| filesystem_error(label))
}

fn next_sequence() -> Result<u64, TestkitError> {
    ROOT_SEQUENCE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| TestkitError::SequenceExhausted {
            context: "temporary_root_sequence",
        })
}

fn filesystem_error(label: &str) -> TestkitError {
    TestkitError::LocalFilesystem {
        path: safe_label(label),
    }
}

fn safe_label(label: &str) -> String {
    if validate_label(label).is_ok() {
        label.into()
    } else {
        "temporary-root-label".into()
    }
}

fn validate_label(label: &str) -> Result<(), TestkitError> {
    let valid = !label.is_empty()
        && label.len() <= LABEL_BYTES_MAX
        && label.is_ascii()
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if valid && label != "." && label != ".." {
        Ok(())
    } else {
        Err(TestkitError::ConfinedPath {
            path: "temporary-root-label".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{MARKER_NAME, ROOT_PREFIX, ROOT_SEQUENCE, TemporaryRoot};
    use crate::TestkitError;
    use std::fs;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Barrier};

    fn assert_no_absolute(error: &TestkitError) {
        let temp = std::env::temp_dir().to_string_lossy().into_owned();
        assert!(!error.to_string().contains(&temp));
        assert!(!format!("{error:?}").contains(&temp));
    }

    #[test]
    fn roots_parallel_unique_names() {
        let barrier = Arc::new(Barrier::new(16));
        let handles = (0..16)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let root = TemporaryRoot::new("parallel").expect("root");
                    root.path().to_owned()
                })
            })
            .collect::<Vec<_>>();
        let mut names = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 16);
        assert!(names.iter().all(|path| !path.exists()));
    }

    #[test]
    fn roots_forced_collision_stale_retry() {
        let sequence = ROOT_SEQUENCE.load(Ordering::Relaxed);
        let name = format!("{ROOT_PREFIX}collision-{}-{sequence}", std::process::id());
        let stale = std::env::temp_dir()
            .canonicalize()
            .expect("temp")
            .join(name);
        fs::create_dir(&stale).expect("stale root");
        let root = TemporaryRoot::new("collision").expect("retry root");
        assert_ne!(root.path(), stale);
        assert!(stale.exists());
        fs::remove_dir(stale).expect("remove stale");
    }

    #[test]
    fn roots_explicit_cleanup_twice() {
        let mut root = TemporaryRoot::new("twice").expect("root");
        let path = root.path().to_owned();
        root.cleanup().expect("first cleanup");
        root.cleanup().expect("second cleanup");
        assert!(!path.exists());
    }

    #[test]
    fn roots_drop_cleanup() {
        let path = {
            let root = TemporaryRoot::new("drop").expect("root");
            root.path().to_owned()
        };
        assert!(!path.exists());
    }

    #[test]
    fn roots_removed_and_replaced_safety() {
        let mut root = TemporaryRoot::new("replaced").expect("root");
        let path = root.path().to_owned();
        fs::remove_dir_all(&path).expect("remove original");
        fs::create_dir(&path).expect("replacement");
        fs::write(path.join("keep"), b"replacement").expect("replacement file");
        root.cleanup().expect("safe cleanup");
        assert_eq!(fs::read(path.join("keep")).expect("kept"), b"replacement");
        fs::remove_dir_all(path).expect("remove replacement");
    }

    #[test]
    fn roots_marker_changed_safety() {
        let mut root = TemporaryRoot::new("marker").expect("root");
        let path = root.path().to_owned();
        fs::write(path.join(MARKER_NAME), b"changed").expect("change marker");
        root.cleanup().expect("safe cleanup");
        assert!(path.exists());
        fs::remove_dir_all(path).expect("remove retained root");
    }

    #[test]
    fn roots_invalid_labels() {
        for label in ["", ".", "..", "../escape", "a/b", "a\\b", "space here", "é"] {
            let error = TemporaryRoot::new(label).unwrap_err();
            assert!(matches!(error, TestkitError::ConfinedPath { .. }));
            assert_no_absolute(&error);
        }
        let overbound = "a".repeat(65);
        let error = TemporaryRoot::new(&overbound).unwrap_err();
        assert!(matches!(error, TestkitError::ConfinedPath { .. }));
        assert_no_absolute(&error);
    }
}
