//! Bounded skill discovery, parsing, selection, complete loading, and script policy.

mod discovery;
mod frontmatter;
#[path = "load.rs"]
mod loader;
#[path = "script_policy.rs"]
mod script_runner;
mod tool_port;

pub use discovery::{SkillDiscovery, SkillRoots};
pub use frontmatter::{ParsedSkillDocument, parse_skill_document};
pub use loader::{LoadedSkill, SkillCompanion, SkillLoader};
pub use script_runner::{SkillScriptError, SkillScriptRunner};
pub use tool_port::SkillToolPort;

use std::path::PathBuf;

/// Maximum and aggregate bounds for all reachable skill filesystem work.
pub mod limits {
    /// Maximum bytes in one `SKILL.md`.
    pub const SKILL_DOCUMENT_BYTES_MAX: usize = 2 * 1024 * 1024;
    /// Maximum bytes in parsed frontmatter.
    pub const SKILL_FRONTMATTER_BYTES_MAX: usize = 64 * 1024;
    /// Maximum bytes in one frontmatter line.
    pub const SKILL_FRONTMATTER_LINE_BYTES_MAX: usize = 8 * 1024;
    /// Maximum identifier bytes.
    pub const SKILL_ID_BYTES_MAX: usize = 512;
    /// Maximum human-readable name bytes.
    pub const SKILL_NAME_BYTES_MAX: usize = 512;
    /// Maximum filesystem display path bytes.
    pub const SKILL_PATH_BYTES_MAX: usize = 4 * 1024;
    /// Maximum recursive discovery depth.
    pub const SKILL_DISCOVERY_DEPTH_MAX: usize = 32;
    /// Maximum visited discovery directories.
    pub const SKILL_DIRECTORIES_ITEMS_MAX: usize = 4_096;
    /// Maximum filesystem entries inspected during discovery.
    pub const SKILL_DISCOVERY_ENTRIES_ITEMS_MAX: usize = 32_768;
    /// Maximum discovered `SKILL.md` files.
    pub const SKILLS_DISCOVERED_ITEMS_MAX: usize = 4_096;
    /// Maximum selected skill IDs.
    pub const SKILLS_SELECTED_ITEMS_MAX: usize = 1_024;
    /// Maximum complete-load depth.
    pub const SKILL_LOAD_DEPTH_MAX: usize = 32;
    /// Maximum directories visited while loading one skill.
    pub const SKILL_LOAD_DIRECTORIES_ITEMS_MAX: usize = 4_096;
    /// Maximum filesystem entries inspected while loading one skill.
    pub const SKILL_LOAD_ENTRIES_ITEMS_MAX: usize = 32_768;
    /// Maximum companion files loaded for one skill.
    pub const SKILL_COMPANION_FILES_ITEMS_MAX: usize = 1_024;
    /// Maximum bytes in one companion file.
    pub const SKILL_COMPANION_FILE_BYTES_MAX: usize = 8 * 1024 * 1024;
    /// Maximum aggregate companion bytes.
    pub const SKILL_COMPANIONS_AGGREGATE_BYTES_MAX: usize = 32 * 1024 * 1024;
}

/// Canonical skill source with fixed precedence independent of caller ordering.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillSource {
    /// Project-local source.
    Project,
    /// Agent memory source.
    Agent,
    /// User-global source.
    Global,
    /// Shipped built-in source.
    Bundled,
}

impl SkillSource {
    /// High-to-low canonical precedence.
    pub const PRECEDENCE: [Self; 4] = [Self::Project, Self::Agent, Self::Global, Self::Bundled];

    const fn bit(self) -> u8 {
        match self {
            Self::Project => 1,
            Self::Agent => 2,
            Self::Global => 4,
            Self::Bundled => 8,
        }
    }
}

/// Normalized duplicate-free source subset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkillSources(u8);

impl SkillSources {
    /// Includes all sources.
    pub const ALL: Self = Self(15);
    /// Includes no sources.
    pub const NONE: Self = Self(0);
    /// Normalizes any source order and duplicate set.
    #[must_use]
    pub fn new(sources: &[SkillSource]) -> Self {
        Self(sources.iter().fold(0, |bits, source| bits | source.bit()))
    }
    /// Reports whether a source is included.
    #[must_use]
    pub const fn contains(self, source: SkillSource) -> bool {
        self.0 & source.bit() != 0
    }
}

impl From<&[SkillSource]> for SkillSources {
    fn from(value: &[SkillSource]) -> Self {
        Self::new(value)
    }
}
impl<const N: usize> From<&[SkillSource; N]> for SkillSources {
    fn from(value: &[SkillSource; N]) -> Self {
        Self::new(value)
    }
}

/// Discovered metadata and retained selected source location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Skill {
    /// Stable ID.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Model-facing description.
    pub description: String,
    /// Winning source.
    pub source: SkillSource,
    /// Selected source root.
    pub root: PathBuf,
    /// Exact selected `SKILL.md` location.
    pub skill_file: PathBuf,
}

/// Neutral selected prompt-entry value avoiding adapter layering cycles.
///
/// Prompt adapters must use [`SelectedSkill::id`] as the prompt skill name; `name` is display
/// metadata and is not the stable selector identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedSkill {
    /// Exact selected ID.
    pub id: String,
    /// Exact selected name.
    pub name: String,
    /// Exact selected description.
    pub description: String,
    /// Display-only `SKILL.md` location.
    pub location: String,
}

/// Fixed skill failures that retain no concrete path, content, or OS error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillError {
    /// Input syntax or text is malformed.
    Malformed,
    /// A named resource bound was exceeded.
    LimitExceeded,
    /// A symlink or canonical target escaped authority.
    Escape,
    /// A special file was encountered.
    SpecialFile,
    /// One source contains duplicate IDs.
    DuplicateSkill,
    /// Runtime selection repeats an ID.
    DuplicateSelection,
    /// Runtime selection names an absent ID.
    MissingSelection,
    /// The selected entry disappeared.
    MissingEntry,
    /// The selected entry changed identity.
    ChangedEntry,
    /// Filesystem policy infrastructure failed.
    Infrastructure,
}

#[cfg(test)]
#[path = "tests/frontmatter_fallbacks.rs"]
mod frontmatter_fallbacks;
#[cfg(test)]
#[path = "tests/load.rs"]
mod load;
#[cfg(test)]
#[path = "tests/precedence.rs"]
mod precedence;
#[cfg(test)]
#[path = "tests/script_policy.rs"]
mod script_policy;
#[cfg(test)]
#[path = "tests/source_restriction.rs"]
mod source_restriction;
