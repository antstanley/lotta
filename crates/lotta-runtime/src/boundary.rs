//! Purpose-specific validated values crossing runtime port boundaries.

use crate::RuntimeError;
use crate::bounds::{
    COMMIT_MESSAGE_BYTES_MAX, CONFINED_PATH_BYTES_MAX, CONFINED_PATH_COMPONENTS_MAX,
    MEMFS_DIFF_CHUNK_BYTES_MAX, MEMORY_FILE_BYTES_MAX, MEMORY_FILES_MAX,
    PROCESS_ARGUMENT_BYTES_MAX, PROCESS_ARGUMENTS_ITEMS_MAX, PROCESS_ENVIRONMENT_ITEMS_MAX,
    PROCESS_ENVIRONMENT_NAME_BYTES_MAX, PROCESS_ENVIRONMENT_VALUE_BYTES_MAX,
    PROCESS_OUTPUT_CHUNK_BYTES_MAX, PROCESS_PROGRAM_BYTES_MAX, PROCESS_STDIN_BYTES_MAX,
    REPOSITORY_PATH_BYTES_MAX, REPOSITORY_PATH_COMPONENTS_MAX, REVISION_ID_BYTES_MAX,
    WORKTREE_ID_BYTES_MAX,
};
use lotta_domain::{BoundedVec, MemoryBlockInput, bounds::ResourceBound};
use std::path::{Component, Path, PathBuf};

fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}
fn check_len(actual: usize, bound: ResourceBound) -> Result<(), RuntimeError> {
    if actual > bound.value {
        return Err(RuntimeError::LimitExceeded {
            context: bound.name.into(),
        });
    }
    Ok(())
}

macro_rules! text_value {
    ($name:ident, $bound:ident, $allow_empty:literal, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Eq, Hash, PartialEq)]
        pub struct $name(String);
        impl $name {
            /// Validates and stores a UTF-8 value under its declared empty-value policy.
            /// # Errors
            /// Returns a runtime validation or limit error for empty or oversized input.
            pub fn new(value: String) -> Result<Self, RuntimeError> {
                if !$allow_empty && value.is_empty() {
                    return Err(invalid(stringify!($name)));
                }
                check_len(value.len(), $bound)?;
                Ok(Self(value))
            }
            /// Borrows the validated value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
            /// Consumes the wrapper.
            #[must_use]
            pub fn into_string(self) -> String {
                self.0
            }
        }
    };
}

text_value!(
    Program,
    PROCESS_PROGRAM_BYTES_MAX,
    false,
    "A bounded non-empty executable name or path."
);
text_value!(
    ProcessArgument,
    PROCESS_ARGUMENT_BYTES_MAX,
    true,
    "One bounded process argument; empty argv entries are preserved."
);
text_value!(
    EnvironmentName,
    PROCESS_ENVIRONMENT_NAME_BYTES_MAX,
    false,
    "A bounded non-empty process environment name."
);
text_value!(
    EnvironmentValue,
    PROCESS_ENVIRONMENT_VALUE_BYTES_MAX,
    true,
    "A bounded process environment value; empty values are preserved."
);
text_value!(
    CommitMessage,
    COMMIT_MESSAGE_BYTES_MAX,
    false,
    "A bounded non-empty commit message."
);
text_value!(
    RevisionId,
    REVISION_ID_BYTES_MAX,
    false,
    "An opaque bounded non-empty repository revision."
);
text_value!(
    WorktreeId,
    WORKTREE_ID_BYTES_MAX,
    false,
    "An opaque bounded non-empty worktree identifier."
);
text_value!(
    MemoryLabel,
    REPOSITORY_PATH_BYTES_MAX,
    false,
    "A bounded non-empty initial memory label."
);
text_value!(
    MemoryDescription,
    MEMORY_FILE_BYTES_MAX,
    true,
    "A bounded optional initial memory description; empty schema values are preserved."
);

macro_rules! debug_text {
    ($name:ident) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }
    };
}
debug_text!(Program);
debug_text!(ProcessArgument);
debug_text!(EnvironmentName);
debug_text!(CommitMessage);
debug_text!(RevisionId);
debug_text!(WorktreeId);
debug_text!(MemoryLabel);
debug_text!(MemoryDescription);
impl std::fmt::Debug for EnvironmentValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EnvironmentValue([REDACTED])")
    }
}

