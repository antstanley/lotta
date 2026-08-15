//! Exact transcript manifest encoding and bounded validation.

use crate::adapter::{BoundedJson, read_record};
use crate::{StoreError, StoreErrorKind};
use lotta_domain::{
    ProviderStack, TranscriptManifest, TranscriptMessageFormat, TranscriptSchemaVersion,
};
use std::io::Write as _;
use std::path::Path;

const REQUIRED_KEYS: [&str; 4] = [
    "schema_version",
    "message_format",
    "provider_stack",
    "created_at",
];
const OPTIONAL_KEYS: [&str; 3] = ["migrated_from", "migrated_at", "backup_path"];

/// Current manifest schema number.
pub const TRANSCRIPT_SCHEMA_VERSION: u8 = 2;

/// Encodes an exact supported schema-v2 manifest as normalized pretty JSON plus LF.
///
/// # Errors
/// Rejects unsupported constants, over-limit JSON, or serialization failure.
pub fn encode(manifest: &TranscriptManifest, path: &Path) -> Result<Vec<u8>, StoreError> {
    validate_current(manifest, path)?;
    let mut sink = BoundedJson::new(path);
    serde_json::to_writer_pretty(&mut sink, manifest).map_err(|_| sink.error())?;
    sink.write_all(b"\n").map_err(|_| sink.error())?;
    Ok(sink.into_bytes())
}

