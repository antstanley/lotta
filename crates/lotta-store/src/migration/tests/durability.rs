use super::support::*;
use super::*;
use crate::AtomicObserver;
use sha2::{Digest as _, Sha256};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct OrderObserver(Mutex<Vec<(&'static str, PathBuf)>>);

impl AtomicObserver for OrderObserver {}
impl transcript::TranscriptRewriteObserver for OrderObserver {
    fn before_rename(&self, path: &Path) -> Result<(), StoreError> {
        self.0
            .lock()
            .expect("events")
            .push(("rename", path.to_owned()));
        Ok(())
    }
}
impl transcript::upgrade::BackupObserver for OrderObserver {
    fn file_flushed(&self, path: &Path) {
        self.0
            .lock()
            .expect("events")
            .push(("file", path.to_owned()));
    }
    fn parent_flushed(&self, path: &Path) {
        self.0
            .lock()
            .expect("events")
            .push(("parent", path.to_owned()));
    }
}
impl execute::MigrationObserver for OrderObserver {
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
fn backup_durable_before_replace() {
    let root = copy_fixture("persistence/versioned_legacy_transcript");
    let directory = conversation(root.path());
    let source = bytes(&directory.join("messages.jsonl"));
    let observer = OrderObserver(Mutex::new(Vec::new()));
    let report = migrate_transcripts_observed(root.path(), false, &observer).expect("migration");
    let backup = report.items[0].backup_path.as_ref().expect("backup");
    assert_eq!(bytes(backup), source);
    assert_eq!(Sha256::digest(bytes(backup)), Sha256::digest(source));
    let events = observer.0.lock().expect("events");
    let file = events
        .iter()
        .position(|event| event.0 == "file")
        .expect("file flush");
    let parent = events
        .iter()
        .position(|event| event.0 == "parent")
        .expect("parent flush");
    let rename = events
        .iter()
        .position(|event| event.0 == "rename")
        .expect("rename");
    assert!(file < parent && parent < rename);
}

#[derive(Clone, Copy)]
enum FailurePoint {
    TranscriptPre,
    TranscriptPost,
    ConversationPre,
    ConversationPost,
    ManifestPre,
    ManifestPost,
}

struct FailureObserver {
    point: FailurePoint,
    atomic_target: Mutex<Option<PathBuf>>,
    fired: AtomicUsize,
    backup_flushes: AtomicUsize,
}

impl FailureObserver {
    fn new(point: FailurePoint) -> Self {
        Self {
            point,
            atomic_target: Mutex::new(None),
            fired: AtomicUsize::new(0),
            backup_flushes: AtomicUsize::new(0),
        }
    }

    fn fail_once(&self, path: &Path) -> Result<(), StoreError> {
        if self.fired.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(StoreError::new(StoreErrorKind::Permission, path))
        } else {
            Ok(())
        }
    }
}

impl AtomicObserver for FailureObserver {
    fn before_rename(&self, target: &Path) -> Result<(), StoreError> {
        *self.atomic_target.lock().expect("target") = Some(target.to_owned());
        let matching = match self.point {
            FailurePoint::ConversationPre => target.ends_with("conversation.json"),
            FailurePoint::ManifestPre => target.ends_with("manifest.json"),
            _ => false,
        };
        if matching {
            self.fail_once(target)
        } else {
            Ok(())
        }
    }

    fn before_parent_sync(&self, parent: &Path) -> Result<(), StoreError> {
        let target = self.atomic_target.lock().expect("target").take();
        let matching = match self.point {
            FailurePoint::ConversationPost => target
                .as_deref()
                .is_some_and(|p| p.ends_with("conversation.json")),
            FailurePoint::ManifestPost => target
                .as_deref()
                .is_some_and(|p| p.ends_with("manifest.json")),
            _ => false,
        };
        if matching {
            self.fail_once(parent)
        } else {
            Ok(())
        }
    }
}

impl transcript::TranscriptRewriteObserver for FailureObserver {
    fn before_rename(&self, target: &Path) -> Result<(), StoreError> {
        if matches!(self.point, FailurePoint::TranscriptPre) {
            self.fail_once(target)
        } else {
            Ok(())
        }
    }

    fn before_parent_sync(&self, parent: &Path, _temp: &Path) -> Result<(), StoreError> {
        if matches!(self.point, FailurePoint::TranscriptPost) {
            self.fail_once(parent)
        } else {
            Ok(())
        }
    }
}

impl transcript::upgrade::BackupObserver for FailureObserver {
    fn file_flushed(&self, _path: &Path) {
        self.backup_flushes.fetch_add(1, Ordering::SeqCst);
    }
}

impl execute::MigrationObserver for FailureObserver {
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
fn failure_leaves_original() {
    for absent_manifest in [false, true] {
        for point in [
            FailurePoint::TranscriptPre,
            FailurePoint::TranscriptPost,
            FailurePoint::ConversationPre,
            FailurePoint::ConversationPost,
            FailurePoint::ManifestPre,
            FailurePoint::ManifestPost,
        ] {
            let fixture = if absent_manifest {
                "persistence/unversioned_legacy_transcript"
            } else {
                "persistence/versioned_legacy_transcript"
            };
            let root = copy_fixture(fixture);
            let directory = conversation(root.path());
            let message_path = directory.join("messages.jsonl");
            let conversation_path = directory.join("conversation.json");
            let manifest_path = directory.join("manifest.json");
            let originals = (
                bytes(&message_path),
                bytes(&conversation_path),
                manifest_path.exists().then(|| bytes(&manifest_path)),
            );
            let observer = FailureObserver::new(point);
            let error = migrate_transcripts_observed(root.path(), false, &observer)
                .expect_err("injected failure");
            assert!(matches!(
                error.kind(),
                StoreErrorKind::Permission | StoreErrorKind::Io
            ));
            if matches!(point, FailurePoint::ManifestPost) {
                let store = crate::LocalStore::new(StorePaths::new(root.path()).expect("paths"));
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("runtime");
                runtime
                    .block_on(store.load_transcript(
                        &lotta_domain::AgentId::accept("agent-local-fixture").expect("agent"),
                        &lotta_domain::ConversationId::default_for_agent(),
                    ))
                    .expect("postcommit Task25 current reload");
                let manifest =
                    transcript::manifest::read_current(&manifest_path).expect("current manifest");
                assert_eq!(
                    bytes(&directory.join(manifest.backup_path.expect("backup"))),
                    originals.0
                );
            } else {
                assert_eq!(bytes(&message_path), originals.0);
                assert_eq!(bytes(&conversation_path), originals.1);
                assert_eq!(
                    manifest_path.exists().then(|| bytes(&manifest_path)),
                    originals.2
                );
                if absent_manifest {
                    let store =
                        crate::LocalStore::new(StorePaths::new(root.path()).expect("paths"));
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .build()
                        .expect("runtime");
                    assert_eq!(
                        runtime
                            .block_on(
                                store.load_transcript(
                                    &lotta_domain::AgentId::accept("agent-local-fixture")
                                        .expect("agent"),
                                    &lotta_domain::ConversationId::default_for_agent(),
                                )
                            )
                            .expect_err("unversioned baseline")
                            .kind(),
                        StoreErrorKind::TranscriptMigrationRequired
                    );
                }
            }
            if observer.backup_flushes.load(Ordering::SeqCst) > 0 {
                let retained = std::fs::read_dir(&directory)
                    .expect("directory")
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
                    .map(|entry| bytes(&entry.path()))
                    .collect::<Vec<_>>();
                assert!(retained.iter().any(|backup| backup == &originals.0));
            }
            super::converts::assert_no_temp(&directory);
        }
    }
}

#[derive(Default)]
struct CountingObserver(AtomicUsize);
impl AtomicObserver for CountingObserver {
    fn before_rename(&self, _target: &Path) -> Result<(), StoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn before_parent_sync(&self, _parent: &Path) -> Result<(), StoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
impl transcript::TranscriptRewriteObserver for CountingObserver {
    fn before_rename(&self, _target: &Path) -> Result<(), StoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn before_parent_sync(&self, _parent: &Path, _temp: &Path) -> Result<(), StoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
impl transcript::upgrade::BackupObserver for CountingObserver {
    fn file_flushed(&self, _path: &Path) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn parent_flushed(&self, _path: &Path) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
impl execute::MigrationObserver for CountingObserver {
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
fn rerun_is_noop() {
    let root = copy_fixture("persistence/versioned_legacy_transcript");
    migrate_transcripts(root.path(), false).expect("migration");
    let before = snapshot(root.path());
    let observer = CountingObserver::default();
    let report = migrate_transcripts_observed(root.path(), false, &observer).expect("rerun");
    assert_eq!(
        report.items[0].disposition,
        MigrationDisposition::AlreadyCurrent
    );
    assert_eq!(observer.0.load(Ordering::SeqCst), 0);
    assert_eq!(snapshot(root.path()), before);
}
