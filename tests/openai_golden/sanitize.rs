use crate::schema::{DynamicKind, FIXTURE_BYTES_MAX, Fixture, FixtureIndex, IndexCase, SourcePin};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

const APPROVED_SOURCES: &[(&str, &str, &str)] = &[
    (
        "handler",
        "src/websocket/app-server-openai.ts",
        "22058763c116e87ea0d8decaa675c8c4df711e51f4895936832455a0376c2849",
    ),
    (
        "turn_bridge",
        "src/websocket/app-server-openai-turn.ts",
        "08f445e1ed19304dff8227ae058e71f555ec69142b11e7c0073154e916eec531",
    ),
    (
        "common",
        "src/websocket/app-server-openai-common.ts",
        "3053fcf764a9048ffafefba74d780c40a35ca1ed88df0131d6bc4f40d8482ccd",
    ),
    (
        "lockfile",
        "bun.lock",
        "0a8cad33168b97cfd08958d29b853f683eff9a36f42d1709e761990a74adc26e",
    ),
    (
        "capture_runner",
        "tools/openai-capture-runner.ts",
        "6869d6ed36dd1a67cc884c8d276abbf98f182f6c8573e51d9833984dcf21de3f",
    ),
];

pub fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/openai")
}

pub fn load_index() -> FixtureIndex {
    let path = fixtures_root().join("index.json");
    regular_file(&path).expect("fixture index confinement");
    let bytes = fs::read(path).expect("fixture index");
    assert!(bytes.len() <= FIXTURE_BYTES_MAX, "index byte bound");
    let original: Value = serde_json::from_slice(&bytes).expect("index original JSON");
    scan_value(&original, "index").expect("sanitize original index JSON");
    let index: FixtureIndex = serde_json::from_slice(&bytes).expect("strict fixture index JSON");
    assert_eq!(
        index.sources,
        approved_sources(),
        "approved source provenance"
    );
    index
}

pub fn load_fixture(entry: &IndexCase) -> Fixture {
    let root = fixtures_root().canonicalize().expect("fixture root");
    let bytes = read_verified(&root, &entry.path, &entry.sha256)
        .unwrap_or_else(|error| panic!("{}: {error}", entry.name));
    let original: Value = serde_json::from_slice(&bytes).expect("case original JSON");
    scan_value(&original, &entry.name).expect("sanitize original case JSON");
    serde_json::from_slice(&bytes).expect("strict fixture case JSON")
}

fn approved_sources() -> BTreeMap<String, SourcePin> {
    APPROVED_SOURCES
        .iter()
        .map(|(name, path, sha256)| {
            (
                (*name).to_owned(),
                SourcePin {
                    path: (*path).to_owned(),
                    sha256: (*sha256).to_owned(),
                },
            )
        })
        .collect()
}

fn regular_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("fixture must be a regular non-symlink file".to_owned());
    }
    Ok(())
}

fn read_verified(root: &Path, relative: &str, expected_hash: &str) -> Result<Vec<u8>, String> {
    validate_relative(relative).map_err(str::to_owned)?;
    let path = root.join(relative);
    regular_file(&path)?;
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
    let approved = indexed
        .iter()
        .cloned()
        .chain(["index.json".to_owned(), "cases".to_owned()])
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    walk_layout(root, root, &approved, &mut seen)?;
    if seen != approved {
        return Err("fixture corpus allowlist mismatch".to_owned());
    }
    Ok(())
}

fn walk_layout(
    root: &Path,
    directory: &Path,
    approved: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("fixture corpus symlink".to_owned());
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_str()
            .ok_or_else(|| "non-UTF8 fixture path".to_owned())?
            .to_owned();
        if !approved.contains(&relative) {
            return Err(format!("unapproved fixture corpus entry: {relative}"));
        }
        if !seen.insert(relative) {
            return Err("duplicate fixture corpus entry".to_owned());
        }
        if metadata.is_dir() {
            walk_layout(root, &path, approved, seen)?;
        } else if !metadata.is_file() {
            return Err("fixture corpus non-file".to_owned());
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
            Value::String(text) => scan_string(text, &path)
                .and_then(|()| scan_secret_field(path.rsplit('/').next().unwrap_or(""), text))
                .map_err(|error| format!("{path}: {error}"))?,
            Value::Array(values) => push_array(&mut stack, values, &path),
            Value::Object(values) => push_object(&mut stack, values, &path),
            _ => {}
        }
    }
    Ok(())
}

fn push_array<'a>(stack: &mut Vec<(&'a Value, String)>, values: &'a [Value], path: &str) {
    for (index, child) in values.iter().enumerate() {
        stack.push((child, format!("{path}/{index}")));
    }
}

fn push_object<'a>(
    stack: &mut Vec<(&'a Value, String)>,
    values: &'a serde_json::Map<String, Value>,
    path: &str,
) {
    for (key, child) in values {
        stack.push((child, format!("{path}/{key}")));
    }
}

fn scan_secret_field(key: &str, value: &str) -> Result<(), &'static str> {
    let key = key.to_ascii_lowercase();
    let secret = [
        "authorization",
        "cookie",
        "set-cookie",
        "api_key",
        "access_token",
        "refresh_token",
    ]
    .iter()
    .any(|candidate| key == *candidate || key.ends_with(candidate));
    if secret && !(key == "authorization" && value == "<AUTHORIZATION>") {
        return Err("secret field is not the designated placeholder");
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
    if value.starts_with('<') || value.ends_with('>') {
        return validate_placeholder(value, path);
    }
    if content_path(path) && !value.is_empty() && !content_literal(value) {
        return Err("unapproved free text");
    }
    Ok(())
}

