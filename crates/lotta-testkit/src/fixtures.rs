use crate::{FIXTURE_BYTES_MAX, TestkitError};
use serde::de::DeserializeOwned;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Loader confined to one canonical, static fixture tree.
///
/// Fixture trees are assumed not to be mutated concurrently with a load. The loader rejects
/// symlinks before opening and checks the canonical existing target, but does not attempt to
/// defend against an adversarial rename race in a repository controlled by the test process.
#[derive(Clone, Debug)]
pub struct FixtureLoader {
    root: PathBuf,
}

impl Default for FixtureLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureLoader {
    /// Creates a loader rooted at the workspace repository fixtures directory.
    #[must_use]
    pub fn new() -> Self {
        let configured = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        Self::from_configured_root(&configured).unwrap_or_else(|_| Self {
            root: PathBuf::new(),
        })
    }

    /// Creates a loader from a custom fixture root, canonicalized at construction time.
    ///
    /// # Errors
    /// Returns a confined filesystem error naming only `fixture-root` when the root is absent or
    /// cannot be canonicalized.
    pub fn from_root(root: impl AsRef<Path>) -> Result<Self, TestkitError> {
        Self::from_configured_root(root.as_ref())
    }

    fn from_configured_root(configured: &Path) -> Result<Self, TestkitError> {
        let root = configured
            .canonicalize()
            .map_err(|_| local_error("fixture-root"))?;
        if !root.is_dir() {
            return Err(local_error("fixture-root"));
        }
        Ok(Self { root })
    }

    /// Loads and generically deserializes one validated relative JSON fixture.
    ///
    /// # Errors
    /// Returns a typed missing, malformed, UTF-8, confined-path, limit, or filesystem error.
    pub fn load<T: DeserializeOwned>(&self, relative: impl AsRef<Path>) -> Result<T, TestkitError> {
        let (relative, display) = validate_relative(relative.as_ref())?;
        let path = self.root.join(&relative);
        reject_symlink_components(&self.root, &relative, &display)?;
        let canonical = canonical_target(&path, &display)?;
        if !canonical.starts_with(&self.root) {
            return Err(confined_error(&display));
        }
        let file = File::open(canonical).map_err(|error| open_error(error.kind(), &display))?;
        let bytes = bounded_read(file, &display)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| TestkitError::InvalidUtf8Fixture {
            path: display.clone(),
        })?;
        serde_json::from_str(text).map_err(|_| TestkitError::MalformedFixture { path: display })
    }
}

fn bounded_read(file: File, display: &str) -> Result<Vec<u8>, TestkitError> {
    let mut bytes = Vec::new();
    file.take(FIXTURE_BYTES_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| local_error(display))?;
    if bytes.len() as u64 > FIXTURE_BYTES_MAX {
        Err(TestkitError::FixtureLimit {
            path: display.into(),
        })
    } else {
        Ok(bytes)
    }
}

fn canonical_target(path: &Path, display: &str) -> Result<PathBuf, TestkitError> {
    path.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            TestkitError::MissingFixture {
                path: display.into(),
            }
        } else {
            local_error(display)
        }
    })
}

fn open_error(kind: std::io::ErrorKind, display: &str) -> TestkitError {
    if kind == std::io::ErrorKind::NotFound {
        TestkitError::MissingFixture {
            path: display.into(),
        }
    } else {
        local_error(display)
    }
}

fn reject_symlink_components(
    root: &Path,
    relative: &Path,
    display: &str,
) -> Result<(), TestkitError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(confined_error(display));
        };
        current.push(value);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(confined_error(display));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err(local_error(display)),
        }
    }
    Ok(())
}

fn validate_relative(path: &Path) -> Result<(PathBuf, String), TestkitError> {
    let label = confined_label(path);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(confined_error(&label));
    }
    let mut validated = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => validated.push(value),
            _ => return Err(confined_error(&label)),
        }
    }
    let display = format!("fixtures/{}", validated.display());
    Ok((validated, display))
}

fn confined_label(path: &Path) -> String {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .filter(|name| !name.is_empty() && *name != ".." && *name != ".")
        .unwrap_or("fixture-path")
        .into()
}

fn confined_error(path: &str) -> TestkitError {
    TestkitError::ConfinedPath { path: path.into() }
}

fn local_error(path: &str) -> TestkitError {
    TestkitError::LocalFilesystem { path: path.into() }
}

#[cfg(test)]
mod tests {
    use super::FixtureLoader;
    use crate::{FIXTURE_BYTES_MAX, TestkitError};
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CASE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    struct FixtureCase {
        path: std::path::PathBuf,
    }

    impl FixtureCase {
        fn new(label: &str) -> Self {
            let sequence = CASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "lotta-testkit-fixture-{label}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("fixture root");
            Self { path }
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("fixture parent");
            }
            let mut file = File::create(path).expect("fixture file");
            file.write_all(bytes).expect("fixture bytes");
        }

