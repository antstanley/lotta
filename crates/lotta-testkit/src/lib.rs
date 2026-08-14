//!
//! This crate supports tests across workspace boundaries with deterministic clocks, identifiers,
//! bounded in-memory port fakes, reusable adapter contracts, confined fixtures, and owned roots.
//! Production crates must not depend on it outside test and development dependency scopes.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Deterministic clock support.
pub mod clock;
/// Reusable generic port contract suites.
pub mod contract;
/// Public in-memory effect-port fakes.
pub mod fakes;
/// Confined golden fixture loading.
pub mod fixtures;
/// Deterministic identifier generation.
pub mod ids;
/// Owned temporary test roots.
pub mod roots;

macro_rules! port_matrix {
    ($macro:ident) => {
        $macro! {
            Clock, clock, fake_clock, run_clock_contract,
                FakeClock, lotta_domain::Clock, clock_fixture;
            IdGenerator, id_generator, fake_id_generator, run_id_generator_contract,
                FakeIdGenerator, lotta_runtime::ports::IdGenerator, default_fixture;
            AgentStore, agent_store, fake_agent_store, run_agent_store_contract,
                FakeAgentStore, lotta_runtime::ports::AgentStore, default_fixture;
            ConversationStore, conversation_store, fake_conversation_store,
                run_conversation_store_contract, FakeConversationStore,
                lotta_runtime::ports::ConversationStore, default_fixture;
            TranscriptStore, transcript_store, fake_transcript_store,
                run_transcript_store_contract, FakeTranscriptStore,
                lotta_runtime::ports::TranscriptStore, default_fixture;
            MemFs, memfs, fake_memfs, run_memfs_contract,
                FakeMemFs, lotta_runtime::ports::MemFsPort, default_fixture;
            Sandbox, sandbox, fake_sandbox, run_sandbox_contract,
                FakeSandbox, lotta_runtime::ports::SandboxPort, default_fixture;
            ChildProcess, child_process, fake_child_process, run_child_process_contract,
                FakeChildProcess, lotta_runtime::ports::ChildProcessPort, default_fixture;
            Provider, provider, fake_provider, run_provider_contract,
                FakeProvider, lotta_runtime::ports::ProviderPort, default_fixture;
            Tool, tool, fake_tool, run_tool_contract,
                FakeTool, lotta_runtime::ports::ToolPort, default_fixture;
        }
    };
}

#[cfg(test)]
pub(crate) use port_matrix;

macro_rules! define_port_kind {
    ($( $kind:ident, $contract:ident, $fake_test:ident, $helper:ident,
        $adapter:ident, $port:path, $fixture:ident; )*) => {
        /// Canonical effect ports covered by the generated fake and contract matrix.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum PortKind {
            $(
                #[doc = concat!("Generated port row for `", stringify!($port), "`.")]
                $kind,
            )*
        }
    };
}

port_matrix!(define_port_kind);

/// Maximum retained records in any focused testkit collection.
pub const TESTKIT_ITEMS_MAX: usize = 1_024;
/// Maximum bytes accepted from one fixture.
pub const FIXTURE_BYTES_MAX: u64 = 1_048_576;

/// Stable failures produced by testkit helpers and deterministic fakes.
#[derive(Debug, thiserror::Error)]
pub enum TestkitError {
    /// A fixture path was absent below the canonical fixture root.
    #[error("fixture missing: {path}")]
    MissingFixture {
        /// Canonical fixture-relative path.
        path: String,
    },
    /// Fixture bytes were not valid UTF-8.
    #[error("fixture is not UTF-8: {path}")]
    InvalidUtf8Fixture {
        /// Canonical fixture-relative path.
        path: String,
    },
    /// Fixture JSON did not deserialize into the requested type.
    #[error("fixture malformed: {path}")]
    MalformedFixture {
        /// Canonical fixture-relative path.
        path: String,
    },
    /// A path was absolute, traversing, malformed, or outside its owned root.
    #[error("path is not confined: {path}")]
    ConfinedPath {
        /// Rejected path text.
        path: String,
    },
    /// A fixture exceeded the bounded streaming-read limit.
    #[error("fixture exceeds byte limit: {path}")]
    FixtureLimit {
        /// Canonical fixture-relative path.
        path: String,
    },
    /// A named testkit retention limit was exceeded.
    #[error("testkit limit exceeded: {context}")]
    LimitExceeded {
        /// Stable limit context.
        context: &'static str,
    },
    /// A deterministic counter exhausted its complete range.
    #[error("deterministic sequence exhausted: {context}")]
    SequenceExhausted {
        /// Stable counter context.
        context: &'static str,
    },
    /// A deterministic timestamp operation overflowed its representable range.
    #[error("fake clock advance overflow")]
    ClockOverflow,
    /// An owned root could not be created, inspected, read, or removed.
    #[error("testkit local filesystem operation failed: {path}")]
    LocalFilesystem {
        /// Confined path involved in the operation.
        path: String,
    },
}

#[cfg(test)]
mod tests;
