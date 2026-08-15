use crate::*;
use lotta_domain::{AgentId, ConversationId};
use lotta_testkit::fixtures::{FixtureLoader, persistence};
use lotta_testkit::roots::TemporaryRoot;
use std::path::Path;
use std::sync::Mutex;

fn root(label: &str) -> (TemporaryRoot, StorePaths) {
    let root = TemporaryRoot::new(label).expect("temporary root");
    let paths = StorePaths::new(root.path().join("backend")).expect("store paths");
    (root, paths)
}

mod paths {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn key_forms() {
        let index = persistence::load_index(&FixtureLoader::new()).expect("persistence corpus");
        let agent = AgentId::accept(index.ids.agent).expect("agent fixture");
        let named = ConversationId::accept(index.ids.conversation).expect("conversation fixture");
        let fixtures = [
            (
                ConversationKey::Default(agent.clone()),
                index.keys.default_conversation,
            ),
            (ConversationKey::Named(named), index.keys.named_conversation),
        ];
        let loader = FixtureLoader::new();
        let mut encountered = Vec::new();
        for file in loader.list_tree("persistence").expect("fixture tree") {
            let parts = file.split('/').collect::<Vec<_>>();
            for (position, part) in parts.iter().enumerate() {
                if *part == "conversations" && position + 1 < parts.len() {
                    let segment = parts[position + 1].to_owned();
                    assert!(segment.len() <= crate::paths::STORE_KEY_BYTES_MAX);
                    if !encountered.contains(&segment) {
                        encountered.try_reserve(1).expect("bounded fixture keys");
                        encountered.push(segment);
                    }
                }
            }
        }
        encountered.sort();
        let mut expected = Vec::new();
        for (key, fixture) in fixtures {
            let encoded = key.encoded().expect("encode");
            assert_eq!(encoded, fixture.encoded);
            assert!(encountered.contains(&encoded));
            assert_eq!(ConversationKey::decode(&encoded).expect("decode"), key);
            assert_eq!(
                String::from_utf8(base64_decode(&encoded)).expect("utf8"),
                fixture.source
            );
            expected.push(encoded);
        }
        expected.sort();
        assert_eq!(encountered, expected);
        let encode =
            |value: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.as_bytes());
        let over_limit = "a".repeat(crate::paths::STORE_KEY_BYTES_MAX + 1);
        let malformed = [
            String::new(),
            "====".into(),
            "ZGVmYXVsdDo=".into(),
            "YWJj+".into(),
            "YWJj/".into(),
            "ZGVmYXVsdDo".into(),
            "AB".into(),
            encode("unknown:value"),
            encode("default:"),
            encode("conversation:"),
            over_limit,
        ];
        for value in malformed {
            assert!(ConversationKey::decode(&value).is_err(), "accepted {value}");
        }
        let (_owned, store) = root("paths-layout");
        assert_eq!(store.agents(), store.root().join("agents"));
        assert_eq!(store.conversations(), store.root().join("conversations"));
        assert_eq!(store.memfs(), store.root().join("memfs"));
        assert_eq!(store.providers(), store.root().join("providers"));
        assert_eq!(store.indexes(), store.root().join("indexes"));
    }

    fn base64_decode(value: &str) -> Vec<u8> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .expect("base64")
    }

    #[test]
    fn root_resolution_is_explicit_and_bounded() {
        let resolved = StorePaths::resolve_from(None, Some("/home/test".into())).expect("home");
        assert_eq!(
            resolved.root(),
            Path::new("/home/test/.letta/lc-local-backend")
        );
        let override_root =
            StorePaths::resolve_from(Some("/custom".into()), None).expect("override");
        assert_eq!(override_root.root(), Path::new("/custom"));
        assert!(StorePaths::resolve_from(None, None).is_err());
        assert!(StorePaths::resolve_from(Some("relative".into()), None).is_err());
        for length in [
            crate::paths::STORE_ROOT_PATH_BYTES_MAX - 1,
            crate::paths::STORE_ROOT_PATH_BYTES_MAX,
        ] {
            let path = format!("/{}", "r".repeat(length - 1));
            assert_eq!(path.len(), length);
            StorePaths::new(path).expect("lexical bound accepted");
        }
        let above = format!("/{}", "r".repeat(crate::paths::STORE_ROOT_PATH_BYTES_MAX));
        assert_eq!(
            StorePaths::new(above).expect_err("above bound").kind(),
            StoreErrorKind::InvalidPath
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;
            let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
            assert_eq!(
                StorePaths::new(path).expect_err("non-UTF8").kind(),
                StoreErrorKind::InvalidPath
            );
        }
    }

    #[test]
    fn id_derived_paths_are_bounded_and_confined() {
        let (_owned, store) = root("paths-id-bounds");
        let raw_at = AgentId::accept("a".repeat(crate::paths::STORE_KEY_BYTES_MAX)).expect("id");
        assert!(
            store
                .memory(&raw_at)
                .expect("raw at")
                .starts_with(store.memfs())
        );
        let raw_above =
            AgentId::accept("a".repeat(crate::paths::STORE_KEY_BYTES_MAX + 1)).expect("id");
        assert!(store.memory(&raw_above).is_err());
        for malicious in [".", "..", "a/b", "a\\b", "/absolute", "a\0b"] {
            let id = AgentId::accept(malicious).expect("domain accepts id");
            assert!(store.memory(&id).is_err());
        }
        let encoded_below = AgentId::accept("a".repeat(768)).expect("id");
        let record = store.agent_record(&encoded_below).expect("encoded below");
        assert!(record.starts_with(store.agents()));
        let encoded_above = AgentId::accept("a".repeat(769)).expect("id");
        assert!(store.agent_record(&encoded_above).is_err());
        let conversation = ConversationId::accept("c".repeat(760)).expect("id");
        assert!(
            store
                .conversation_dir(&encoded_below, &conversation)
                .is_err()
        );
        assert_ne!(record, store.agents());
        assert_ne!(record, store.root());
    }
}

