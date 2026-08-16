use std::path::Path;

/// Pinned macOS sandbox executable.
pub const SEATBELT_PROGRAM: &str = "/usr/bin/sandbox-exec";
/// Pinned workspace profile. Concrete paths are supplied only as argv definitions.
pub const SEATBELT_PROFILE: &str = concat!(
    "(version 1)\n",
    "(allow default)\n",
    "(deny file-write* (subpath \"/\"))\n",
    "(allow file-write* (subpath \"/dev\"))\n",
    "(deny file-read* file-write* (subpath (param \"ISOLATION_ROOT\")))\n",
    "(allow file-read-metadata (literal (param \"ISOLATION_ROOT\")))\n",
    "(allow file-read* file-write* (subpath (param \"WORKSPACE_ROOT\")))",
);

/// Renders exact Seatbelt arguments without interpolating filesystem paths into SBPL.
#[must_use]
pub fn seatbelt_arguments(
    workspace_root: &Path,
    isolation_root: &Path,
    program: &str,
    arguments: &[String],
) -> Vec<String> {
    let mut output = Vec::with_capacity(arguments.len() + 8);
    output.extend([
        "-p".into(),
        SEATBELT_PROFILE.into(),
        format!("-DISOLATION_ROOT={}", isolation_root.display()),
        format!("-DWORKSPACE_ROOT={}", workspace_root.display()),
        "--".into(),
        program.into(),
    ]);
    output.extend(arguments.iter().cloned());
    output
}
