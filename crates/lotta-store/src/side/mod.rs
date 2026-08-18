//! Opaque, bounded access to baseline-compatible side stores.

/// Channel side-store files and opaque plugin inventory.
pub mod channels;
/// Cron and run-log side stores.
pub mod crons;
mod io;
/// Pure side-store path authority.
pub mod paths;
/// Scoped project-local settings.
pub mod project;
/// Global settings side store.
pub mod settings;

pub use io::{OpaqueFile, SideRevision, read_opaque, write_opaque_expected};
pub use paths::{ChannelFile, ProjectFile, SidePaths};

#[cfg(test)]
#[path = "../side_test_support.rs"]
mod evidence;

#[cfg(test)]
#[test]
fn conflict() {
    evidence::conflict_body();
}

#[cfg(test)]
#[test]
fn negatives() {
    evidence::negatives_body();
}

#[cfg(test)]
#[test]
fn read_mutation_conflict() {
    evidence::read_mutation_conflict_body();
}

#[cfg(test)]
#[test]
fn limits() {
    evidence::limits_body();
}

#[cfg(all(test, unix))]
#[test]
fn unix_confinement() {
    evidence::unix_confinement_body();
}
