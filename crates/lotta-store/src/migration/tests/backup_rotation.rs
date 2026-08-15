use super::support::*;
use super::*;

#[test]
#[cfg(unix)]
fn three_to_three_oldest_and_symlink_safe() {
    use std::os::unix::fs::symlink;
    let root = copy_fixture("persistence/versioned_legacy_transcript");
    let directory = conversation(root.path());
    let source = bytes(&directory.join("messages.jsonl"));
    let mut regular = Vec::new();
    for sequence in 1..=3 {
        let path = directory.join(format!(
            "messages.jsonl.lotta-upgrade-20000101-000000-{sequence}.bak"
        ));
        std::fs::write(&path, format!("{sequence}")).expect("backup");
        regular.push(path);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let sentinel = directory.join("sentinel");
    std::fs::write(&sentinel, b"safe").expect("sentinel");
    symlink(
        &sentinel,
        directory.join("messages.jsonl.lotta-upgrade-link.bak"),
    )
    .expect("symlink");
    let report = migrate_transcripts(root.path(), false).expect("real migration");
    let newest = report.items[0].backup_path.as_ref().expect("new backup");
    assert!(!regular[0].exists());
    assert!(regular[1].exists());
    assert!(regular[2].exists());
    assert_eq!(bytes(newest), source);
    let matching = std::fs::read_dir(&directory)
        .expect("directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("messages.jsonl.lotta-upgrade-")
                && entry.file_name().to_string_lossy().ends_with(".bak")
        })
        .count();
    assert_eq!(matching, 3);
    assert_eq!(bytes(&sentinel), b"safe");
    super::converts::assert_no_temp(&directory);
}
