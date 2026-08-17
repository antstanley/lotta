//! Sanitized provider-stream fixture corpus and replay support.

mod convert;
mod load;
mod replay;
mod types;

pub use load::{load_all, load_case, load_index};
pub use replay::{
    ProviderFixtureError, ProviderReplayDivergence, ProviderReplayError, ProviderReplaySide,
    replay_provider,
};
pub use types::{
    Dialect, Dimension, ErrorKindCase, HeaderFixture, InventoryEntry, InventoryKind, ProviderCase,
    ProviderCaseRecord, ProviderErrorKind, ProviderIndex, RESPONSE_HEADERS_MAX, RawFormat,
    ReasoningFlags, ResponseFixture, SourceRegion, TerminalKind,
};

#[cfg(test)]
mod tests;
