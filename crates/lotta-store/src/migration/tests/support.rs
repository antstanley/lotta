use super::*;
use lotta_testkit::fixtures::FixtureLoader;
use lotta_testkit::roots::TemporaryRoot;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) const KEY: &str = "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl";

pub(super) fn copy_fixture(name: &str) -> TemporaryRoot {
    let label = name.rsplit('/').next().unwrap_or("migration");
    let root = TemporaryRoot::new(&format!("task26-{label}")).expect("temporary root");
    let loader = FixtureLoader::default();
    for relative in loader.list_tree(name).expect("fixture tree") {
        let source = format!("{name}/{relative}");
        let destination = root.path().join(&relative);
        std::fs::create_dir_all(destination.parent().expect("parent")).expect("directory");
        std::fs::write(
            destination,
            loader.load_bytes(source).expect("fixture bytes"),
        )
        .expect("fixture copy");
    }
    root
}

pub(super) fn conversation(root: &Path) -> PathBuf {
    root.join("conversations").join(KEY)
}

pub(super) fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("fixture bytes")
}

pub(super) fn rows(path: &Path) -> Vec<Value> {
    String::from_utf8(bytes(path))
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("row"))
        .collect()
}

type TreeState = ([u8; 32], u64, std::time::SystemTime, u64);

pub(super) fn snapshot(root: &Path) -> BTreeMap<PathBuf, TreeState> {
    let mut result = BTreeMap::new();
    collect(root, root, &mut result);
    result
}

fn collect(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, TreeState>) {
    let mut entries = std::fs::read_dir(directory)
        .expect("directory")
        .map(|entry| entry.expect("entry"))
        .collect::<Vec<_>>();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let metadata = entry.metadata().expect("metadata");
        let relative = entry
            .path()
            .strip_prefix(root)
            .expect("relative")
            .to_owned();
        if metadata.is_dir() {
            collect(root, &entry.path(), result);
        } else {
            let contents = bytes(&entry.path());
            result.insert(
                relative,
                (
                    <[u8; 32]>::from(Sha256::digest(&contents)),
                    metadata.len(),
                    metadata.modified().expect("modified"),
                    inode(&metadata),
                ),
            );
        }
    }
}

#[cfg(unix)]
fn inode(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    metadata.ino()
}

#[cfg(not(unix))]
const fn inode(_metadata: &std::fs::Metadata) -> u64 {
    0
}
