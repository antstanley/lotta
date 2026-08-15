//! Transcript byte bounds, bounded row encoding, and streaming row validation.

use crate::confinement::validate_regular_file;
use crate::{StoreError, StoreErrorKind};
use lotta_domain::TranscriptEntry;
use std::io::{BufRead, BufReader, Read as _, Write};
use std::path::Path;

/// Maximum compact JSON payload bytes in one transcript row, excluding its terminating LF.
pub const TRANSCRIPT_LINE_BYTES_MAX: usize = 8 * 1_024 * 1_024;
/// Maximum final `messages.jsonl` bytes, including every terminating LF.
pub const TRANSCRIPT_BYTES_MAX: u64 = 16 * 1_024 * 1_024 * 1_024;
const ROW_BUFFER_BYTES: usize = TRANSCRIPT_LINE_BYTES_MAX + 2;

pub(crate) fn final_size(current: u64, added: u64, path: &Path) -> Result<u64, StoreError> {
    let size = current
        .checked_add(added)
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?;
    if current >= TRANSCRIPT_BYTES_MAX || size > TRANSCRIPT_BYTES_MAX {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    Ok(size)
}

pub(crate) fn encode_line(entry: &TranscriptEntry, path: &Path) -> Result<Vec<u8>, StoreError> {
    let mut sink = LineJson::new(path)?;
    serde_json::to_writer(&mut sink, entry).map_err(|_| sink.error())?;
    Ok(sink.finish())
}

/// Streams rows from a confined regular transcript. Callback payloads exclude the terminating LF.
///
/// # Errors
/// Returns typed path, metadata, total-size, line-size, allocation, callback, or I/O failures.
pub fn read_rows(
    root: &Path,
    path: &Path,
    mut callback: impl FnMut(usize, &[u8]) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    read_rows_terminated(root, path, |number, row, _terminated| callback(number, row))
}

pub(crate) fn read_rows_terminated(
    root: &Path,
    path: &Path,
    mut callback: impl FnMut(usize, &[u8], bool) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    validate_regular_file(root, path)?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| StoreError::from_io(path, &error))?;
    if metadata.len() > TRANSCRIPT_BYTES_MAX {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    let file = std::fs::File::open(path).map_err(|error| StoreError::from_io(path, &error))?;
    let mut reader = BufReader::new(file);
    let mut row = Vec::new();
    row.try_reserve_exact(ROW_BUFFER_BYTES)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    stream_rows(&mut reader, path, &mut row, &mut callback)
}

fn stream_rows(
    reader: &mut impl BufRead,
    path: &Path,
    row: &mut Vec<u8>,
    callback: &mut impl FnMut(usize, &[u8], bool) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut number = 0_usize;
    loop {
        row.clear();
        let read = reader
            .by_ref()
            .take(ROW_BUFFER_BYTES as u64)
            .read_until(b'\n', row)
            .map_err(|error| StoreError::from_io(path, &error))?;
        if read == 0 {
            return Ok(());
        }
        let terminated = row.last() == Some(&b'\n');
        let payload = if terminated {
            &row[..read - 1]
        } else {
            row.as_slice()
        };
        if payload.len() > TRANSCRIPT_LINE_BYTES_MAX {
            return Err(StoreError::new(StoreErrorKind::Limit, path));
        }
        number = number
            .checked_add(1)
            .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?;
        callback(number, payload, terminated)?;
    }
}

struct LineJson<'a> {
    path: &'a Path,
    bytes: Vec<u8>,
    failed: bool,
}

impl<'a> LineJson<'a> {
    fn new(path: &'a Path) -> Result<Self, StoreError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(TRANSCRIPT_LINE_BYTES_MAX + 1)
            .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
        Ok(Self {
            path,
            bytes,
            failed: false,
        })
    }

    fn finish(mut self) -> Vec<u8> {
        self.bytes.push(b'\n');
        self.bytes
    }

    fn error(&self) -> StoreError {
        let kind = if self.failed {
            StoreErrorKind::Limit
        } else {
            StoreErrorKind::Parse
        };
        StoreError::new(kind, self.path)
    }
}

impl Write for LineJson<'_> {
    fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        let length = self.bytes.len().checked_add(chunk.len()).ok_or_else(|| {
            self.failed = true;
            std::io::Error::other("bounded transcript line")
        })?;
        if length > TRANSCRIPT_LINE_BYTES_MAX {
            self.failed = true;
            return Err(std::io::Error::other("bounded transcript line"));
        }
        self.bytes.extend_from_slice(chunk);
        Ok(chunk.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::test_support::root_file;

    #[test]
    fn checked_total_below_exact_and_above() {
        let path = Path::new("/tmp/messages.jsonl");
        assert_eq!(final_size(1, 1, path).expect("below"), 2);
        assert_eq!(
            final_size(TRANSCRIPT_BYTES_MAX - 1, 1, path).expect("exact"),
            TRANSCRIPT_BYTES_MAX
        );
        assert_eq!(
            final_size(TRANSCRIPT_BYTES_MAX - 1, 2, path)
                .expect_err("above")
                .kind(),
            StoreErrorKind::Limit
        );
    }

    #[test]
    fn production_reader_accepts_below_and_exact_payload() {
        for payload in [TRANSCRIPT_LINE_BYTES_MAX - 1, TRANSCRIPT_LINE_BYTES_MAX] {
            let (root, path) = root_file("bounds-accept", payload, true);
            let mut observed = 0;
            read_rows(root.path(), &path, |row, bytes| {
                assert_eq!(row, 1);
                observed = bytes.len();
                Ok(())
            })
            .expect("read rows");
            assert_eq!(observed, payload);
        }
    }

    #[test]
    fn production_reader_rejects_payload_above() {
        let (root, path) = root_file("bounds-line-above", TRANSCRIPT_LINE_BYTES_MAX + 1, true);
        assert_eq!(
            read_rows(root.path(), &path, |_, _| Ok(()))
                .expect_err("line above")
                .kind(),
            StoreErrorKind::Limit
        );
    }

    #[test]
    fn production_reader_rejects_sparse_total_before_scan() {
        let (root, path) = root_file("bounds-total-above", 1, true);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open sparse")
            .set_len(TRANSCRIPT_BYTES_MAX + 1)
            .expect("set sparse length");
        assert_eq!(
            read_rows(root.path(), &path, |_, _| panic!("must not scan"))
                .expect_err("total above")
                .kind(),
            StoreErrorKind::Limit
        );
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("cleanup open")
            .set_len(0)
            .expect("cleanup sparse");
    }
}