fn validate_placeholder(value: &str, path: &str) -> Result<(), &'static str> {
    if static_placeholder(value, path) {
        return Ok(());
    }
    if dynamic_path(path) && dynamic_token(value) {
        return Ok(());
    }
    Err("placeholder is not approved for this field")
}

fn static_placeholder(value: &str, path: &str) -> bool {
    if path.ends_with("/authorization") {
        return value == "<AUTHORIZATION>";
    }
    if path.ends_with("/x-letta-chat-key") {
        return matches!(value, "<CHAT_KEY>" | "<IDEMPOTENCY_CHAT_KEY>");
    }
    if path.ends_with("/idempotency-key") {
        return value == "<IDEMPOTENCY_KEY>";
    }
    if path.ends_with("/previous_response_id") {
        return value == "<FROM:responses_stored_json>";
    }
    if path.contains("/provider_calls/") && path.contains("/inputs/") {
        return content_placeholder(value);
    }
    content_path(path) && content_placeholder(value)
}

fn content_placeholder(value: &str) -> bool {
    matches!(
        value,
        "<ASSISTANT_TEXT>"
            | "<USER_TEXT_A>"
            | "<USER_TEXT_B>"
            | "<USER_TEXT_C>"
            | "<OLD_USER_TEXT>"
            | "<OLD_ASSISTANT_TEXT>"
            | "<NEWEST_USER_TEXT>"
            | "<IDEMPOTENT_USER_TEXT>"
            | "<RESPONSE_USER_TEXT_A>"
            | "<RESPONSE_USER_TEXT_B>"
            | "<RESPONSE_USER_TEXT_C>"
            | "<RESPONSE_USER_TEXT_D>"
            | "<RESPONSE_USER_TEXT_E>"
    )
}

fn dynamic_path(path: &str) -> bool {
    path.contains("/dynamic_map/")
        || path.contains("/relationships/")
        || path.contains("/conversations/deleted/")
        || [
            "/id",
            "/agent_id",
            "/item_id",
            "/conversation_id",
            "/source_id",
            "/target_id",
            "/nonce",
            "/created",
            "/created_at",
            "/timestamp",
        ]
        .iter()
        .any(|suffix| path.ends_with(suffix))
}

fn dynamic_token(value: &str) -> bool {
    DynamicKind::ALL.iter().any(|kind| {
        let prefix = format!("<{}_ID_", kind.token_name());
        value
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix('>'))
            .is_some_and(|number| {
                number.parse::<usize>().is_ok_and(|parsed| parsed > 0) && !number.starts_with('0')
            })
    })
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

fn content_literal(value: &str) -> bool {
    matches!(value, "[DONE]" | "assistant" | "user" | "stop")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn structural_secret_and_placeholder_mutations_fail_every_surface() {
        for path in [
            "index/capture_command",
            "case/request/headers/authorization",
            "case/request/headers/cookie",
            "case/request/body/content",
            "case/expected/body/text",
            "case/expected/events/0/data/text",
            "case/observable/provider_calls/0/inputs/0",
            "case/cursor/nonce",
        ] {
            assert!(scan_value(&json!({"content":"Bearer planted"}), path).is_err());
        }
        for planted in ["<SECRET>", "<USER_PLANTED>", "<AUTHORIZATION_X>"] {
            assert!(scan_value(&json!({"text":planted}), "case").is_err());
        }
    }

    #[test]
    fn dynamic_tokens_are_typed_and_field_confined() {
        assert!(scan_value(&json!({"id":"<MSG_ID_1>"}), "case").is_ok());
        for value in ["<OTHER_ID_1>", "<MSG_ID_0>", "<MSG_ID_01>", "<MSG_ID_X>"] {
            assert!(scan_value(&json!({"id":value}), "case").is_err());
        }
        assert!(scan_value(&json!({"model":"<MSG_ID_1>"}), "case").is_err());
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
    fn stale_unindexed_recursive_and_symlink_mutations_fail() {
        let root = mutation_root();
        let cases = root.join("cases");
        fs::create_dir_all(&cases).expect("mutation directory");
        fs::write(root.join("index.json"), b"{}\n").expect("mutation index");
        fs::write(cases.join("a.json"), b"{}\n").expect("mutation fixture");
        assert!(read_verified(&root, "cases/a.json", "stale").is_err());
        let indexed = BTreeSet::from(["cases/a.json".to_owned()]);
        fs::create_dir(cases.join("nested")).expect("nested mutation");
        assert!(corpus_layout(&root, &indexed).is_err());
        fs::remove_dir(cases.join("nested")).expect("remove nested mutation");
        symlink_mutations(&root, &cases, &indexed);
        fs::remove_dir_all(root).expect("remove mutation directory");
    }

    fn mutation_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("task77-sanitize-{}", std::process::id()))
    }

    #[cfg(unix)]
    fn symlink_mutations(root: &Path, cases: &Path, indexed: &BTreeSet<String>) {
        std::os::unix::fs::symlink("a.json", cases.join("link.json"))
            .expect("case symlink mutation");
        assert!(corpus_layout(root, indexed).is_err());
        fs::remove_file(cases.join("link.json")).expect("remove case symlink");
        fs::remove_file(root.join("index.json")).expect("remove index");
        std::os::unix::fs::symlink("cases/a.json", root.join("index.json"))
            .expect("index symlink mutation");
        assert!(regular_file(&root.join("index.json")).is_err());
    }

    #[cfg(not(unix))]
    fn symlink_mutations(_root: &Path, _cases: &Path, _indexed: &BTreeSet<String>) {}
}
