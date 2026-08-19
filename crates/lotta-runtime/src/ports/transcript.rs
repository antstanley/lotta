use super::PortFuture;
use lotta_domain::{AgentId, ConversationId, TranscriptEntry, TranscriptManifest};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Persists append-only canonical conversation transcripts.
///
/// # Preconditions
/// Callers preserve agent/conversation scope and parent ordering. Load uses a caller-created
/// bounded
/// sender; the manifest is emitted first, followed by entries in storage order.
///
/// # Errors
/// Reports absence, invalid or unsupported formats, repair requirements, conflicts, limits,
/// permissions, cancellation, channel closure, and translated I/O as [`crate::RuntimeError`].
///
/// # Cancellation
/// Loading checks its explicit token and may have emitted a valid prefix. Initialization and append
/// are atomic: cancellation never exposes a partial manifest or JSON line.
///
/// # Ownership
/// Streamed values are owned. Initialization and append borrow validated domain values only for the
/// returned future's lifetime.
pub trait TranscriptStore: Send + Sync {
    /// Streams the manifest then entries with bounded-channel backpressure.
    fn load(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
        items: Sender<TranscriptItem>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()>;
    /// Initializes a manifest and first session entry atomically.
    fn initialize(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
        manifest: &TranscriptManifest,
        session: &TranscriptEntry,
    ) -> PortFuture<'_, ()>;
    /// Appends one complete durable transcript entry.
    fn append(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
        entry: &TranscriptEntry,
    ) -> PortFuture<'_, ()>;
}

/// One ordered item from a transcript load stream.
#[derive(Clone, Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "public port contract preserves owning entry shape"
)]
pub enum TranscriptItem {
    /// Schema and format declaration, emitted exactly once first.
    Manifest(TranscriptManifest),
    /// One accepted entry in storage order.
    Entry(TranscriptEntry),
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest(value: TranscriptManifest) {
        let TranscriptItem::Manifest(value) = TranscriptItem::Manifest(value) else {
            panic!()
        };
        let _: TranscriptManifest = value;
    }
    fn entry(value: TranscriptEntry) {
        let TranscriptItem::Entry(value) = TranscriptItem::Entry(value) else {
            panic!()
        };
        let _: TranscriptEntry = value;
    }
    #[test]
    fn structural_transcript_variants_have_exact_types() {
        let _ = (
            manifest as fn(TranscriptManifest),
            entry as fn(TranscriptEntry),
        );
    }
}