        fn loader(&self) -> FixtureLoader {
            FixtureLoader::from_root(&self.path).expect("loader")
        }
    }

    impl Drop for FixtureCase {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn assert_confined(error: &TestkitError, root: &Path, forbidden: &[&str]) {
        let display = error.to_string();
        let debug = format!("{error:?}");
        for text in [display.as_str(), debug.as_str()] {
            assert!(!text.contains(&root.to_string_lossy().to_string()));
            for value in forbidden {
                assert!(!text.contains(value));
            }
        }
    }

    fn json_with_exact_bytes(bytes: u64) -> Vec<u8> {
        let payload = usize::try_from(bytes - 2).expect("fixture size");
        format!("\"{}\"", "x".repeat(payload)).into_bytes()
    }

    #[test]
    fn fixtures_valid() {
        let case = FixtureCase::new("valid");
        case.write("nested/value.json", br#"{"valid":true}"#);
        let value: serde_json::Value = case.loader().load("nested/value.json").expect("valid");
        assert_eq!(value["valid"], true);
    }

    #[test]
    fn fixtures_missing() {
        let case = FixtureCase::new("missing");
        let error = case
            .loader()
            .load::<serde_json::Value>("absent.json")
            .unwrap_err();
        assert!(matches!(error, TestkitError::MissingFixture { .. }));
        assert_confined(&error, &case.path, &["No such file", "os error"]);
    }

    #[test]
    fn fixtures_malformed() {
        let case = FixtureCase::new("malformed");
        case.write("bad.json", b"{not-json");
        let error = case
            .loader()
            .load::<serde_json::Value>("bad.json")
            .unwrap_err();
        assert!(matches!(error, TestkitError::MalformedFixture { .. }));
        assert_confined(&error, &case.path, &["expected", "line", "column"]);
    }

    #[test]
    fn fixtures_traversal() {
        let case = FixtureCase::new("traversal");
        let error = case
            .loader()
            .load::<serde_json::Value>("../outside.json")
            .unwrap_err();
        assert!(matches!(error, TestkitError::ConfinedPath { .. }));
        assert_confined(&error, &case.path, &["../"]);
    }

    #[test]
    fn fixtures_absolute() {
        let case = FixtureCase::new("absolute");
        let absolute = case.path.join("secret.json");
        let error = case
            .loader()
            .load::<serde_json::Value>(&absolute)
            .unwrap_err();
        assert!(matches!(error, TestkitError::ConfinedPath { .. }));
        assert_confined(&error, &case.path, &[]);
    }

    #[cfg(unix)]
    #[test]
    fn fixtures_symlink_escape() {
        use std::os::unix::fs::symlink;

        let case = FixtureCase::new("symlink");
        let outside = FixtureCase::new("outside");
        outside.write("secret.json", br#"{"outside":true}"#);
        symlink(&outside.path, case.path.join("escape")).expect("symlink");
        let error = case
            .loader()
            .load::<serde_json::Value>("escape/secret.json")
            .unwrap_err();
        assert!(matches!(error, TestkitError::ConfinedPath { .. }));
        assert_confined(&error, &case.path, &["outside"]);
    }

    #[test]
    fn fixtures_invalid_utf8() {
        let case = FixtureCase::new("utf8");
        case.write("invalid.json", &[0xff, 0xfe]);
        let error = case
            .loader()
            .load::<serde_json::Value>("invalid.json")
            .unwrap_err();
        assert!(matches!(error, TestkitError::InvalidUtf8Fixture { .. }));
        assert_confined(&error, &case.path, &[]);
    }

    #[test]
    fn fixtures_below_exact_limit() {
        let case = FixtureCase::new("below");
        case.write("below.json", &json_with_exact_bytes(FIXTURE_BYTES_MAX - 1));
        let value: String = case.loader().load("below.json").expect("below limit");
        assert_eq!(value.len() as u64, FIXTURE_BYTES_MAX - 3);
    }

    #[test]
    fn fixtures_at_exact_limit_is_valid_json() {
        let case = FixtureCase::new("at");
        case.write("at.json", &json_with_exact_bytes(FIXTURE_BYTES_MAX));
        let value: String = case.loader().load("at.json").expect("at limit");
        assert_eq!(value.len() as u64, FIXTURE_BYTES_MAX - 2);
    }

    #[test]
    fn fixtures_above_exact_limit() {
        let case = FixtureCase::new("above");
        case.write("above.json", &json_with_exact_bytes(FIXTURE_BYTES_MAX + 1));
        let error = case.loader().load::<String>("above.json").unwrap_err();
        assert!(matches!(error, TestkitError::FixtureLimit { .. }));
        assert_eq!(
            error.to_string(),
            "fixture exceeds byte limit: fixtures/above.json"
        );
        assert_confined(&error, &case.path, &[]);
    }

    #[test]
    fn fixtures_root_is_cwd_independent() {
        let loader = FixtureLoader::new();
        let value: serde_json::Value = loader.load("testkit/valid.json").expect("before cwd");
        let original = std::env::current_dir().expect("cwd");
        let case = FixtureCase::new("cwd");
        std::env::set_current_dir(&case.path).expect("change cwd");
        let after = loader.load::<serde_json::Value>("testkit/valid.json");
        std::env::set_current_dir(original).expect("restore cwd");
        assert_eq!(after.expect("after cwd"), value);
    }
}
