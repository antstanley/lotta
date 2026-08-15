use super::{TranscriptPaths, final_size};
use crate::confinement::validate_regular_file;
use crate::{LottaStorageLock, StoreError};
use std::fs::OpenOptions;
use std::io::Write as _;

pub(crate) fn append(paths: &TranscriptPaths, line: &[u8]) -> Result<(), StoreError> {
    let _lock = LottaStorageLock::try_acquire_confined(&paths.root)?;
    super::manifest::read_current(&paths.manifest)?;
    validate_regular_file(&paths.root, &paths.messages)?;
    let metadata = std::fs::symlink_metadata(&paths.messages)
        .map_err(|error| StoreError::from_io(&paths.messages, &error))?;
    let added = u64::try_from(line.len())
        .map_err(|_| StoreError::new(crate::StoreErrorKind::Limit, &paths.messages))?;
    final_size(metadata.len(), added, &paths.messages)?;
    validate_regular_file(&paths.root, &paths.messages)?;
    let mut file = OpenOptions::new()
        .append(true)
        .open(&paths.messages)
        .map_err(|error| StoreError::from_io(&paths.messages, &error))?;
    file.write_all(line)
        .map_err(|error| StoreError::from_io(&paths.messages, &error))?;
    file.sync_all()
        .map_err(|error| StoreError::from_io(&paths.messages, &error))
}

