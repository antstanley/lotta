use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct CopyCase(PathBuf);
impl CopyCase {
    fn new(label: &str) -> Self {
        let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "lotta-persistence-{label}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary root");
        copy_bounded(&FixtureLoader::new(), &path);
        Self(path)
    }
    fn loader(&self) -> FixtureLoader {
        FixtureLoader::from_root(&self.0).expect("custom loader")
    }
}
impl Drop for CopyCase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_bounded(loader: &FixtureLoader, destination: &Path) {
    let paths = loader
        .list_tree("persistence")
        .expect("bounded source tree");
    assert_eq!(paths.len(), INVENTORY_ENTRIES + 1);
    let mut bytes = 0_usize;
    for relative in paths {
        let value = loader
            .load_bytes(format!("persistence/{relative}"))
            .expect("bounded source file");
        bytes += value.len();
        assert!(bytes <= 50_000);
        let target = destination.join("persistence").join(relative);
        fs::create_dir_all(target.parent().expect("parent")).expect("copy parent");
        fs::write(target, value).expect("copy fixture");
    }
}

fn assert_rejected(case: &CopyCase) {
    assert!(matches!(
        load_index(&case.loader()),
        Err(TestkitError::MalformedFixture { .. }
            | TestkitError::MissingFixture { .. }
            | TestkitError::ConfinedPath { .. })
    ));
}

fn rewrite_index(case: &CopyCase, mutate: impl FnOnce(&mut Value)) {
    let path = case.0.join("persistence/index.json");
    let mut value: Value =
        serde_json::from_slice(&fs::read(&path).expect("index bytes")).expect("index JSON");
    mutate(&mut value);
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("index encoding"),
    )
    .expect("write index");
}

#[test]
fn index_is_complete_and_tuples_are_fixed() {
    let value = index();
    assert_eq!(value.inventory.len(), INVENTORY_ENTRIES);
    assert_eq!(value.cases.len(), PERSISTENCE_CASES);
    for (position, case) in value.cases.iter().enumerate() {
        assert_eq!(case, &expected_case(position));
        assert_eq!(
            case.relative_root,
            PersistenceCase::all()[position].directory()
        );
    }
}

#[test]
fn inventory_stale_hash_is_rejected() {
    let case = CopyCase::new("stale-hash");
    rewrite_index(&case, |index| {
        index["inventory"][0]["sha256"] = "0".repeat(64).into();
    });
    assert_rejected(&case);
}

#[test]
fn inventory_non_lowercase_hex_is_rejected() {
    let case = CopyCase::new("hash-alphabet");
    rewrite_index(&case, |index| {
        index["inventory"][0]["sha256"] = "G".repeat(64).into();
    });
    assert_rejected(&case);
}

#[test]
fn inventory_same_length_mutation_is_rejected() {
    let case = CopyCase::new("same-length");
    let path = case
        .0
        .join("persistence/current_typescript_state/expectation.json");
    let mut bytes = fs::read(&path).expect("fixture bytes");
    let position = bytes
        .iter()
        .position(|byte| *byte == b'r')
        .expect("mutable byte");
    bytes[position] = b'R';
    fs::write(path, bytes).expect("mutated fixture");
    assert_rejected(&case);
}

#[test]
fn inventory_extra_and_missing_files_are_rejected() {
    let extra = CopyCase::new("extra-file");
    fs::write(
        extra.0.join("persistence/current_typescript_state/extra"),
        b"x",
    )
    .expect("extra file");
    assert_rejected(&extra);
    let missing = CopyCase::new("missing-file");
    fs::remove_file(
        missing
            .0
            .join("persistence/current_typescript_state/expectation.json"),
    )
    .expect("remove fixture");
    assert_rejected(&missing);
}

#[test]
fn top_level_empty_extra_and_key_directory_drift_are_rejected() {
    let extra = CopyCase::new("empty-top");
    fs::create_dir(extra.0.join("persistence/empty-extra")).expect("empty top dir");
    assert_rejected(&extra);
    let drift = CopyCase::new("key-drift");
    let root = drift
        .0
        .join("persistence/current_typescript_state/conversations");
    fs::rename(root.join(DEFAULT_KEY), root.join("wrong-key")).expect("rename key dir");
    assert_rejected(&drift);
}
