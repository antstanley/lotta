use super::handshake::*;
use super::test_support::*;
use super::*;
use std::{
    cell::RefCell,
    future::Ready,
    io,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};
use tokio::io::AsyncRead;

#[derive(Default)]
struct DispatchState {
    calls: usize,
    envelopes: Vec<SidecarEnvelope>,
}
struct FakeDispatcher(Rc<RefCell<DispatchState>>);
impl SidecarDispatcher for FakeDispatcher {
    type Dispatch<'a> = Ready<Result<(), SidecarSessionError>>;

    fn dispatch(&mut self, envelope: SidecarEnvelope) -> Self::Dispatch<'_> {
        let mut state = self.0.borrow_mut();
        state.calls += 1;
        state.envelopes.push(envelope);
        std::future::ready(Ok(()))
    }
}

struct CountingReader {
    inner: std::io::Cursor<Vec<u8>>,
    bytes_read: Rc<RefCell<usize>>,
}
impl AsyncRead for CountingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(context, buffer);
        *self.bytes_read.borrow_mut() += buffer.filled().len() - before;
        result
    }
}

#[tokio::test]
async fn rejects_unsupported_version() {
    let mut hello = envelope(SidecarEnvelopeKind::Hello);
    hello.version = SidecarProtocolVersion(99);
    let payload = envelope(SidecarEnvelopeKind::Request);
    let hello_bytes = wire(&[hello]);
    let all = [hello_bytes.clone(), wire(&[payload])].concat();
    let bytes_read = Rc::new(RefCell::new(0));
    let reader = CountingReader {
        inner: std::io::Cursor::new(all),
        bytes_read: Rc::clone(&bytes_read),
    };
    let state = Rc::new(RefCell::new(DispatchState::default()));
    let mut session = SidecarHostSession::new(
        ValidatedSidecarReader::new(reader, policy()),
        FakeDispatcher(Rc::clone(&state)),
        "child-7",
    );
    assert!(matches!(
        session.run().await,
        Err(SidecarSessionError::Version)
    ));
    assert_eq!(state.borrow().calls, 0);
    assert_eq!(*bytes_read.borrow(), hello_bytes.len());
    let child = session.into_child();
    assert_eq!(child, "child-7");
}

#[tokio::test]
async fn production_session_dispatches_exact_accepted_payload() {
    let hello = envelope(SidecarEnvelopeKind::Hello);
    let payload = envelope(SidecarEnvelopeKind::Request);
    let state = Rc::new(RefCell::new(DispatchState::default()));
    let mut session = SidecarHostSession::new(
        ValidatedSidecarReader::new(
            std::io::Cursor::new(wire(&[hello, payload.clone()])),
            policy(),
        ),
        FakeDispatcher(Rc::clone(&state)),
        (),
    );
    session.run().await.unwrap();
    let state = state.borrow();
    assert_eq!(state.calls, 1);
    assert_eq!(state.envelopes, [payload]);
}

#[tokio::test]
async fn enforces_handshake_sequence_and_declarations() {
    let payload = envelope(SidecarEnvelopeKind::Request);
    let mut reader = ValidatedSidecarReader::new(std::io::Cursor::new(wire(&[payload])), policy());
    assert!(matches!(
        reader.accept_handshake().await,
        Err(SidecarSessionError::Handshake)
    ));

    let hello = envelope(SidecarEnvelopeKind::Hello);
    let mut duplicate = ValidatedSidecarReader::new(
        std::io::Cursor::new(wire(&[hello.clone(), hello.clone()])),
        policy(),
    );
    duplicate.accept_handshake().await.unwrap();
    assert!(matches!(
        duplicate.read_payload().await,
        Err(SidecarSessionError::Handshake)
    ));

    let mut capability = hello.clone();
    capability.capability = SidecarCapability::Provider;
    let mut reader =
        ValidatedSidecarReader::new(std::io::Cursor::new(wire(&[capability])), policy());
    assert!(matches!(
        reader.accept_handshake().await,
        Err(SidecarSessionError::Capability)
    ));
}

#[tokio::test]
async fn deadline_only_tightens_and_zero_is_expired() {
    let mut hello = envelope(SidecarEnvelopeKind::Hello);
    hello.timeout_ms = 600;
    let tightened = policy().with_deadline_ms(500).with_deadline_ms(900);
    let mut reader = ValidatedSidecarReader::new(std::io::Cursor::new(wire(&[hello])), tightened);
    assert!(matches!(
        reader.accept_handshake().await,
        Err(SidecarSessionError::Timeout)
    ));

    let hello = envelope(SidecarEnvelopeKind::Hello);
    let expired = policy().with_deadline_ms(0).with_deadline_ms(u64::MAX);
    let mut reader = ValidatedSidecarReader::new(std::io::Cursor::new(wire(&[hello])), expired);
    assert!(matches!(
        reader.accept_handshake().await,
        Err(SidecarSessionError::Timeout)
    ));

    let mut hello = envelope(SidecarEnvelopeKind::Hello);
    hello.timeout_ms = SIDECAR_TIMEOUT_MS_MAX + 1;
    let static_bound = policy().with_deadline_ms(u64::MAX);
    let mut reader =
        ValidatedSidecarReader::new(std::io::Cursor::new(wire(&[hello])), static_bound);
    assert!(matches!(
        reader.accept_handshake().await,
        Err(SidecarSessionError::Timeout)
    ));
}
