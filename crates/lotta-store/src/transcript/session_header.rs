//! Initial version-three session header encoding and validation.

use crate::{StoreError, StoreErrorKind};
use lotta_domain::TranscriptEntry;
use std::path::Path;

const SESSION_VERSION: u8 = 3;

pub(crate) fn encode(entry: &TranscriptEntry, path: &Path) -> Result<Vec<u8>, StoreError> {
    let TranscriptEntry::Session(session) = entry else {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    };
    if session.version != SESSION_VERSION {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    super::bounds::encode_line(entry, path)
}

pub(crate) fn validate_sequence(
    entries: &[TranscriptEntry],
    path: &Path,
) -> Result<(), StoreError> {
    let Some(TranscriptEntry::Session(session)) = entries.first() else {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    };
    if session.version != SESSION_VERSION
        || entries[1..]
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::Session(_)))
    {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::StoreErrorKind;
    use crate::transcript::test_support::{message, session, setup, transcript_files};

    #[tokio::test]
    async fn written_once_at_row_zero() {
        let (_owned, store, agent, conversation) = setup("session-header-row-zero");
        store
            .initialize_transcript(
                &agent,
                &conversation,
                &crate::transcript::test_support::manifest(),
                &session(),
            )
            .await
            .expect("initialize");
        let (_, messages) = transcript_files(&store, &agent, &conversation);
        let bytes = std::fs::read(&messages).expect("messages");
        assert_eq!(bytes.split(|byte| *byte == b'\n').count(), 2);
        assert_eq!(bytes.last(), Some(&b'\n'));
        let row: serde_json::Value =
            serde_json::from_slice(&bytes[..bytes.len() - 1]).expect("row");
        let object = row.as_object().expect("session object");
        assert_eq!(object.len(), 5);
        assert_eq!(object["type"], "session");
        assert_eq!(object["version"], 3);
        assert_eq!(object["id"], "session-1");
        assert_eq!(object["timestamp"], "2026-08-15T01:02:03.456Z");
        assert_eq!(object["cwd"], "/tmp/ponytail");
    }

    #[tokio::test]
    async fn never_appended_again() {
        let (_owned, store, agent, conversation) = setup("session-header-once");
        store
            .initialize_transcript(
                &agent,
                &conversation,
                &crate::transcript::test_support::manifest(),
                &session(),
            )
            .await
            .expect("initialize");
        let (manifest_path, messages) = transcript_files(&store, &agent, &conversation);
        let before = std::fs::read(&messages).expect("before");
        let manifest_before = std::fs::read(&manifest_path).expect("manifest before");
        let error = store
            .initialize_transcript(
                &agent,
                &conversation,
                &crate::transcript::test_support::manifest(),
                &session(),
            )
            .await
            .expect_err("second initialize");
        assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
        assert_eq!(std::fs::read(&messages).expect("after initialize"), before);
        assert_eq!(
            std::fs::read(&manifest_path).expect("manifest after"),
            manifest_before
        );
        let error = store
            .append_transcript_entry(&agent, &conversation, &session())
            .await
            .expect_err("session append");
        assert_eq!(error.kind(), StoreErrorKind::Parse);
        assert_eq!(std::fs::read(&messages).expect("after"), before);
        assert_eq!(
            super::validate_sequence(
                &[session(), message("entry", None, "x"), session()],
                std::path::Path::new("/tmp/messages.jsonl")
            )
            .expect_err("second session")
            .kind(),
            StoreErrorKind::Parse
        );
    }
}