mod atomic {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    enum Mutation {
        Replace(Vec<u8>),
        PreserveMtime(Vec<u8>),
    }

    #[derive(Default)]
    struct Observer {
        attempts: AtomicUsize,
        flushes: AtomicUsize,
        mutate: Mutex<Option<Mutation>>,
        fail_attempts: bool,
        fail_parent_sync: bool,
        mutate_before_remove: bool,
        removals: AtomicUsize,
        renames: AtomicUsize,
    }

    impl AtomicObserver for Observer {
        fn attempt(&self, _attempt: usize, target: &Path) -> Result<(), StoreError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.fail_attempts {
                Err(StoreError::new(StoreErrorKind::Io, target))
            } else {
                Ok(())
            }
        }

        fn before_rename(&self, target: &Path) -> Result<(), StoreError> {
            if let Some(mutation) = self.mutate.lock().expect("mutation lock").take() {
                let (bytes, modified) = match mutation {
                    Mutation::Replace(bytes) => (bytes, None),
                    Mutation::PreserveMtime(bytes) => (
                        bytes,
                        Some(
                            std::fs::metadata(target)
                                .expect("metadata")
                                .modified()
                                .expect("mtime"),
                        ),
                    ),
                };
                std::fs::write(target, bytes).expect("external write bypassing Lotta lock");
                if let Some(modified) = modified {
                    let file = std::fs::OpenOptions::new()
                        .write(true)
                        .open(target)
                        .expect("restore mtime file");
                    file.set_times(std::fs::FileTimes::new().set_modified(modified))
                        .expect("restore mtime");
                }
            }
            Ok(())
        }

        fn before_parent_sync(&self, parent: &Path) -> Result<(), StoreError> {
            self.renames.fetch_add(1, Ordering::SeqCst);
            if self.fail_parent_sync {
                Err(StoreError::new(StoreErrorKind::Io, parent))
            } else {
                Ok(())
            }
        }

        fn parent_flushed(&self, _parent: &Path) {
            self.flushes.fetch_add(1, Ordering::SeqCst);
        }

