use lotta_store::migration::{MigrationDisposition, MigrationReport};
use lotta_store::verify::{VerificationClass, VerificationReport};
use std::path::{Path, PathBuf};

pub(crate) enum Command {
    Server(Vec<String>),
    Migrate { storage_dir: PathBuf, dry_run: bool },
    Verify { storage_dir: PathBuf },
}

pub(crate) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Command, CliError> {
    let values = arguments.into_iter().collect::<Vec<_>>();
    if values.first().is_none_or(|value| value != "local-backend") {
        return Ok(Command::Server(values));
    }
    match values.get(1).map(String::as_str) {
        Some("migrate-transcripts") => parse_migrate(&values[2..]),
        Some("verify") => parse_verify(&values[2..]),
        _ => Err(CliError::Config("unknown local-backend command")),
    }
}

fn parse_migrate(values: &[String]) -> Result<Command, CliError> {
    let mut storage_dir = None;
    let mut dry_run = false;
    let mut index = 0;
    while index < values.len() {
        match values[index].as_str() {
            "--storage-dir" => index = set_storage(values, index, &mut storage_dir)?,
            "--dry-run" if !dry_run => {
                dry_run = true;
                index += 1;
            }
            "--dry-run" => return Err(CliError::Config("--dry-run may appear only once")),
            _ => return Err(CliError::Config("unknown migration argument")),
        }
    }
    Ok(Command::Migrate {
        storage_dir: required_storage(storage_dir)?,
        dry_run,
    })
}

fn parse_verify(values: &[String]) -> Result<Command, CliError> {
    let mut storage_dir = None;
    let mut index = 0;
    while index < values.len() {
        match values[index].as_str() {
            "--storage-dir" => index = set_storage(values, index, &mut storage_dir)?,
            _ => return Err(CliError::Config("unknown verification argument")),
        }
    }
    Ok(Command::Verify {
        storage_dir: required_storage(storage_dir)?,
    })
}

fn set_storage(
    values: &[String],
    index: usize,
    target: &mut Option<PathBuf>,
) -> Result<usize, CliError> {
    if target.is_some() {
        return Err(CliError::Config("--storage-dir may appear only once"));
    }
    let value = values
        .get(index + 1)
        .filter(|value| !value.starts_with('-'))
        .ok_or(CliError::Config("--storage-dir requires a value"))?;
    let path = PathBuf::from(value);
    lotta_store::StorePaths::new(&path).map_err(CliError::Store)?;
    *target = Some(path);
    Ok(index + 2)
}

fn required_storage(value: Option<PathBuf>) -> Result<PathBuf, CliError> {
    value.ok_or(CliError::Config("--storage-dir is required"))
}

pub(crate) fn migrate(path: PathBuf, dry_run: bool) -> Result<(), CliError> {
    let report = lotta_store::migration::migrate_transcripts(path, dry_run)?;
    print_migration(&report);
    Ok(())
}

pub(crate) fn verify(path: PathBuf) -> Result<(), CliError> {
    let report = lotta_store::verify::verify_transcripts(path)?;
    print_verification(&report);
    Ok(())
}

fn print_migration(report: &MigrationReport) {
    println!(
        "migration dry_run={} conversations={}",
        report.dry_run,
        report.items.len()
    );
    for item in &report.items {
        let status = match item.disposition {
            MigrationDisposition::Converted => "converted",
            MigrationDisposition::AlreadyCurrent => "already_current",
            MigrationDisposition::Empty => "empty",
        };
        println!(
            "migration path={} status={} messages={} backup={}",
            safe_relative(&report.storage_root, &item.conversation_dir),
            status,
            item.message_count,
            item.backup_path
                .as_ref()
                .map_or("none".to_owned(), |path| safe_relative(
                    &report.storage_root,
                    path
                ))
        );
    }
}

fn print_verification(report: &VerificationReport) {
    println!(
        "verification conversations={} rows={} findings={}",
        report.conversations_checked,
        report.rows_checked,
        report.findings.len()
    );
    for finding in &report.findings {
        println!(
            "finding path={} row={} class={}",
            safe_relative(&report.storage_root, &finding.path),
            finding.row,
            finding.class.code()
        );
    }
    for class in all_classes() {
        let count = report
            .findings
            .iter()
            .filter(|finding| finding.class == class)
            .count();
        println!("class {} count={count}", class.code());
    }
}

fn safe_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

const fn all_classes() -> [VerificationClass; 6] {
    [
        VerificationClass::DuplicateEntryId,
        VerificationClass::InvalidParentLink,
        VerificationClass::InvalidSessionHeader,
        VerificationClass::OrphanToolResult,
        VerificationClass::InvalidCompactionReference,
        VerificationClass::UnsupportedOuterField,
    ]
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    #[error("invalid command configuration: {0}")]
    Config(&'static str),
    #[error(transparent)]
    Store(#[from] lotta_store::StoreError),
    #[error(transparent)]
    Server(#[from] lotta_app_server::error::AppServerError),
    #[error(transparent)]
    Setup(#[from] lotta_runtime::turn::SetupError),
}

impl CliError {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::Config(_) => "config",
            Self::Store(error) => error.kind().code(),
            Self::Server(error) => error.code(),
            Self::Setup(_) => "production_setup",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_argument_negatives() {
        for values in [
            vec!["local-backend", "verify"],
            vec!["local-backend", "verify", "--storage-dir"],
            vec!["local-backend", "verify", "--storage-dir", "relative"],
            vec!["local-backend", "verify", "--unknown"],
            vec![
                "local-backend",
                "verify",
                "--storage-dir",
                "/tmp/a",
                "--storage-dir",
                "/tmp/b",
            ],
            vec![
                "local-backend",
                "migrate-transcripts",
                "--dry-run",
                "--dry-run",
            ],
        ] {
            assert!(parse(values.into_iter().map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn server_arguments_remain_delegated() {
        let Command::Server(values) = parse(
            ["server", "--backend", "local", "--listen"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("dispatch") else {
            panic!("server dispatch expected");
        };
        assert!(lotta_app_server::config::parse_cli(values).is_ok());
    }
}
