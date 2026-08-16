use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const TREE_DEPTH_MAX: usize = 32;
pub const TREE_FILES_MAX: usize = 4_096;
pub const TREE_ITEMS_MAX: usize = 8_192;
pub const TREE_BYTES_MAX: u64 = 64 * 1_024 * 1_024;
pub const TREE_FILE_BYTES_MAX: u64 = 16 * 1_024 * 1_024;
pub const TREE_PATH_BYTES_MAX: usize = 4_096;
pub const COPY_BUFFER_BYTES: usize = 64 * 1_024;

#[derive(Clone, Copy)]
pub struct TreeLimits {
    pub depth: usize,
    pub files: usize,
    pub items: usize,
    pub bytes: u64,
    pub file_bytes: u64,
    pub path_bytes: usize,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            depth: TREE_DEPTH_MAX,
            files: TREE_FILES_MAX,
            items: TREE_ITEMS_MAX,
            bytes: TREE_BYTES_MAX,
            file_bytes: TREE_FILE_BYTES_MAX,
            path_bytes: TREE_PATH_BYTES_MAX,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct InventoryEntry {
    pub path: String,
    pub kind: &'static str,
    pub mode: u32,
    pub length: u64,
    pub sha256: String,
}

pub fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    copy_tree_with(source, target, TreeLimits::default())
}

fn copy_tree_with(source: &Path, target: &Path, limits: TreeLimits) -> Result<(), String> {
    check_root(source, limits)?;
    std::fs::create_dir_all(target).map_err(diagnostic("create target", target))?;
    let mut stack = Vec::new();
    reserve_push(&mut stack, (source.to_owned(), target.to_owned(), 0_usize))?;
    let mut totals = Totals::default();
    while let Some((directory, destination, depth)) = stack.pop() {
        let remaining = limits
            .items
            .checked_sub(totals.items)
            .ok_or("tree processed item count exceeds limit")?;
        let entries = entries(&directory, remaining)?;
        for entry in entries.into_iter().rev() {
            let path = entry.path();
            let target_path = destination.join(entry.file_name());
            let metadata =
                std::fs::symlink_metadata(&path).map_err(diagnostic("metadata", &path))?;
            check_entry(&path, source, &metadata, depth + 1, limits, &mut totals)?;
            if metadata.is_dir() {
                std::fs::create_dir(&target_path)
                    .map_err(diagnostic("create directory", &target_path))?;
                set_mode(&target_path, &metadata)?;
                check_push(stack.len(), limits.items, "tree stack item limit exceeded")?;
                reserve_push(&mut stack, (path, target_path, depth + 1))?;
            } else {
                copy_file(&path, &target_path, &metadata)?;
            }
        }
    }
    Ok(())
}

pub fn inventory_tree(root: &Path) -> Result<Vec<InventoryEntry>, String> {
    inventory_tree_with(root, TreeLimits::default())
}

fn inventory_tree_with(root: &Path, limits: TreeLimits) -> Result<Vec<InventoryEntry>, String> {
    check_root(root, limits)?;
    let mut output = Vec::new();
    let mut stack = Vec::new();
    reserve_push(&mut stack, (root.to_owned(), PathBuf::new(), 0_usize))?;
    let mut totals = Totals::default();
    while let Some((directory, relative, depth)) = stack.pop() {
        let remaining = limits
            .items
            .checked_sub(totals.items)
            .ok_or("tree processed item count exceeds limit")?;
        let entries = entries(&directory, remaining)?;
        for entry in entries.into_iter().rev() {
            let path = entry.path();
            let child_relative = relative.join(entry.file_name());
            let metadata =
                std::fs::symlink_metadata(&path).map_err(diagnostic("metadata", &path))?;
            check_entry(&path, root, &metadata, depth + 1, limits, &mut totals)?;
            let display = child_relative.to_string_lossy().into_owned();
            check_push(
                output.len(),
                limits.items,
                "tree output item limit exceeded",
            )?;
            output
                .try_reserve(1)
                .map_err(|_| "tree output allocation failed".to_owned())?;
            if metadata.is_dir() {
                output.push(inventory_entry(
                    display,
                    "directory",
                    &metadata,
                    String::new(),
                ));
                check_push(stack.len(), limits.items, "tree stack item limit exceeded")?;
                reserve_push(&mut stack, (path, child_relative, depth + 1))?;
            } else {
                output.push(inventory_entry(
                    display,
                    "file",
                    &metadata,
                    hash_file(&path)?,
                ));
            }
        }
    }
    output.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(output)
}

