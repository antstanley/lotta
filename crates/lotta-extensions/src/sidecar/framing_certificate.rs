use super::framing::*;
use super::*;
use serde_json::{Value, json};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt};

#[tokio::test]
async fn golden_v1_fixture_is_exact_prefix_and_canonical_json() {
    let value = json!({"message": "pony-tail"});
    let expected_payload = br#"{"message":"pony-tail"}"#;
    let mut expected = u32::try_from(expected_payload.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    expected.extend_from_slice(expected_payload);

    let mut wire = Vec::new();
    write_frame(&mut wire, SidecarFrameLimit::bounded(256), &value)
        .await
        .unwrap();
    const {
        assert!(SIDECAR_FRAME_PREFIX_BYTES == 4);
        assert!(SIDECAR_FRAME_PREFIX_BIG_ENDIAN);
    }
    assert_eq!(wire, expected);
    let decoded: Value = read_frame(
        &mut std::io::Cursor::new(expected),
        SidecarFrameLimit::bounded(256),
    )
    .await
    .unwrap();
    assert_eq!(decoded, value);
}

#[tokio::test]
async fn round_trips_frame() {
    let value = json!({"message": "pony-tail"});
    let (mut writer, mut reader) = tokio::io::duplex(256);
    write_frame(&mut writer, SidecarFrameLimit::mod_host(), &value)
        .await
        .unwrap();
    drop(writer);
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix).await.unwrap();
    let expected = serde_json::to_vec(&value).unwrap();
    assert_eq!(prefix, u32::try_from(expected.len()).unwrap().to_be_bytes());
    let mut payload = vec![0; expected.len()];
    reader.read_exact(&mut payload).await.unwrap();
    assert_eq!(payload, expected);
}

struct PrefixOnly {
    prefix: std::io::Cursor<[u8; 4]>,
    payload_polls: usize,
}
impl AsyncRead for PrefixOnly {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.prefix.position() < 4 {
            return Pin::new(&mut self.prefix).poll_read(cx, buffer);
        }
        self.payload_polls += 1;
        panic!("oversized frame payload was read")
    }
}

#[derive(Default)]
struct CountingAllocator {
    calls: usize,
    current: usize,
    peak: usize,
    largest: usize,
}
impl FrameAllocator for CountingAllocator {
    fn allocate(&mut self, length: usize) -> Result<Vec<u8>, FramingError> {
        self.calls += 1;
        self.current += length;
        self.peak = self.peak.max(self.current);
        self.largest = self.largest.max(length);
        let allocation = FallibleFrameAllocator.allocate(length)?;
        self.current -= length;
        Ok(allocation)
    }
}

#[tokio::test]
async fn does_not_allocate_before_check() {
    let limit = SidecarFrameLimit::bounded(32);
    let mut reader = PrefixOnly {
        prefix: std::io::Cursor::new(33_u32.to_be_bytes()),
        payload_polls: 0,
    };
    let mut allocator = CountingAllocator::default();
    assert!(matches!(
        read_frame_with_allocator::<_, Value, _>(&mut reader, limit, &mut allocator).await,
        Err(FramingError::Length)
    ));
    assert_eq!(allocator.calls, 0);
    assert_eq!(allocator.current, 0);
    assert_eq!(allocator.peak, 0);
    assert_eq!(reader.payload_polls, 0);
}

#[tokio::test]
async fn rejects_oversized_declared_length() {
    let limit = SidecarFrameLimit::bounded(32);
    let wire = 33_u32.to_be_bytes();
    assert!(matches!(
        read_frame::<_, Value>(&mut std::io::Cursor::new(wire), limit).await,
        Err(FramingError::Length)
    ));
}

#[tokio::test]
async fn at_limit_allocates_once_within_limit() {
    let limit = SidecarFrameLimit::bounded(8);
    let bytes = b"12345678";
    let wire = [
        &u32::try_from(bytes.len()).unwrap().to_be_bytes()[..],
        bytes,
    ]
    .concat();
    let mut allocator = CountingAllocator::default();
    let value: Value =
        read_frame_with_allocator(&mut std::io::Cursor::new(wire), limit, &mut allocator)
            .await
            .unwrap();
    assert_eq!(value, json!(12_345_678));
    assert_eq!(allocator.calls, 1);
    assert_eq!(allocator.current, 0);
    assert_eq!(allocator.peak, limit.bytes_max());
    assert!(allocator.largest <= limit.bytes_max());
}

#[tokio::test]
async fn exercises_boundaries_and_malformed_input() {
    let limit = SidecarFrameLimit::bounded(8);
    for bytes in [b"0".as_slice(), b"12345678"] {
        let wire = [
            &u32::try_from(bytes.len()).unwrap().to_be_bytes()[..],
            bytes,
        ]
        .concat();
        let mut reader = std::io::Cursor::new(wire);
        assert!(read_frame::<_, Value>(&mut reader, limit).await.is_ok());
    }
    for wire in [
        vec![],
        vec![0, 0],
        0_u32.to_be_bytes().to_vec(),
        9_u32.to_be_bytes().to_vec(),
        [4_u32.to_be_bytes().as_slice(), b"nul"].concat(),
        [2_u32.to_be_bytes().as_slice(), &[0xff, 0xff]].concat(),
        [2_u32.to_be_bytes().as_slice(), b"0x"].concat(),
    ] {
        let mut reader = std::io::Cursor::new(wire);
        assert!(read_frame::<_, Value>(&mut reader, limit).await.is_err());
    }
    let mut sink = Vec::new();
    assert!(matches!(
        write_frame(&mut sink, limit, &"123456789").await,
        Err(FramingError::Length)
    ));
    assert!(sink.is_empty());
}
