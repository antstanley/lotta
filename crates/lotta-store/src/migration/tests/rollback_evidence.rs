use super::support::*;
use super::*;
use crate::AtomicObserver;
use std::fs::File;
use std::fs::FileTimes;
use std::sync::atomic::{AtomicUsize, Ordering};

enum Target {
    Conversation,
    Manifest,
}

struct ReplacementObserver {
    target: Target,
    replacement: Vec<u8>,
    original_modified: std::time::SystemTime,
    fired: AtomicUsize,
}

impl AtomicObserver for ReplacementObserver {
    fn before_rename(&self, path: &Path) -> Result<(), StoreError> {
        let matches = match self.target {
            Target::Conversation => path.ends_with("conversation.json"),
            Target::Manifest => path.ends_with("manifest.json"),
        };
        if matches && self.fired.fetch_add(1, Ordering::SeqCst) == 0 {
            std::fs::write(path, &self.replacement).expect("external replacement");
            File::options()
                .write(true)
                .open(path)
                .expect("replacement file")
                .set_times(FileTimes::new().set_modified(self.original_modified))
                .expect("restore mtime");
        }
        Ok(())
    }
}
impl transcript::TranscriptRewriteObserver for ReplacementObserver {}
impl transcript::upgrade::BackupObserver for ReplacementObserver {}
impl execute::MigrationObserver for ReplacementObserver {
    fn atomic(&self) -> &dyn AtomicObserver {
        self
    }
    fn transcript(&self) -> &dyn transcript::TranscriptRewriteObserver {
        self
    }
    fn backup(&self) -> &dyn transcript::upgrade::BackupObserver {
        self
    }
}

#[test]
fn external_replacements_survive() {
    external_conversation_survives();
    external_manifest_survives();
}

fn equal_length_replacement(original: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    assert_eq!(needle.len(), replacement.len());
    let offset = original
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("replaceable value");
    let mut changed = original.to_vec();
    changed[offset..offset + needle.len()].copy_from_slice(replacement);
    assert_eq!(changed.len(), original.len());
    assert_ne!(changed, original);
    changed
}

fn external_conversation_survives() {
    let root = copy_fixture("persistence/versioned_legacy_transcript");
    let directory = conversation(root.path());
    let message_path = directory.join("messages.jsonl");
    let conversation_path = directory.join("conversation.json");
    let manifest_path = directory.join("manifest.json");
    let source = bytes(&message_path);
    let original_manifest = bytes(&manifest_path);
    let original_conversation = bytes(&conversation_path);
    let replacement = equal_length_replacement(
        &original_conversation,
        b"SANITIZED_FIXTURE_SUMMARY",
        b"EXTERNAL_FIXTURE_SUMMARY_",
    );
    let modified = std::fs::metadata(&conversation_path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    let observer = ReplacementObserver {
        target: Target::Conversation,
        replacement: replacement.clone(),
        original_modified: modified,
        fired: AtomicUsize::new(0),
    };
    let error = migrate_transcripts_observed(root.path(), false, &observer)
        .expect_err("conversation conflict");
    assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
    assert_eq!(bytes(&conversation_path), replacement);
    assert_eq!(
        std::fs::metadata(&conversation_path)
            .expect("metadata")
            .modified()
            .expect("mtime"),
        modified
    );
    assert_eq!(bytes(&manifest_path), original_manifest);
    let current = bytes(&message_path);
    assert_ne!(current, source);
    assert_eq!(rows(&message_path)[0]["type"], "session");
    assert_retained_backup(&directory, &source);
    super::converts::assert_no_temp(&directory);
}

fn external_manifest_survives() {
    let root = copy_fixture("persistence/versioned_legacy_transcript");
    let directory = conversation(root.path());
    let message_path = directory.join("messages.jsonl");
    let conversation_path = directory.join("conversation.json");
    let manifest_path = directory.join("manifest.json");
    let source = bytes(&message_path);
    let original_conversation = bytes(&conversation_path);
    let original_manifest = bytes(&manifest_path);
    let replacement = equal_length_replacement(
        &original_manifest,
        b"2000-01-01T00:00:00.000Z",
        b"1999-12-31T23:59:59.999Z",
    );
    let modified = std::fs::metadata(&manifest_path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    let observer = ReplacementObserver {
        target: Target::Manifest,
        replacement: replacement.clone(),
        original_modified: modified,
        fired: AtomicUsize::new(0),
    };
    let error =
        migrate_transcripts_observed(root.path(), false, &observer).expect_err("manifest conflict");
    assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
    assert_eq!(bytes(&manifest_path), replacement);
    assert_eq!(
        std::fs::metadata(&manifest_path)
            .expect("metadata")
            .modified()
            .expect("mtime"),
        modified
    );
    assert_ne!(bytes(&message_path), source);
    assert_eq!(rows(&message_path)[0]["type"], "session");
    assert_ne!(bytes(&conversation_path), original_conversation);
    assert_retained_backup(&directory, &source);
    super::converts::assert_no_temp(&directory);
}

fn assert_retained_backup(directory: &Path, source: &[u8]) {
    let backups = std::fs::read_dir(directory)
        .expect("directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
        .map(|entry| bytes(&entry.path()))
        .collect::<Vec<_>>();
    assert!(backups.iter().any(|backup| backup == source));
}