pub fn hash_file(path: &Path) -> Result<String, String> {
    hash_file_bounded(path, TREE_FILE_BYTES_MAX)
}

pub fn hash_file_bounded(path: &Path, limit: u64) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(diagnostic("metadata", path))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "hash source is not a plain regular file: {}",
            path.display()
        ));
    }
    if metadata.len() > limit {
        return Err(format!("hash file limit exceeded: {}", path.display()));
    }
    let mut file = File::open(path).map_err(diagnostic("open", path))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer).map_err(diagnostic("read", path))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(count).map_err(|_| "hash byte count overflow")?)
            .ok_or("hash byte count overflow")?;
        if total > limit {
            return Err(format!("hash file limit exceeded: {}", path.display()));
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Default)]
struct Totals {
    items: usize,
    files: usize,
    bytes: u64,
}

fn check_root(path: &Path, _limits: TreeLimits) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(diagnostic("metadata", path))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "tree root is not a plain directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn check_entry(
    path: &Path,
    root: &Path,
    metadata: &Metadata,
    depth: usize,
    limits: TreeLimits,
    totals: &mut Totals,
) -> Result<(), String> {
    totals.items = totals
        .items
        .checked_add(1)
        .ok_or("tree item count overflow")?;
    if totals.items > limits.items {
        return Err(format!("tree item limit exceeded: {}", path.display()));
    }
    if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
        return Err(format!(
            "tree symlink or special file rejected: {}",
            path.display()
        ));
    }
    if depth > limits.depth {
        return Err(format!("tree depth limit exceeded: {}", path.display()));
    }
    let relative = path.strip_prefix(root).map_err(|error| error.to_string())?;
    if path_bytes(relative) > limits.path_bytes {
        return Err(format!("tree path limit exceeded: {}", path.display()));
    }
    if metadata.is_file() {
        totals.files = totals
            .files
            .checked_add(1)
            .ok_or("tree file count overflow")?;
        totals.bytes = totals
            .bytes
            .checked_add(metadata.len())
            .ok_or("tree byte overflow")?;
        if totals.files > limits.files || metadata.len() > limits.file_bytes {
            return Err(format!("tree file limit exceeded: {}", path.display()));
        }
        if totals.bytes > limits.bytes {
            return Err(format!(
                "tree total byte limit exceeded: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn entries(path: &Path, item_limit: usize) -> Result<Vec<std::fs::DirEntry>, String> {
    let reader = std::fs::read_dir(path).map_err(diagnostic("read directory", path))?;
    let mut output = Vec::new();
    for item in reader {
        if output.len() >= item_limit {
            return Err(format!("tree item limit exceeded: {}", path.display()));
        }
        output
            .try_reserve(1)
            .map_err(|_| "tree entry allocation failed".to_owned())?;
        output.push(item.map_err(diagnostic("read directory entry", path))?);
    }
    output.sort_by_key(std::fs::DirEntry::file_name);
    Ok(output)
}

fn reserve_push<T>(output: &mut Vec<T>, value: T) -> Result<(), String> {
    output
        .try_reserve(1)
        .map_err(|_| "tree stack allocation failed".to_owned())?;
    output.push(value);
    Ok(())
}

fn check_push(length: usize, limit: usize, message: &str) -> Result<(), String> {
    let next = length
        .checked_add(1)
        .ok_or("tree collection item overflow")?;
    if next > limit {
        return Err(message.to_owned());
    }
    Ok(())
}

fn copy_file(source: &Path, target: &Path, metadata: &Metadata) -> Result<(), String> {
    let mut input = File::open(source).map_err(diagnostic("open source", source))?;
    let mut output = File::create(target).map_err(diagnostic("create target", target))?;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(diagnostic("read source", source))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(diagnostic("write target", target))?;
    }
    output
        .sync_all()
        .map_err(diagnostic("sync target", target))?;
    set_mode(target, metadata)
}

fn inventory_entry(
    path: String,
    kind: &'static str,
    metadata: &Metadata,
    sha256: String,
) -> InventoryEntry {
    InventoryEntry {
        path,
        kind,
        mode: mode(metadata),
        length: metadata.len(),
        sha256,
    }
}

fn diagnostic<'a>(action: &'a str, path: &'a Path) -> impl Fn(std::io::Error) -> String + 'a {
    move |error| format!("{action} {}: {error}", path.display())
}

fn path_bytes(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

#[cfg(unix)]
fn mode(metadata: &Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    metadata.mode() & 0o7777
}

#[cfg(not(unix))]
fn mode(_: &Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn set_mode(path: &Path, metadata: &Metadata) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let permissions = std::fs::Permissions::from_mode(mode(metadata));
    std::fs::set_permissions(path, permissions).map_err(diagnostic("set mode", path))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: &Metadata) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> super::super::support::TestRoot {
        super::super::support::TestRoot::new(&format!("tree-{name}"))
    }

    fn limits() -> TreeLimits {
        TreeLimits {
            depth: 2,
            files: 2,
            items: 2,
            bytes: 4,
            file_bytes: 3,
            path_bytes: 5,
        }
    }

    #[test]
    fn boundaries_and_copy_use_shared_path() {
        let root = root("bounds");
        let source = root.path().join("s");
        let target = root.path().join("t");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("a"), b"123").unwrap();
        std::fs::write(source.join("b"), b"4").unwrap();
        copy_tree_with(&source, &target, limits()).unwrap();
        assert_eq!(
            inventory_tree_with(&source, limits()).unwrap(),
            inventory_tree_with(&target, limits()).unwrap()
        );
        std::fs::write(source.join("c"), b"").unwrap();
        assert!(inventory_tree_with(&source, limits()).is_err());
    }

    #[test]
    fn empty_directories_obey_item_boundaries() {
        for count in [1_usize, 2, 3] {
            let root = root(&format!("items-{count}"));
            let source = root.path().join("s");
            std::fs::create_dir(&source).unwrap();
            for index in 0..count {
                std::fs::create_dir(source.join(format!("d{index}"))).unwrap();
            }
            assert_eq!(inventory_tree_with(&source, limits()).is_ok(), count <= 2);
            if count <= 2 {
                let target = root.path().join("t");
                assert!(copy_tree_with(&source, &target, limits()).is_ok());
            }
        }
    }

    #[test]
    fn nested_fanout_cannot_exceed_aggregate_item_cap() {
        let fanout_root = root("fanout");
        let source = fanout_root.path().join("s");
        std::fs::create_dir_all(source.join("a")).unwrap();
        std::fs::create_dir(source.join("b")).unwrap();
        std::fs::create_dir(source.join("a/c")).unwrap();
        assert!(inventory_tree_with(&source, limits()).is_err());

        let exact = root("fanout-exact");
        let source = exact.path().join("s");
        std::fs::create_dir_all(source.join("a/c")).unwrap();
        assert_eq!(inventory_tree_with(&source, limits()).unwrap().len(), 2);
    }

    #[test]
    fn rejects_depth_path_file_total_and_symlink() {
        for kind in ["depth", "path", "file", "total"] {
            let root = root(kind);
            let source = root.path().join("s");
            std::fs::create_dir(&source).unwrap();
            match kind {
                "depth" => std::fs::create_dir_all(source.join("a/b/c")).unwrap(),
                "path" => std::fs::write(source.join("abcdef"), b"").unwrap(),
                "file" => std::fs::write(source.join("a"), b"1234").unwrap(),
                _ => {
                    std::fs::write(source.join("a"), b"123").unwrap();
                    std::fs::write(source.join("b"), b"12").unwrap();
                }
            }
            assert!(inventory_tree_with(&source, limits()).is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let root = root("link");
            let source = root.path().join("s");
            std::fs::create_dir(&source).unwrap();
            symlink("missing", source.join("a")).unwrap();
            assert!(inventory_tree_with(&source, limits()).is_err());
        }
    }
}
