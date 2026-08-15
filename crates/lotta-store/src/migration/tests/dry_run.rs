use super::support::*;
use super::*;

#[test]
fn public_api_preserves_full_tree() {
    let root = copy_fixture("persistence/unversioned_legacy_transcript");
    let before = snapshot(root.path());
    let report = migrate_transcripts(root.path(), true).expect("dry run");
    assert!(report.dry_run);
    assert_eq!(report.items[0].disposition, MigrationDisposition::Converted);
    assert_eq!(snapshot(root.path()), before);
}
