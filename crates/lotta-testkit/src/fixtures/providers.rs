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
    Dialect, Dimension, ErrorKindCase, InventoryEntry, InventoryKind, ProviderCase,
    ProviderCaseRecord, ProviderErrorKind, ProviderIndex, RawFormat, ReasoningFlags, SourceRegion,
    TerminalKind,
};

#[cfg(test)]
mod tests;
