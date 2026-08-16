use super::handshake::{SidecarSessionError, SidecarSessionPolicy, ValidatedSidecarReader};
use super::test_support::*;
use super::*;

async fn accept(
    frame: SidecarEnvelope,
    policy: SidecarSessionPolicy,
) -> Result<(), SidecarSessionError> {
    ValidatedSidecarReader::new(std::io::Cursor::new(wire(&[frame])), policy)
        .accept_handshake()
        .await
}

#[tokio::test]
async fn accepts_frame_length() {
    assert!(
        accept(envelope(SidecarEnvelopeKind::Hello), policy())
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn accepts_version() {
    assert!(
        accept(envelope(SidecarEnvelopeKind::Hello), policy())
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn accepts_exact_owner() {
    assert!(
        accept(envelope(SidecarEnvelopeKind::Hello), policy())
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn accepts_declared_capability() {
    assert!(
        accept(envelope(SidecarEnvelopeKind::Hello), policy())
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn accepts_timeout() {
    let mut frame = envelope(SidecarEnvelopeKind::Hello);
    frame.timeout_ms = SIDECAR_TIMEOUT_MS_MAX;
    assert!(accept(frame, policy()).await.is_ok());
}

#[tokio::test]
async fn rejects_frame_length() {
    let mut reader =
        ValidatedSidecarReader::new(std::io::Cursor::new(0_u32.to_be_bytes()), policy());
    assert!(matches!(
        reader.accept_handshake().await,
        Err(SidecarSessionError::Length)
    ));
}
#[tokio::test]
async fn rejects_version() {
    let mut frame = envelope(SidecarEnvelopeKind::Hello);
    frame.version = SidecarProtocolVersion(2);
    assert!(matches!(
        accept(frame, policy()).await,
        Err(SidecarSessionError::Version)
    ));
}
#[tokio::test]
async fn rejects_exact_owner() {
    let mut frame = envelope(SidecarEnvelopeKind::Hello);
    frame.owner = owner("other");
    assert!(matches!(
        accept(frame, policy()).await,
        Err(SidecarSessionError::Owner)
    ));
}
#[tokio::test]
async fn rejects_declared_capability() {
    let mut frame = envelope(SidecarEnvelopeKind::Hello);
    frame.capability = SidecarCapability::Provider;
    assert!(matches!(
        accept(frame, policy()).await,
        Err(SidecarSessionError::Capability)
    ));
}
#[tokio::test]
async fn rejects_timeout() {
    for timeout in [0, 501, SIDECAR_TIMEOUT_MS_MAX + 1] {
        let mut frame = envelope(SidecarEnvelopeKind::Hello);
        frame.timeout_ms = timeout;
        let deadline = if timeout == 501 {
            500
        } else {
            SIDECAR_TIMEOUT_MS_MAX
        };
        assert!(matches!(
            accept(frame, policy().with_deadline_ms(deadline)).await,
            Err(SidecarSessionError::Timeout)
        ));
    }
}

#[tokio::test]
async fn validates_every_inbound_frame_and_failure_is_terminal() {
    let hello = envelope(SidecarEnvelopeKind::Hello);
    let mut bad = envelope(SidecarEnvelopeKind::Request);
    bad.owner = owner("other");
    let good = envelope(SidecarEnvelopeKind::Request);
    let mut reader =
        ValidatedSidecarReader::new(std::io::Cursor::new(wire(&[hello, bad, good])), policy());
    reader.accept_handshake().await.unwrap();
    assert!(matches!(
        reader.read_payload().await,
        Err(SidecarSessionError::Owner)
    ));
    assert!(matches!(
        reader.read_payload().await,
        Err(SidecarSessionError::Handshake)
    ));
}

#[test]
fn validation_order_is_frame_version_owner_capability_timeout() {
    let source = include_str!("handshake.rs");
    let needles = [
        "read_frame",
        "envelope.version",
        "envelope.owner",
        "capabilities.contains",
        "envelope.timeout_ms",
    ];
    let mut offset = 0;
    for needle in needles {
        let found = source[offset..]
            .find(needle)
            .expect("validation stage exists");
        offset += found + needle.len();
    }
}