macro_rules! bytes_value {
    ($name:ident, $bound:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $name(Vec<u8>);
        impl $name {
            /// Validates bytes before storing them.
            /// # Errors
            /// Returns a runtime limit error when input exceeds the purpose-specific ceiling.
            pub fn new(value: Vec<u8>) -> Result<Self, RuntimeError> {
                check_len(value.len(), $bound)?;
                Ok(Self(value))
            }
            /// Borrows validated bytes.
            #[must_use]
            pub fn as_slice(&self) -> &[u8] {
                &self.0
            }
            /// Consumes the wrapper.
            #[must_use]
            pub fn into_vec(self) -> Vec<u8> {
                self.0
            }
        }
    };
}
bytes_value!(
    ProcessStdin,
    PROCESS_STDIN_BYTES_MAX,
    "Bounded process standard input."
);
bytes_value!(
    ProcessOutputChunk,
    PROCESS_OUTPUT_CHUNK_BYTES_MAX,
    "One bounded process output chunk."
);
bytes_value!(
    MemoryFileContent,
    MEMORY_FILE_BYTES_MAX,
    "One `MemFS` file bounded to exactly 8 MiB."
);
bytes_value!(
    DiffChunk,
    MEMFS_DIFF_CHUNK_BYTES_MAX,
    "One bounded `MemFS` diff chunk."
);

/// Validated total process-output ceiling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessOutputBytesMax(usize);
impl ProcessOutputBytesMax {
    /// Validates the requested total output ceiling.
    /// # Errors
    /// Returns a limit error above `PROCESS_OUTPUT_TOTAL_BYTES_MAX`.
    pub fn new(value: usize) -> Result<Self, RuntimeError> {
        check_len(value, crate::bounds::PROCESS_OUTPUT_TOTAL_BYTES_MAX)?;
        Ok(Self(value))
    }
    /// Returns the validated byte ceiling.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// Bounded process arguments.
pub type ProcessArguments = BoundedVec<ProcessArgument, { PROCESS_ARGUMENTS_ITEMS_MAX.value }>;
/// One validated environment entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentEntry {
    name: EnvironmentName,
    value: EnvironmentValue,
}
impl EnvironmentEntry {
    /// Validates an environment entry.
    /// # Errors
    /// Rejects names containing `=` or NUL.
    pub fn new(name: EnvironmentName, value: EnvironmentValue) -> Result<Self, RuntimeError> {
        if name.as_str().contains(['=', '\0']) {
            return Err(invalid("process environment name"));
        }
        Ok(Self { name, value })
    }
    /// Borrows the name.
    #[must_use]
    pub fn name(&self) -> &EnvironmentName {
        &self.name
    }
    /// Borrows the value.
    #[must_use]
    pub fn value(&self) -> &EnvironmentValue {
        &self.value
    }
}
/// Bounded process environment.
pub type ProcessEnvironment = BoundedVec<EnvironmentEntry, { PROCESS_ENVIRONMENT_ITEMS_MAX.value }>;

/// Purpose-specific validated initial memory block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitialMemoryBlock {
    label: MemoryLabel,
    value: MemoryFileContent,
    description: Option<MemoryDescription>,
}
impl InitialMemoryBlock {
    /// Converts a domain input while validating every retained value before storage.
    /// # Errors
    /// Returns validation or purpose-specific limit errors.
    pub fn from_domain(input: MemoryBlockInput) -> Result<Self, RuntimeError> {
        let label = MemoryLabel::new(input.label.into_string())?;
        let value = MemoryFileContent::new(input.value.into_bytes())?;
        let description = input
            .description
            .flatten()
            .map(MemoryDescription::new)
            .transpose()?;
        Ok(Self {
            label,
            value,
            description,
        })
    }
    /// Borrows the label.
    #[must_use]
    pub fn label(&self) -> &MemoryLabel {
        &self.label
    }
    /// Borrows Markdown bytes.
    #[must_use]
    pub fn value(&self) -> &MemoryFileContent {
        &self.value
    }
    /// Borrows the optional description.
    #[must_use]
    pub fn description(&self) -> Option<&MemoryDescription> {
        self.description.as_ref()
    }
}
/// Up to exactly 100,000 initial memory files.
pub type InitialMemoryBlocks = BoundedVec<InitialMemoryBlock, { MEMORY_FILES_MAX.value }>;

fn path_text(path: &Path, bound: ResourceBound) -> Result<&str, RuntimeError> {
    let value = path.to_str().ok_or_else(|| invalid("path must be UTF-8"))?;
    check_len(value.len(), bound)?;
    Ok(value)
}
fn validate_components(
    path: &Path,
    bound: ResourceBound,
    absolute: bool,
) -> Result<(), RuntimeError> {
    let mut count = 0;
    for component in path.components() {
        match component {
            Component::Normal(_) => count += 1,
            Component::RootDir if absolute => (),
            _ => return Err(invalid("path component")),
        }
    }
    check_len(count, bound)
}

