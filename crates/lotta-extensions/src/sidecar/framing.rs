//! Authoritative Task 42 v1 length-prefixed JSON adapter codec.

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::io::ErrorKind;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::SidecarFrameLimit;

/// Exact wire prefix width defined by Task 42 v1.
pub const SIDECAR_FRAME_PREFIX_BYTES: usize = 4;
/// Task 42 v1 encodes the frame prefix in network (big-endian) byte order.
pub const SIDECAR_FRAME_PREFIX_BIG_ENDIAN: bool = true;

/// Stable terminal framing failures.
#[derive(Debug, thiserror::Error)]
pub enum FramingError {
    /// Zero or oversized declared frame length.
    #[error("invalid sidecar frame length")]
    Length,
    /// Payload allocation failed.
    #[error("sidecar frame allocation failed")]
    Allocation,
    /// Input ended before a complete frame.
    #[error("truncated sidecar frame")]
    Truncated,
    /// JSON encoding failed.
    #[error("sidecar json encoding failed")]
    JsonEncode,
    /// UTF-8 or JSON decoding failed.
    #[error("sidecar json decoding failed")]
    JsonDecode,
    /// Local-pipe I/O failed.
    #[error("sidecar pipe io failed")]
    Io,
}

/// Generic structural JSON limits shared by local sidecar transports.
#[derive(Clone, Copy, Debug)]
pub struct JsonStructureBounds {
    /// Maximum nesting depth, with the root at depth zero.
    pub depth_max: usize,
    /// Maximum UTF-8 bytes in any key or string value.
    pub string_bytes_max: usize,
    /// Maximum items in any array.
    pub array_items_max: usize,
    /// Maximum entries in any object.
    pub map_entries_max: usize,
}

/// Validates JSON structure using the authoritative Task 42 depth/collection ordering.
///
/// # Errors
/// Returns [`FramingError::Length`] at the first structural bound violation.
pub fn validate_json_structure(
    value: &Value,
    bounds: JsonStructureBounds,
) -> Result<(), FramingError> {
    validate_json_at_depth(value, bounds, 0)
}

fn validate_json_at_depth(
    value: &Value,
    bounds: JsonStructureBounds,
    depth: usize,
) -> Result<(), FramingError> {
    if depth > bounds.depth_max {
        return Err(FramingError::Length);
    }
    match value {
        Value::String(text) if text.len() > bounds.string_bytes_max => Err(FramingError::Length),
        Value::Array(values) if values.len() > bounds.array_items_max => Err(FramingError::Length),
        Value::Object(values) if values.len() > bounds.map_entries_max => Err(FramingError::Length),
        Value::Array(values) => values
            .iter()
            .try_for_each(|item| validate_json_at_depth(item, bounds, depth + 1)),
        Value::Object(values) => values.iter().try_for_each(|(key, item)| {
            if key.len() > bounds.string_bytes_max {
                return Err(FramingError::Length);
            }
            validate_json_at_depth(item, bounds, depth + 1)
        }),
        _ => Ok(()),
    }
}

/// Fallible factory for a frame payload buffer.
///
/// The production codec calls this seam only after validating the declared length.
pub trait FrameAllocator {
    /// Allocates a buffer with exactly `length` initialized bytes.
    fn allocate(&mut self, length: usize) -> Result<Vec<u8>, FramingError>;
}

/// Production allocator using fallible exact reservation.
#[derive(Clone, Copy, Debug, Default)]
pub struct FallibleFrameAllocator;
impl FrameAllocator for FallibleFrameAllocator {
    fn allocate(&mut self, length: usize) -> Result<Vec<u8>, FramingError> {
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(length)
            .map_err(|_| FramingError::Allocation)?;
        payload.resize(length, 0);
        Ok(payload)
    }
}

/// Writes exactly one bounded JSON frame to an async local pipe.
pub async fn write_frame<W, T>(
    writer: &mut W,
    limit: SidecarFrameLimit,
    value: &T,
) -> Result<(), FramingError>
where
    W: AsyncWrite + Unpin,
    T: Serialize + ?Sized,
{
    let mut payload = Vec::new();
    let limit_writer = PayloadLimitWriter::new(&mut payload, limit.bytes_max());
    serde_json::to_writer(limit_writer, value).map_err(map_encode)?;
    let length = checked_length(payload.len(), limit)?;
    writer
        .write_all(&length.to_be_bytes())
        .await
        .map_err(map_io)?;
    writer.write_all(&payload).await.map_err(map_io)?;
    writer.flush().await.map_err(map_io)
}

/// Reads exactly one bounded JSON frame using the production fallible allocator.
pub async fn read_frame<R, T>(reader: &mut R, limit: SidecarFrameLimit) -> Result<T, FramingError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    read_frame_with_allocator(reader, limit, &mut FallibleFrameAllocator).await
}

/// Reads one bounded JSON frame using an injected payload allocator.
///
/// This is the same production path as [`read_frame`], with only buffer creation replaced.
pub async fn read_frame_with_allocator<R, T, A>(
    reader: &mut R,
    limit: SidecarFrameLimit,
    allocator: &mut A,
) -> Result<T, FramingError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
    A: FrameAllocator + ?Sized,
{
    let mut prefix = [0_u8; SIDECAR_FRAME_PREFIX_BYTES];
    reader.read_exact(&mut prefix).await.map_err(map_io)?;
    let length = checked_length(u32::from_be_bytes(prefix) as usize, limit)? as usize;
    let mut payload = allocator.allocate(length)?;
    reader.read_exact(&mut payload).await.map_err(map_io)?;
    let text = std::str::from_utf8(&payload).map_err(|_| FramingError::JsonDecode)?;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = T::deserialize(&mut deserializer).map_err(|_| FramingError::JsonDecode)?;
    deserializer.end().map_err(|_| FramingError::JsonDecode)?;
    Ok(value)
}

fn checked_length(length: usize, limit: SidecarFrameLimit) -> Result<u32, FramingError> {
    if length == 0 || length > limit.bytes_max() {
        return Err(FramingError::Length);
    }
    u32::try_from(length).map_err(|_| FramingError::Length)
}

struct PayloadLimitWriter<'a> {
    payload: &'a mut Vec<u8>,
    bytes_max: usize,
}
impl<'a> PayloadLimitWriter<'a> {
    const fn new(payload: &'a mut Vec<u8>, bytes_max: usize) -> Self {
        Self { payload, bytes_max }
    }
}
impl std::io::Write for PayloadLimitWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .payload
            .len()
            .checked_add(bytes.len())
            .filter(|next| *next <= self.bytes_max)
            .ok_or_else(|| std::io::Error::other("sidecar payload limit"))?;
        self.payload
            .try_reserve(next - self.payload.len())
            .map_err(|_| std::io::Error::other("sidecar payload allocation"))?;
        self.payload.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn map_encode(error: serde_json::Error) -> FramingError {
    if error.is_io() {
        FramingError::Length
    } else {
        FramingError::JsonEncode
    }
}

fn map_io(error: std::io::Error) -> FramingError {
    if error.kind() == ErrorKind::UnexpectedEof {
        FramingError::Truncated
    } else {
        FramingError::Io
    }
}