#[cfg(test)]
mod tests {
    use crate::StoreErrorKind;
    use crate::transcript::test_support::{
        compaction, manifest, message, session, setup, transcript_files,
    };
    use crate::transcript::{TRANSCRIPT_BYTES_MAX, TRANSCRIPT_LINE_BYTES_MAX, encode_line};
    use lotta_domain::{BoundedMap, Timestamp, TranscriptEntry, TranscriptMessageFormat};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    fn rows(path: &std::path::Path) -> Vec<Value> {
        let bytes = std::fs::read(path).expect("transcript");
        assert_eq!(bytes.last(), Some(&b'\n'));
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice(row).expect("independent row"))
            .collect()
    }

    #[tokio::test]
    async fn one_line_per_entry() {
        let (_root, store, agent, conversation) = setup("append-lines");
        store
            .initialize_transcript(&agent, &conversation, &manifest(), &session())
            .await
            .expect("initialize");
        store
            .append_transcript_entry(&agent, &conversation, &message("entry-1", None, "one"))
            .await
            .expect("message");
        let mut with_details = compaction("summary one".into());
        let TranscriptEntry::Compaction(value) = &mut with_details else {
            panic!("compaction fixture")
        };
        let mut details = BTreeMap::new();
        details.insert("stats".into(), json!({"trigger":"manual"}));
        value.details = Some(BoundedMap::new(details).expect("bounded details"));
        store
            .append_transcript_entry(&agent, &conversation, &with_details)
            .await
            .expect("compaction details");
        store
            .append_transcript_entry(&agent, &conversation, &compaction("summary two".into()))
            .await
            .expect("compaction absent details");
        let (_, path) = transcript_files(&store, &agent, &conversation);
        let rows = rows(&path);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0]["type"], "session");
        assert_eq!(rows[1]["type"], "message");
        assert_eq!(rows[2]["type"], "compaction");
        assert!(rows[2].get("details").is_some());
        assert!(rows[3].get("details").is_none());
    }

    #[tokio::test]
    async fn parent_chain_links() {
        let (_root, store, agent, conversation) = setup("append-parents");
        store
            .initialize_transcript(&agent, &conversation, &manifest(), &session())
            .await
            .expect("initialize");
        for entry in [
            message("entry-1", None, "one"),
            message("entry-2", Some("entry-1"), "two"),
        ] {
            store
                .append_transcript_entry(&agent, &conversation, &entry)
                .await
                .expect("append");
        }
        let (_, path) = transcript_files(&store, &agent, &conversation);
        let rows = rows(&path);
        assert!(rows[1]["parentId"].is_null());
        assert_eq!(rows[2]["id"], "entry-2");
        assert_eq!(rows[2]["parentId"], "entry-1");
    }

    #[tokio::test]
    async fn timestamp_encodings_differ() {
        let (_root, store, agent, conversation) = setup("append-timestamps");
        store
            .initialize_transcript(&agent, &conversation, &manifest(), &session())
            .await
            .expect("initialize");
        store
            .append_transcript_entry(&agent, &conversation, &message("entry-1", None, "one"))
            .await
            .expect("append");
        let (_, path) = transcript_files(&store, &agent, &conversation);
        let rows = rows(&path);
        let row = &rows[1];
        let timestamp = row["timestamp"].as_str().expect("RFC3339 string");
        Timestamp::parse_persisted_rfc3339(timestamp).expect("RFC3339 timestamp");
        assert_eq!(
            row["message"]["timestamp"].as_f64(),
            Some(1_776_214_923_456.25)
        );
    }

    #[tokio::test]
    async fn legacy_manifest_rejects_before_append() {
        let (_root, store, agent, conversation) = setup("append-legacy");
        store
            .initialize_transcript(&agent, &conversation, &manifest(), &session())
            .await
            .expect("initialize");
        let (manifest_path, messages) = transcript_files(&store, &agent, &conversation);
        let mut legacy = manifest();
        legacy.schema_version = 1;
        legacy.message_format = TranscriptMessageFormat::PiAiMessageJsonl;
        std::fs::write(
            &manifest_path,
            crate::transcript::manifest::encode_supported_for_test(&legacy, &manifest_path),
        )
        .expect("legacy manifest");
        let before = std::fs::read(&messages).expect("before");
        let metadata = std::fs::metadata(&messages).expect("metadata").len();
        assert_eq!(
            store
                .append_transcript_entry(&agent, &conversation, &message("entry", None, "x"))
                .await
                .expect_err("legacy append")
                .kind(),
            StoreErrorKind::Parse
        );
        assert_eq!(std::fs::read(&messages).expect("after"), before);
        assert_eq!(
            std::fs::metadata(messages).expect("after metadata").len(),
            metadata
        );
    }

    async fn append_boundary(payload: usize, accepted: bool) {
        let label = format!("append-boundary-{payload}");
        let (_root, store, agent, conversation) = setup(&label);
        store
            .initialize_transcript(&agent, &conversation, &manifest(), &session())
            .await
            .expect("initialize");
        let (_, path) = transcript_files(&store, &agent, &conversation);
        let overhead = encode_line(&compaction(String::new()), &path)
            .expect("empty compaction")
            .len()
            - 1;
        let entry = compaction("x".repeat(payload - overhead));
        let encoded = encode_line(&entry, &path);
        if accepted {
            let line = encoded.expect("encoded boundary");
            assert_eq!(line.len() - 1, payload);
            let before = std::fs::metadata(&path).expect("before").len();
            store
                .append_transcript_entry(&agent, &conversation, &entry)
                .await
                .expect("append boundary");
            assert_eq!(
                std::fs::metadata(path).expect("after").len() - before,
                payload as u64 + 1
            );
        } else {
            assert_eq!(
                encoded.expect_err("encoded above").kind(),
                StoreErrorKind::Limit
            );
            let before = std::fs::read(&path).expect("before");
            let metadata = std::fs::metadata(&path).expect("metadata").len();
            assert_eq!(
                store
                    .append_transcript_entry(&agent, &conversation, &entry)
                    .await
                    .expect_err("append above")
                    .kind(),
                StoreErrorKind::Limit
            );
            assert_eq!(std::fs::read(&path).expect("after"), before);
            assert_eq!(
                std::fs::metadata(path).expect("after metadata").len(),
                metadata
            );
        }
    }

    #[tokio::test]
    async fn public_append_exact_line_boundaries() {
        append_boundary(TRANSCRIPT_LINE_BYTES_MAX - 1, true).await;
        append_boundary(TRANSCRIPT_LINE_BYTES_MAX, true).await;
        append_boundary(TRANSCRIPT_LINE_BYTES_MAX + 1, false).await;
    }

    #[tokio::test]
    async fn public_append_reaches_exact_sparse_total_then_rejects() {
        let (_root, store, agent, conversation) = setup("append-sparse-total");
        store
            .initialize_transcript(&agent, &conversation, &manifest(), &session())
            .await
            .expect("initialize");
        let (_, path) = transcript_files(&store, &agent, &conversation);
        let entry = message("small", None, "x");
        let line = encode_line(&entry, &path).expect("small line");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open")
            .set_len(TRANSCRIPT_BYTES_MAX - line.len() as u64)
            .expect("sparse");
        store
            .append_transcript_entry(&agent, &conversation, &entry)
            .await
            .expect("exact total");
        assert_eq!(
            std::fs::metadata(&path).expect("exact metadata").len(),
            TRANSCRIPT_BYTES_MAX
        );
        assert_eq!(
            store
                .append_transcript_entry(&agent, &conversation, &entry)
                .await
                .expect_err("at cap")
                .kind(),
            StoreErrorKind::Limit
        );
        assert_eq!(
            std::fs::metadata(&path).expect("unchanged").len(),
            TRANSCRIPT_BYTES_MAX
        );
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("cleanup open")
            .set_len(0)
            .expect("cleanup sparse");
    }
}