/// UTF-8 repository-relative path with no root, prefix, current, or parent components.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RepositoryPath(PathBuf);
impl RepositoryPath {
    /// Validates a repository-relative path lexically without filesystem I/O.
    /// # Errors
    /// Rejects empty, non-UTF-8, absolute, traversal, overlong, or over-component input.
    pub fn new(value: PathBuf) -> Result<Self, RuntimeError> {
        if value.as_os_str().is_empty() || value.is_absolute() {
            return Err(invalid("repository path"));
        }
        path_text(&value, REPOSITORY_PATH_BYTES_MAX)?;
        validate_components(&value, REPOSITORY_PATH_COMPONENTS_MAX, false)?;
        Ok(Self(value))
    }
    /// Borrows the validated path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
    /// Consumes the wrapper.
    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

/// Absolute UTF-8 working directory retained with its absolute lexical confinement root.
///
/// This proves lexical confinement only. Before every effect, adapters must immediately
/// canonicalize
/// root and value, reject missing paths where required, re-check symlinks, and reject any escape.
/// Root equality is accepted because a process may run at the sandbox root.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConfinedPath {
    root: PathBuf,
    value: PathBuf,
}
impl ConfinedPath {
    /// Validates absolute lexical confinement without filesystem I/O.
    /// # Errors
    /// Rejects non-UTF-8, relative, traversal, escaped, overlong, or over-component paths.
    pub fn new(root: PathBuf, value: PathBuf) -> Result<Self, RuntimeError> {
        if !root.is_absolute() || !value.is_absolute() {
            return Err(invalid("confined absolute path"));
        }
        path_text(&root, CONFINED_PATH_BYTES_MAX)?;
        path_text(&value, CONFINED_PATH_BYTES_MAX)?;
        validate_components(&root, CONFINED_PATH_COMPONENTS_MAX, true)?;
        validate_components(&value, CONFINED_PATH_COMPONENTS_MAX, true)?;
        if !value.starts_with(&root) {
            return Err(invalid("confined path escape"));
        }
        Ok(Self { root, value })
    }
    /// Borrows the retained lexical root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Borrows the working directory value.
    #[must_use]
    pub fn value(&self) -> &Path {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structural_boundary_values_have_exact_storage_types() {
        let Program(inner): Program = Program::new("x".into()).unwrap();
        let _: String = inner;
        let ProcessArgument(inner): ProcessArgument = ProcessArgument::new(String::new()).unwrap();
        let _: String = inner;
        let EnvironmentName(inner): EnvironmentName = EnvironmentName::new("N".into()).unwrap();
        let _: String = inner;
        let EnvironmentValue(inner): EnvironmentValue =
            EnvironmentValue::new(String::new()).unwrap();
        let _: String = inner;
        let CommitMessage(inner): CommitMessage = CommitMessage::new("x".into()).unwrap();
        let _: String = inner;
        let RevisionId(inner): RevisionId = RevisionId::new("x".into()).unwrap();
        let _: String = inner;
        let WorktreeId(inner): WorktreeId = WorktreeId::new("x".into()).unwrap();
        let _: String = inner;
        let MemoryLabel(inner): MemoryLabel = MemoryLabel::new("x".into()).unwrap();
        let _: String = inner;
        let MemoryDescription(inner): MemoryDescription =
            MemoryDescription::new(String::new()).unwrap();
        let _: String = inner;
        let ProcessStdin(inner): ProcessStdin = ProcessStdin::new(Vec::new()).unwrap();
        let _: Vec<u8> = inner;
        let ProcessOutputChunk(inner): ProcessOutputChunk =
            ProcessOutputChunk::new(Vec::new()).unwrap();
        let _: Vec<u8> = inner;
        let MemoryFileContent(inner): MemoryFileContent =
            MemoryFileContent::new(Vec::new()).unwrap();
        let _: Vec<u8> = inner;
        let DiffChunk(inner): DiffChunk = DiffChunk::new(Vec::new()).unwrap();
        let _: Vec<u8> = inner;
        let ProcessOutputBytesMax(inner): ProcessOutputBytesMax =
            ProcessOutputBytesMax::new(0).unwrap();
        let _: usize = inner;
        let RepositoryPath(inner): RepositoryPath = RepositoryPath::new("x".into()).unwrap();
        let _: PathBuf = inner;
        let ConfinedPath { root, value }: ConfinedPath =
            ConfinedPath::new("/x".into(), "/x".into()).unwrap();
        let _: PathBuf = root;
        let _: PathBuf = value;
    }

    #[test]
    fn empty_semantics_and_environment_debug_redaction() {
        assert!(Program::new(String::new()).is_err());
        assert!(EnvironmentName::new(String::new()).is_err());
        assert!(CommitMessage::new(String::new()).is_err());
        assert!(RevisionId::new(String::new()).is_err());
        assert!(WorktreeId::new(String::new()).is_err());
        assert!(MemoryLabel::new(String::new()).is_err());
        assert!(ProcessArgument::new(String::new()).is_ok());
        let secret = EnvironmentValue::new("secret-value".into()).unwrap();
        assert_eq!(secret.as_str(), "secret-value");
        assert!(!format!("{secret:?}").contains("secret-value"));
        assert!(EnvironmentValue::new(String::new()).is_ok());
        assert!(MemoryDescription::new(String::new()).is_ok());
    }

    #[test]
    fn resource_observation_and_constructors() {
        for bound in [
            MEMORY_FILES_MAX,
            MEMORY_FILE_BYTES_MAX,
            PROCESS_OUTPUT_CHUNK_BYTES_MAX,
            REPOSITORY_PATH_BYTES_MAX,
        ] {
            assert!(bound.observe(bound.value - 1).is_none());
            assert!(!bound.observe(bound.value).unwrap().exceeded);
            assert!(bound.observe(bound.value + 1).unwrap().exceeded);
        }
        assert!(MemoryFileContent::new(vec![0; MEMORY_FILE_BYTES_MAX.value]).is_ok());
        assert!(MemoryFileContent::new(vec![0; MEMORY_FILE_BYTES_MAX.value + 1]).is_err());
    }
    #[test]
    fn repository_path_cases() {
        assert!(RepositoryPath::new("a".repeat(REPOSITORY_PATH_BYTES_MAX.value).into()).is_ok());
        assert!(
            RepositoryPath::new("a".repeat(REPOSITORY_PATH_BYTES_MAX.value + 1).into()).is_err()
        );
        assert!(RepositoryPath::new(PathBuf::new()).is_err());
        for path in [".", "..", "a/../b", "/a"] {
            assert!(RepositoryPath::new(path.into()).is_err());
        }
        let at = (0..REPOSITORY_PATH_COMPONENTS_MAX.value)
            .map(|_| "a")
            .collect::<Vec<_>>()
            .join("/");
        assert!(RepositoryPath::new(at.into()).is_ok());
        let over = (0..=REPOSITORY_PATH_COMPONENTS_MAX.value)
            .map(|_| "a")
            .collect::<Vec<_>>()
            .join("/");
        assert!(RepositoryPath::new(over.into()).is_err());
    }
    #[test]
    fn confined_path_byte_and_component_boundaries() {
        let root = PathBuf::from("/");
        let at_bytes = format!("/{}", "a".repeat(CONFINED_PATH_BYTES_MAX.value - 1));
        assert!(ConfinedPath::new(root.clone(), at_bytes.into()).is_ok());
        let over_bytes = format!("/{}", "a".repeat(CONFINED_PATH_BYTES_MAX.value));
        assert!(ConfinedPath::new(root.clone(), over_bytes.into()).is_err());
        let at = format!(
            "/{}",
            (0..CONFINED_PATH_COMPONENTS_MAX.value)
                .map(|_| "a")
                .collect::<Vec<_>>()
                .join("/")
        );
        assert!(ConfinedPath::new(root.clone(), at.into()).is_ok());
        let over = format!(
            "/{}",
            (0..=CONFINED_PATH_COMPONENTS_MAX.value)
                .map(|_| "a")
                .collect::<Vec<_>>()
                .join("/")
        );
        assert!(ConfinedPath::new(root, over.into()).is_err());
    }

    #[test]
    fn confined_path_cases_and_retained_root() {
        let same = ConfinedPath::new("/sandbox".into(), "/sandbox".into()).unwrap();
        assert_eq!(same.root(), Path::new("/sandbox"));
        assert_eq!(same.value(), Path::new("/sandbox"));
        assert!(ConfinedPath::new("/sandbox".into(), "/sandbox/work".into()).is_ok());
        assert!(ConfinedPath::new("/sandbox".into(), "/other".into()).is_err());
        assert!(ConfinedPath::new("/sandbox".into(), "/sandbox/../other".into()).is_err());
        let create_target = RepositoryPath::new("missing/child.md".into()).unwrap();
        assert_eq!(create_target.as_path().parent(), Some(Path::new("missing")));
    }
    #[cfg(unix)]
    #[test]
    fn paths_reject_non_utf8() {
        use std::os::unix::ffi::OsStringExt;
        assert!(
            RepositoryPath::new(PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]))).is_err()
        );
    }
}
