//! Exact resolved pi-ai package pin.

use std::path::{Path, PathBuf};

/// Exact resolved compatibility package version.
pub const PI_AI_VERSION: &str = "0.82.1";
/// Exact package identity expected at the explicit package root.
pub const PI_AI_PACKAGE_NAME: &str = "@earendil-works/pi-ai";

/// Stable package-root validation failure.
#[derive(Debug, thiserror::Error)]
pub enum PinError {
    /// Filesystem input is absent, non-canonical, or not a regular package root.
    #[error("invalid pi-ai package root")]
    Root,
    /// Package identity or resolved version differs from the exact pin.
    #[error("pi-ai resolved version mismatch")]
    Version,
}

/// Validates an explicit canonical package root and its actual package manifest.
///
/// # Errors
/// Returns [`PinError`] for a non-canonical root or package identity/version mismatch.
pub fn validate_package_root(root: &Path) -> Result<PathBuf, PinError> {
    if !root.is_absolute()
        || root
            .symlink_metadata()
            .map_err(|_| PinError::Root)?
            .file_type()
            .is_symlink()
    {
        return Err(PinError::Root);
    }
    let canonical = root.canonicalize().map_err(|_| PinError::Root)?;
    if canonical != root || !canonical.is_dir() {
        return Err(PinError::Root);
    }
    let manifest = std::fs::read(canonical.join("package.json")).map_err(|_| PinError::Root)?;
    let value: serde_json::Value = serde_json::from_slice(&manifest).map_err(|_| PinError::Root)?;
    if value["name"] != PI_AI_PACKAGE_NAME || value["version"] != PI_AI_VERSION {
        return Err(PinError::Version);
    }
    Ok(canonical)
}

#[cfg(test)]
mod version_pin {
    use super::*;

    #[test]
    fn accepts_pinned_version_constant() {
        assert_eq!(PI_AI_VERSION, "0.82.1");
    }

    #[test]
    fn rejects_other_version() {
        let root = std::env::temp_dir().join(format!("lotta-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"@earendil-works/pi-ai","version":"0.82.2"}"#,
        )
        .unwrap();
        let canonical = root.canonicalize().unwrap();
        assert!(matches!(
            validate_package_root(&canonical),
            Err(PinError::Version)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
