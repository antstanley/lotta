//! Public async generic contract suites reusable by every concrete adapter.

mod clock_ids;
mod common;
/// Canonical adapter-independent contract fixture constructors.
pub mod fixtures;
mod memfs;
mod process;
mod provider;
mod stores;
mod tool;

pub use clock_ids::{clock_contract, id_generator_contract};
pub use memfs::memfs_contract;
pub use process::{ProcessContractScenario, child_process_contract, sandbox_contract};
pub use provider::{
    ProviderContractScenario, ProviderContractShape, provider_contract,
    provider_contract_with_shape,
};
pub use stores::{agent_store_contract, conversation_store_contract, transcript_store_contract};
pub use tool::{
    ToolContractCase, ToolContractObservation, ToolContractProbe, ToolContractScenario,
    ToolContractSnapshot, tool_contract,
};
