use crate::schema::{FIXTURE_BYTES_MAX, Fixture, FixtureIndex, IndexCase};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

pub fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/openai")
}

pub fn load_index() -> FixtureIndex {
    let path = fixtures_root().join("index.json");
    let bytes = fs::read(path).expect("fixture index");
    assert!(bytes.len() <= FIXTURE_BYTES_MAX, "index byte bound");
    let original: Value = serde_json::from_slice(&bytes).expect("index original JSON");
    scan_value(&original, "index").expect("sanitize original index JSON");
    serde_json::from_slice(&bytes).expect("strict fixture index JSON")
}

pub fn load_fixture(entry: &IndexCase) -> Fixture {
    let root = fixtures_root().canonicalize().expect("fixture root");
    let bytes = read_verified(&root, &entry.path, &entry.sha256)
        .unwrap_or_else(|error| panic!("{}: {error}", entry.name));
    let original: Value = serde_json::from_slice(&bytes).expect("case original JSON");
    scan_value(&original, &entry.name).expect("sanitize original case JSON");
    serde_json::from_slice(&bytes).expect("strict fixture case JSON")
}

fn read_verified(root: &Path, relative: &str, expected_hash: &str) -> Result<Vec<u8>, String> {
    validate_relative(relative).map_err(str::to_owned)?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err("fixture must be a regular non-symlink file".to_owned());
    }
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    if !canonical.starts_with(root) {
        return Err("fixture escaped root".to_owned());
    }
    let bytes = fs::read(canonical).map_err(|error| error.to_string())?;
    if bytes.len() > FIXTURE_BYTES_MAX {
        return Err("fixture byte bound".to_owned());
    }
    let hash = format!("{:x}", Sha256::digest(&bytes));
    if hash != expected_hash {
        return Err("stale fixture hash".to_owned());
    }
    Ok(bytes)
}

pub fn assert_corpus_layout(index: &FixtureIndex) {
    let indexed = index
        .cases
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    corpus_layout(&fixtures_root(), &indexed).expect("confined exhaustive fixture corpus");
    assert_eq!(indexed.len(), index.cases.len(), "duplicate indexed path");
}

fn corpus_layout(root: &Path, indexed: &BTreeSet<String>) -> Result<(), String> {
    for entry in fs::read_dir(root.join("cases")).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err("unindexed non-file or symlink".to_owned());
        }
        let relative = format!("cases/{}", entry.file_name().to_string_lossy());
        if !indexed.contains(&relative) {
            return Err(format!("unindexed fixture: {relative}"));
        }
    }
    Ok(())
}

pub fn validate_relative(path: &str) -> Result<(), &'static str> {
    let candidate = Path::new(path);
    if candidate.is_absolute() || path.contains('\\') || path.contains('\0') {
        return Err("absolute or noncanonical path");
    }
    if candidate
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("path traversal or non-normal component");
    }
    if !path.starts_with("cases/") || !path.ends_with(".json") {
        return Err("fixture path shape");
    }
    Ok(())
}

pub fn scan_value(value: &Value, root: &str) -> Result<(), String> {
    let mut stack = vec![(value, root.to_owned())];
    while let Some((current, path)) = stack.pop() {
        match current {
            Value::String(text) => {
                scan_string(text, &path).map_err(|error| format!("{path}: {error}"))?;
            }
            Value::Array(values) => {
                for (index, child) in values.iter().enumerate() {
                    stack.push((child, format!("{path}/{index}")));
                }
            }
            Value::Object(values) => {
                for (key, child) in values {
                    if let Value::String(text) = child {
                        scan_secret_field(key, text)
                            .map_err(|error| format!("{path}/{key}: {error}"))?;
                    }
                    stack.push((child, format!("{path}/{key}")));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn scan_secret_field(key: &str, value: &str) -> Result<(), &'static str> {
    let key = key.to_ascii_lowercase();
    if [
        "authorization",
        "cookie",
        "set-cookie",
        "api_key",
        "access_token",
        "refresh_token",
    ]
    .iter()
    .any(|secret| key == *secret || key.ends_with(secret))
        && !approved_placeholder(value)
    {
        return Err("secret field is not an approved placeholder");
    }
    Ok(())
}

fn scan_string(value: &str, path: &str) -> Result<(), &'static str> {
    let lowered = value.to_ascii_lowercase();
    if lowered.contains("sk-") || lowered.contains("bearer ") || lowered.contains("set-cookie") {
        return Err("secret-shaped text");
    }
    if value.contains("/Users/") || value.contains("/home/") || value.starts_with("file://") {
        return Err("private path");
    }
    if uuid::Uuid::parse_str(value).is_ok() {
        return Err("unsanitized UUID");
    }
    if chrono::DateTime::parse_from_rfc3339(value).is_ok() {
        return Err("unsanitized date");
    }
    if content_path(path)
        && !value.is_empty()
        && !approved_placeholder(value)
        && !matches!(value, "[DONE]" | "assistant" | "user" | "stop")
    {
        return Err("unapproved free text");
    }
    Ok(())
}

fn content_path(path: &str) -> bool {
    [
        "/content",
        "/text",
        "/delta",
        "/input",
        "/instructions",
        "/message",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

fn approved_placeholder(value: &str) -> bool {
    value.starts_with('<')
        && value.ends_with('>')
        && value.len() <= 128
        && value[1..value.len() - 1].bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b':')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn structural_secret_mutations_fail_every_surface() {
        for path in [
            "index/capture_command",
            "case/request/headers/authorization",
            "case/request/headers/cookie",
            "case/request/body/content",
            "case/expected/body/text",
            "case/expected/events/0/data/text",
        ] {
            assert!(scan_value(&json!({"content":"Bearer planted"}), path).is_err());
        }
        assert!(scan_value(&json!({"cookie":"session=value"}), "case").is_err());
        assert!(scan_value(&json!({"text":"unapproved transcript sentence"}), "case").is_err());
    }

    #[test]
    fn path_mutations_fail() {
        for path in [
            "../case.json",
            "/cases/a.json",
            "cases/../a.json",
            "cases\\a.json",
        ] {
            assert!(validate_relative(path).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn stale_unindexed_and_symlink_mutations_fail() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("task77-sanitize-{}", std::process::id()));
        let cases = root.join("cases");
        fs::create_dir_all(&cases).expect("mutation directory");
        fs::write(cases.join("a.json"), b"{}\n").expect("mutation fixture");
        assert!(read_verified(&root, "cases/a.json", "stale").is_err());
        let indexed = BTreeSet::from(["cases/a.json".to_owned()]);
        fs::write(cases.join("extra.json"), b"{}\n").expect("unindexed fixture");
        assert!(corpus_layout(&root, &indexed).is_err());
        fs::remove_file(cases.join("extra.json")).expect("remove unindexed mutation");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("a.json", cases.join("link.json"))
                .expect("symlink mutation");
            assert!(read_verified(&root, "cases/link.json", "stale").is_err());
        }
        fs::remove_dir_all(root).expect("remove mutation directory");
    }
}
