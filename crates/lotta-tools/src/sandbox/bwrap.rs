use std::path::Path;

/// Renders exact Bubblewrap arguments with workspace restoration after isolation masking.
#[must_use]
pub fn bubblewrap_arguments(
    workspace_root: &Path,
    isolation_root: &Path,
    program: &str,
    arguments: &[String],
) -> Vec<String> {
    let workspace = workspace_root.to_string_lossy().into_owned();
    let isolation = isolation_root.to_string_lossy().into_owned();
    let mut output = Vec::with_capacity(arguments.len() + 18);
    output.extend([
        "--ro-bind".into(),
        "/".into(),
        "/".into(),
        "--dev".into(),
        "/dev".into(),
        "--proc".into(),
        "/proc".into(),
        "--tmpfs".into(),
        isolation,
        "--bind".into(),
        workspace.clone(),
        workspace,
        "--die-with-parent".into(),
        "--".into(),
        program.into(),
    ]);
    output.extend(arguments.iter().cloned());
    output
}
