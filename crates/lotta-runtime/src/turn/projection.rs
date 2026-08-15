use crate::boundary::ProviderEventText;

/// Kind of one retained provider projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionKind {
    /// User-visible assistant text.
    Text,
    /// Provider reasoning text.
    Reasoning,
    /// Non-secret marker replacing opaque provider reasoning.
    RedactedReasoning,
}

/// One fully bounded provider projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnProjection {
    /// Projection classification.
    pub kind: ProjectionKind,
    /// Bounded projected text or marker.
    pub text: ProviderEventText,
}

impl TurnProjection {
    pub(crate) const fn new(kind: ProjectionKind, text: ProviderEventText) -> Self {
        Self { kind, text }
    }
}