        fn before_remove(&self, target: &Path) -> Result<(), StoreError> {
            self.removals.fetch_add(1, Ordering::SeqCst);
            if self.mutate_before_remove {
                std::fs::write(target, b"external-delete-race").expect("external mutation");
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct MtimeFailureObserver {
        samples: AtomicUsize,
        attempts: AtomicUsize,
    }

    impl AtomicObserver for MtimeFailureObserver {
        fn source_modified(
            &self,
            target: &Path,
            _metadata: &std::fs::Metadata,
        ) -> Result<std::time::SystemTime, StoreError> {
            self.samples.fetch_add(1, Ordering::SeqCst);
            Err(StoreError::new(StoreErrorKind::Io, target))
        }

        fn attempt(&self, _attempt: usize, _target: &Path) -> Result<(), StoreError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_storage_children_are_rejected() {
        use std::os::unix::fs::symlink;
        let (_owned, paths) = root("atomic-symlinks");
        let outside = TemporaryRoot::new("atomic-symlinks-outside").expect("outside");
        let sentinel = outside.path().join("sentinel");
        std::fs::write(&sentinel, b"outside-safe").expect("sentinel");
        std::fs::create_dir_all(paths.root()).expect("backend");
        for (child, target) in [
            ("providers", paths.provider_auth()),
            ("agents", paths.agents().join("record.json")),
            (
                "conversations",
                paths.conversations().join("key/record.json"),
            ),
        ] {
            let link = paths.root().join(child);
            symlink(outside.path(), &link).expect("symlink child");
            let error = atomic_write(&target, b"attack", WriteMode::Standard).expect_err("reject");
            assert_eq!(error.kind(), StoreErrorKind::InvalidPath);
            std::fs::remove_file(link).expect("remove link");
        }
        std::fs::create_dir(paths.conversations()).expect("conversations");
        let encoded = paths.conversations().join("encoded");
        symlink(outside.path(), &encoded).expect("encoded link");
        let error = atomic_write(
            &encoded.join("conversation.json"),
            b"attack",
            WriteMode::Standard,
        )
        .expect_err("reject encoded link");
        assert_eq!(error.kind(), StoreErrorKind::InvalidPath);
        assert_eq!(std::fs::read(sentinel).expect("sentinel"), b"outside-safe");
        let names = std::fs::read_dir(outside.path())
            .expect("outside")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        assert!(names.contains(&"sentinel".into()));
        assert!(!names.contains(&"auth.json".into()));
        assert!(!names.contains(&"record.json".into()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_record_entries_are_rejected() {
        use lotta_runtime::RuntimeError;
        use lotta_runtime::ports::{AgentStore, ConversationStore};
        use std::os::unix::fs::symlink;
        use tokio_util::sync::CancellationToken;
        let (_owned, paths) = root("atomic-record-symlinks");
        let outside = TemporaryRoot::new("atomic-record-symlinks-outside").expect("outside");
        let sentinel = outside.path().join("sentinel");
        std::fs::write(&sentinel, b"outside-safe").expect("sentinel");
        std::fs::create_dir_all(paths.agents()).expect("agents");
        let agent = AgentId::accept("agent-symlink-record").expect("agent");
        let agent_record = paths.agent_record(&agent).expect("agent record");
        symlink(&sentinel, &agent_record).expect("agent record link");
        let store = LocalStore::new(paths.clone());
        assert!(matches!(
            AgentStore::load(&store, &agent).await,
            Err(RuntimeError::InvalidData { .. })
        ));
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        assert!(matches!(
            store.list(sender, CancellationToken::new()).await,
            Err(RuntimeError::InvalidData { .. })
        ));
        std::fs::remove_file(agent_record).expect("remove agent record link");
        let conversation =
            ConversationId::accept("conversation-symlink-record").expect("conversation");
        let directory = paths
            .conversation_dir(&agent, &conversation)
            .expect("conversation directory");
        std::fs::create_dir_all(&directory).expect("conversation directory");
        symlink(&sentinel, directory.join("conversation.json")).expect("conversation record link");
        assert!(matches!(
            ConversationStore::load(&store, &agent, &conversation).await,
            Err(RuntimeError::InvalidData { .. })
        ));
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        assert!(matches!(
            store
                .list_for_agent(&agent, sender, CancellationToken::new())
                .await,
            Err(RuntimeError::InvalidData { .. })
        ));
        assert_eq!(std::fs::read(sentinel).expect("sentinel"), b"outside-safe");
    }

    #[test]
    fn writes_via_temp_and_rename() {
        let (_owned, paths) = root("atomic-write");
        let path = paths.agents().join("record.json");
        atomic_write(&path, b"first", WriteMode::Standard).expect("create");
        atomic_write(&path, b"second", WriteMode::Standard).expect("replace");
        assert_eq!(std::fs::read(&path).expect("record"), b"second");
        let names = std::fs::read_dir(paths.agents())
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["record.json"]);
    }

    #[test]
    fn fresh_nested_root_write() {
        let owned = TemporaryRoot::new("atomic-fresh-nested").expect("root");
        let paths = StorePaths::new(owned.path().join("fresh/a/backend")).expect("paths");
        let path = paths.agents().join("record.json");
        atomic_write(&path, b"fresh", WriteMode::Standard).expect("nested root write");
        assert_eq!(std::fs::read(path).expect("record"), b"fresh");
    }

    #[test]
    fn flushes_parent_directory() {
        let (_owned, paths) = root("atomic-parent-flush");
        let path = paths.agents().join("record.json");
        let observer = Observer::default();
        atomic_write_observed(&path, b"durable", WriteMode::Standard, &observer).expect("write");
        assert_eq!(observer.flushes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn postcommit_parent_sync_failure_is_not_retried() {
        let (_owned, paths) = root("atomic-postcommit-sync");
        let path = paths.agents().join("record.json");
        atomic_write(&path, b"old", WriteMode::Standard).expect("source");
        let observer = Observer {
            fail_parent_sync: true,
            ..Observer::default()
        };
        let error = atomic_write_observed(&path, b"new", WriteMode::Standard, &observer)
            .expect_err("postcommit durability failure");
        assert_eq!(error.kind(), StoreErrorKind::Io);
        assert_eq!(observer.attempts.load(Ordering::SeqCst), 1);
        assert_eq!(observer.renames.load(Ordering::SeqCst), 1);
        assert_eq!(std::fs::read(&path).expect("committed bytes"), b"new");
        assert_eq!(
            std::fs::read_dir(paths.agents()).expect("agents").count(),
            1
        );
    }

    #[test]
    fn external_change_yields_storage_conflict() {
        let (_owned, paths) = root("atomic-conflict");
        let path = paths.agents().join("record.json");
        atomic_write(&path, b"source", WriteMode::Standard).expect("source");
        let observer = Observer {
            mutate: Mutex::new(Some(Mutation::Replace(b"external-change".to_vec()))),
            ..Observer::default()
        };
        let error = atomic_write_observed(&path, b"lotta", WriteMode::Standard, &observer)
            .expect_err("must detect external mutation between compare and rename");
        assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
        assert_eq!(
            std::fs::read(&path).expect("external survives"),
            b"external-change"
        );
        assert_eq!(observer.attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn absent_target_creation_yields_storage_conflict() {
        let (_owned, paths) = root("atomic-create-conflict");
        let path = paths.agents().join("record.json");
        let observer = Observer {
            mutate: Mutex::new(Some(Mutation::Replace(b"external-create".to_vec()))),
            ..Observer::default()
        };
        let error = atomic_write_observed(&path, b"lotta", WriteMode::Standard, &observer)
            .expect_err("must detect absent target creation");
        assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
        assert_eq!(observer.attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read(&path).expect("external survives"),
            b"external-create"
        );
        assert_eq!(
            std::fs::read_dir(paths.agents()).expect("agents").count(),
            1
        );
    }

    #[test]
    fn same_size_same_mtime_change_yields_storage_conflict() {
        let (_owned, paths) = root("atomic-checksum-conflict");
        let path = paths.agents().join("record.json");
        atomic_write(&path, b"source", WriteMode::Standard).expect("source");
        let observer = Observer {
            mutate: Mutex::new(Some(Mutation::PreserveMtime(b"mutate".to_vec()))),
            ..Observer::default()
        };
        let error = atomic_write_observed(&path, b"lotta", WriteMode::Standard, &observer)
            .expect_err("checksum must detect preserved metadata change");
        assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
        assert_eq!(observer.attempts.load(Ordering::SeqCst), 1);
        assert_eq!(std::fs::read(&path).expect("external survives"), b"mutate");
        assert_eq!(
            std::fs::read_dir(paths.agents()).expect("agents").count(),
            1
        );
    }

    #[test]
    fn mtime_failure_is_typed_without_attempt_or_rename() {
        let (_owned, paths) = root("atomic-mtime-failure");
        let path = paths.agents().join("record.json");
        atomic_write(&path, b"source", WriteMode::Standard).expect("source");
        let observer = MtimeFailureObserver::default();
        let error = atomic_write_observed(&path, b"lotta", WriteMode::Standard, &observer)
            .expect_err("mtime failure");
        assert_eq!(error.kind(), StoreErrorKind::Io);
        assert_eq!(observer.samples.load(Ordering::SeqCst), 1);
        assert_eq!(observer.attempts.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(&path).expect("source survives"), b"source");
        assert_eq!(
            std::fs::read_dir(paths.agents()).expect("agents").count(),
            1
        );
    }

    #[test]
    fn retry_bound() {
        let (_owned, paths) = root("atomic-retries");
        let path = paths.agents().join("record.json");
        let observer = Observer {
            fail_attempts: true,
            ..Observer::default()
        };
        let error = atomic_write_observed(&path, b"value", WriteMode::Standard, &observer)
            .expect_err("persistent retryable failure");
        assert_eq!(error.kind(), StoreErrorKind::Io);
        assert_eq!(
            observer.attempts.load(Ordering::SeqCst),
            ATOMIC_WRITE_RETRIES_MAX
        );
    }

    #[test]
    fn temp_sequence_exhaustion_is_typed_without_wrap() {
        let counter = AtomicU64::new(u64::MAX);
        let error = crate::atomic::next_sequence(&counter, Path::new("/safe/record"))
            .expect_err("sequence exhausted");
        assert_eq!(error.kind(), StoreErrorKind::Limit);
        assert_eq!(counter.load(Ordering::SeqCst), u64::MAX);
    }

    #[test]
    fn atomic_delete_boundaries() {
        let (_owned, paths) = root("atomic-delete");
        let path = paths.agents().join("record.json");
        atomic_write(&path, b"source", WriteMode::Standard).expect("source");
        let mutation = Observer {
            mutate_before_remove: true,
            ..Observer::default()
        };
        let error = atomic_delete_observed(&path, &mutation).expect_err("delete conflict");
        assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
        assert_eq!(
            std::fs::read(&path).expect("external survives"),
            b"external-delete-race"
        );
        assert_eq!(mutation.removals.load(Ordering::SeqCst), 1);

        let lock = LottaStorageLock::try_acquire(paths.root()).expect("lock");
        let error = atomic_delete_observed(&path, &Observer::default()).expect_err("contention");
        assert_eq!(error.kind(), StoreErrorKind::LottaLock);
        assert!(path.exists());
        drop(lock);

        let sync = Observer {
            fail_parent_sync: true,
            ..Observer::default()
        };
        let error = atomic_delete_observed(&path, &sync).expect_err("post-remove sync");
        assert_eq!(error.kind(), StoreErrorKind::Io);
        assert!(!path.exists());
        assert_eq!(sync.removals.load(Ordering::SeqCst), 1);
        assert_eq!(sync.renames.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn modes() {
        let (_owned, paths) = root("atomic-modes");
        std::fs::create_dir_all(paths.providers()).expect("providers");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(paths.providers(), std::fs::Permissions::from_mode(0o777))
                .expect("permissive dir");
        }
        atomic_write(&paths.provider_auth(), b"{}", WriteMode::ProviderAuth).expect("auth");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(paths.providers())
                    .expect("dir mode")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(paths.provider_auth())
                    .expect("file mode")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

mod errors {
    use super::*;
    use std::io::Write;
    use std::sync::Arc;
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::prelude::*;

    const MARKER: &str = "TASK22_UNIQUE_SECRET_MARKER";

    #[test]
    fn distinct_kinds() {
        let kinds = [
            StoreErrorKind::DiskFull,
            StoreErrorKind::Permission,
            StoreErrorKind::Parse,
            StoreErrorKind::Checksum,
            StoreErrorKind::StorageConflict,
            StoreErrorKind::LottaLock,
        ];
        let mut codes = kinds.map(StoreErrorKind::code).to_vec();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), kinds.len());
    }

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    struct Writer(Arc<Mutex<Vec<u8>>>);
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("capture").extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Writer;
        fn make_writer(&'a self) -> Self::Writer {
            Writer(Arc::clone(&self.0))
        }
    }

    struct PermissionObserver;
    impl AtomicObserver for PermissionObserver {
        fn attempt(&self, _attempt: usize, target: &Path) -> Result<(), StoreError> {
            Err(StoreError::new(StoreErrorKind::Permission, target))
        }
    }

    #[test]
    fn logs_exclude_contents() {
        static SERIAL: Mutex<()> = Mutex::new(());
        let _serial = SERIAL.lock().expect("serial trace capture");
        let (_owned, paths) = root("errors-logs");
        let path = paths.providers().join("auth.json");
        let capture = Capture::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(capture.clone())
                .without_time(),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, || {
            let error = atomic_write_observed(
                &path,
                MARKER.as_bytes(),
                WriteMode::Standard,
                &PermissionObserver,
            )
            .expect_err("permission failure");
            assert_eq!(error.path(), path);
            assert_eq!(error.kind(), StoreErrorKind::Permission);
            for kind in [
                StoreErrorKind::DiskFull,
                StoreErrorKind::Permission,
                StoreErrorKind::Parse,
                StoreErrorKind::Checksum,
                StoreErrorKind::StorageConflict,
                StoreErrorKind::LottaLock,
            ] {
                let _logged = StoreError::new(kind, &path).log();
            }
            let lock = LottaStorageLock::try_acquire(paths.root()).expect("lock");
            let contention = atomic_write(&path, MARKER.as_bytes(), WriteMode::Standard)
                .expect_err("atomic lock contention");
            assert_eq!(contention.kind(), StoreErrorKind::LottaLock);
            drop(lock);
        });
        assert!(!path.exists());
        let output = String::from_utf8(capture.0.lock().expect("capture").clone()).expect("utf8");
        assert!(output.contains("auth.json"));
        for code in [
            "disk_full",
            "permission",
            "parse",
            "checksum",
            "storage_conflict",
            "lotta_lock",
        ] {
            assert!(output.contains(code), "missing {code}: {output}");
        }
        assert!(!output.contains(MARKER));
        assert_eq!(output.matches("permission").count(), 2);
        assert_eq!(output.matches("lotta_lock").count(), 2);
    }
}

mod adapter {
    use super::*;
    use std::io::Write as _;
    use std::sync::Arc;
    use tokio::sync::{Semaphore, oneshot};

    #[tokio::test(flavor = "current_thread")]
    async fn run_blocking_does_not_block_executor() {
        let pool = Arc::new(Semaphore::new(1));
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let operation = tokio::spawn(crate::adapter::run_blocking(pool, move || {
            let _sent = started_tx.send(());
            release_rx
                .recv()
                .map_err(|_| StoreError::new(StoreErrorKind::Io, "/release"))?;
            Ok(())
        }));
        tokio::time::timeout(std::time::Duration::from_secs(2), started_rx)
            .await
            .expect("started timeout")
            .expect("started");
        let heartbeat = tokio::spawn(async { 42_u8 });
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), heartbeat)
                .await
                .expect("heartbeat timeout")
                .expect("heartbeat"),
            42
        );
        release_tx.send(()).expect("release");
        tokio::time::timeout(std::time::Duration::from_secs(2), operation)
            .await
            .expect("operation timeout")
            .expect("join")
            .expect("operation");
    }

    #[test]
    fn bounded_json_and_list_boundaries() {
        let path = Path::new("/safe/record.json");
        let mut sink = crate::adapter::BoundedJson::new(path);
        let chunk = vec![b'x'; crate::adapter::RECORD_BYTES_MAX / 2];
        sink.write_all(&chunk).expect("below");
        sink.write_all(&chunk).expect("at");
        assert_eq!(sink.as_bytes().len(), crate::adapter::RECORD_BYTES_MAX);
        assert!(sink.write_all(b"x").is_err());
        assert_eq!(sink.as_bytes().len(), crate::adapter::RECORD_BYTES_MAX);

        let mut values = Vec::new();
        crate::adapter::push_bounded_max(&mut values, 1, path, 2).expect("below");
        crate::adapter::push_bounded_max(&mut values, 2, path, 2).expect("at");
        let error = crate::adapter::push_bounded_max(&mut values, 3, path, 2).expect_err("above");
        assert_eq!(error.kind(), StoreErrorKind::Limit);
        assert_eq!(values, vec![1, 2]);
    }
}

mod lock {
    use super::*;

    #[test]
    fn immediate_contention_is_typed() {
        assert_eq!(LOTTA_STORAGE_LOCK_WAIT_MS, 0);
        let (_owned, paths) = root("lock-contention");
        let first = LottaStorageLock::try_acquire(paths.root()).expect("first lock");
        let second = LottaStorageLock::try_acquire(paths.root()).expect_err("immediate contention");
        assert_eq!(second.kind(), StoreErrorKind::LottaLock);
        drop(first);
        LottaStorageLock::try_acquire(paths.root()).expect("RAII release");
    }

    #[test]
    fn advisory_limitation_external_writer_bypasses_lock() {
        let (_owned, paths) = root("lock-advisory");
        let _lock = LottaStorageLock::try_acquire(paths.root()).expect("Lotta lock");
        let path = paths.root().join("external.txt");
        std::fs::write(&path, b"external process analogue")
            .expect("external ignores advisory lock");
        assert!(path.is_file());
    }
}