/// Reads a bounded duplicate-key-free current or legacy baseline manifest.
///
/// # Errors
/// Rejects malformed, unknown-field, unsupported-pair, provider, bound, or confinement failures.
pub fn read(path: &Path) -> Result<TranscriptManifest, StoreError> {
    let value: serde_json::Value = read_record(path)?;
    let object = value
        .as_object()
        .ok_or_else(|| StoreError::new(StoreErrorKind::Parse, path))?;
    if REQUIRED_KEYS.iter().any(|key| !object.contains_key(*key))
        || OPTIONAL_KEYS
            .iter()
            .any(|key| object.get(*key).is_some_and(serde_json::Value::is_null))
        || object.keys().any(|key| {
            !REQUIRED_KEYS.contains(&key.as_str()) && !OPTIONAL_KEYS.contains(&key.as_str())
        })
    {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    let manifest =
        serde_json::from_value(value).map_err(|_| StoreError::new(StoreErrorKind::Parse, path))?;
    validate_supported(&manifest, path)?;
    Ok(manifest)
}

pub(crate) fn read_current(path: &Path) -> Result<TranscriptManifest, StoreError> {
    let manifest = read(path)?;
    validate_current(&manifest, path)?;
    Ok(manifest)
}

pub(crate) fn validate_current(
    manifest: &TranscriptManifest,
    path: &Path,
) -> Result<(), StoreError> {
    if manifest.schema_version != TRANSCRIPT_SCHEMA_VERSION
        || manifest.message_format != TranscriptMessageFormat::PiSessionEntryJsonl
        || manifest.provider_stack != ProviderStack::PiAi
    {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn encode_supported_for_test(manifest: &TranscriptManifest, path: &Path) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(manifest).expect("supported manifest");
    bytes.push(b'\n');
    assert!(validate_supported(manifest, path).is_ok());
    bytes
}

fn validate_supported(manifest: &TranscriptManifest, path: &Path) -> Result<(), StoreError> {
    let version = match manifest.schema_version {
        1 => TranscriptSchemaVersion::One,
        2 => TranscriptSchemaVersion::Two,
        _ => return Err(StoreError::new(StoreErrorKind::Parse, path)),
    };
    let valid_pair = matches!(
        (version, manifest.message_format),
        (
            TranscriptSchemaVersion::One,
            TranscriptMessageFormat::PiAiMessageJsonl
        ) | (
            TranscriptSchemaVersion::Two,
            TranscriptMessageFormat::PiSessionEntryJsonl
        )
    );
    if !valid_pair || manifest.provider_stack != ProviderStack::PiAi {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::RECORD_BYTES_MAX;
    use crate::transcript::test_support::{manifest, session, setup, transcript_files};
    use lotta_testkit::fixtures::{FixtureLoader, persistence};
    use lotta_testkit::roots::TemporaryRoot;
    use serde_json::Value;
    use std::collections::BTreeSet;

    const ACCEPT: bool = true;
    const REJECT: bool = false;
    const CLASSIFICATIONS: [(&str, bool); 12] = [
        (
            concat!(
                "baseline_tolerated_versioned_rows/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            ACCEPT,
        ),
        (
            concat!(
                "corrupt_unsupported_manifests/corrupt_json/input/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            REJECT,
        ),
        (
            concat!(
                "corrupt_unsupported_manifests/unsupported_provider/input/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            REJECT,
        ),
        (
            concat!(
                "corrupt_unsupported_manifests/unsupported_schema/input/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            REJECT,
        ),
        (
            concat!(
                "current_typescript_state/conversations/",
                "Y29udmVyc2F0aW9uOmNvbnZlcnNhdGlvbi1maXh0dXJl/manifest.json",
            ),
            ACCEPT,
        ),
        (
            concat!(
                "current_typescript_state/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            ACCEPT,
        ),
        (
            concat!(
                "interrupted_append/input/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            ACCEPT,
        ),
        (
            concat!(
                "interrupted_replacement/candidate/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            ACCEPT,
        ),
        (
            concat!(
                "interrupted_replacement/input/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            ACCEPT,
        ),
        (
            concat!(
                "orphan_result_repair_input/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            ACCEPT,
        ),
        (
            "rust_target_state/conversations/ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ACCEPT,
        ),
        (
            concat!(
                "versioned_legacy_transcript/conversations/",
                "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
            ),
            ACCEPT,
        ),
    ];

    #[test]
    fn reads_corpus_manifests() {
        let loader = FixtureLoader::new();
        let index = persistence::load_index(&loader).expect("index");
        let mut indexed = index
            .inventory
            .iter()
            .filter(|item| item.path.ends_with("/manifest.json"))
            .map(|item| item.path.as_str())
            .collect::<Vec<_>>();
        indexed.sort_unstable();
        let expected = CLASSIFICATIONS
            .iter()
            .map(|(path, _)| *path)
            .collect::<Vec<_>>();
        assert_eq!(indexed, expected);
        for (fixture, accepted) in CLASSIFICATIONS {
            let bytes = loader
                .load_bytes(format!("persistence/{fixture}"))
                .expect("fixture");
            let root = TemporaryRoot::new("manifest-table").expect("root");
            let paths = crate::StorePaths::new(root.path().join("backend")).expect("paths");
            let path = paths.conversations().join("fixture/manifest.json");
            std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
            std::fs::write(&path, bytes).expect("copy fixture");
            let result = read(&path);
            assert_eq!(result.is_ok(), accepted, "{fixture}");
            if !accepted {
                assert_eq!(result.expect_err("reject").kind(), StoreErrorKind::Parse);
            }
        }
        assert_eq!(
            CLASSIFICATIONS
                .iter()
                .filter(|(_, accepted)| *accepted)
                .count(),
            9
        );
    }

    #[tokio::test]
    async fn writes_exact_shape() {
        for optional in [false, true] {
            let label = if optional {
                "manifest-all"
            } else {
                "manifest-required"
            };
            let (_root, store, agent, conversation) = setup(label);
            let mut value = manifest();
            if optional {
                value.migrated_from = Some("schema-1".into());
                value.migrated_at = Some(crate::transcript::test_support::timestamp());
                value.backup_path = Some("messages.backup.jsonl".into());
            }
            store
                .initialize_transcript(&agent, &conversation, &value, &session())
                .await
                .expect("initialize");
            let (path, _) = transcript_files(&store, &agent, &conversation);
            let bytes = std::fs::read(&path).expect("bytes");
            let json: Value = serde_json::from_slice(&bytes).expect("json");
            assert_eq!(
                json.as_object().expect("object").len(),
                if optional { 7 } else { 4 }
            );
            assert_eq!(
                store
                    .read_transcript_manifest(&agent, &conversation)
                    .await
                    .expect("public read"),
                value
            );
            assert_eq!(bytes, encode(&value, &path).expect("canonical"));
        }
    }

    #[tokio::test]
    async fn public_duplicate_and_unknown_keys_are_parse() {
        let duplicate = concat!(
            r#"{"schema_version":2,"schema_version":2,"message_format":"#,
            r#"pi-session-entry-jsonl","provider_stack":"pi-ai","created_at":"#,
            r#"2026-08-15T01:02:03.456Z"}"#,
        );
        let unknown = concat!(
            r#"{"schema_version":2,"message_format":"pi-session-entry-jsonl","#,
            r#"provider_stack":"pi-ai","created_at":"2026-08-15T01:02:03.456Z","#,
            r#"unknown":1}"#,
        );
        for bytes in [duplicate.as_bytes(), unknown.as_bytes()] {
            let (_root, store, agent, conversation) = setup("manifest-bad-key");
            let (path, _) = transcript_files(&store, &agent, &conversation);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
            std::fs::write(path, bytes).expect("manifest");
            assert_eq!(
                store
                    .read_transcript_manifest(&agent, &conversation)
                    .await
                    .expect_err("public parse")
                    .kind(),
                StoreErrorKind::Parse
            );
        }
    }

    #[tokio::test]
    async fn public_optional_nulls_are_parse() {
        for key in OPTIONAL_KEYS {
            let (_root, store, agent, conversation) = setup("manifest-null");
            let (path, _) = transcript_files(&store, &agent, &conversation);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
            let mut value = serde_json::to_value(manifest()).expect("manifest value");
            value
                .as_object_mut()
                .expect("object")
                .insert(key.into(), Value::Null);
            std::fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("manifest");
            assert_eq!(
                store
                    .read_transcript_manifest(&agent, &conversation)
                    .await
                    .expect_err("null optional")
                    .kind(),
                StoreErrorKind::Parse,
                "{key}"
            );
        }
    }

    #[tokio::test]
    async fn oversized_optional_initialization_is_limit_without_residue() {
        let (_root, store, agent, conversation) = setup("manifest-oversized");
        let mut value = manifest();
        value.backup_path = Some("x".repeat(RECORD_BYTES_MAX));
        assert_eq!(
            store
                .initialize_transcript(&agent, &conversation, &value, &session())
                .await
                .expect_err("oversized")
                .kind(),
            StoreErrorKind::Limit
        );
        let (manifest_path, messages) = transcript_files(&store, &agent, &conversation);
        assert!(!manifest_path.exists());
        assert!(!messages.exists());
    }

    #[test]
    fn required_keys_are_exact() {
        assert_eq!(
            BTreeSet::from(REQUIRED_KEYS),
            BTreeSet::from([
                "schema_version",
                "message_format",
                "provider_stack",
                "created_at"
            ])
        );
    }
}
