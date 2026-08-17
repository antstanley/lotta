//! Local endpoint provider adapters and bounded native discovery.

mod common;
/// Native model discovery APIs for local endpoint dialects.
#[path = "discovery.rs"]
mod discovery_api;
/// Public discovery API types.
pub use discovery_api::{
    DISCOVERY_MODELS_MAX, DISCOVERY_REQUESTS_IN_FLIGHT_MAX, DISCOVERY_RESPONSE_BYTES_MAX,
    DiscoveredModel, DiscoveryClient, DiscoveryDialect, ModelCapabilities,
};
/// llama.cpp adapter.
pub mod llama_cpp;
/// LM Studio adapter.
pub mod lmstudio;
/// Ollama local and Cloud adapters.
pub mod ollama;

#[cfg(test)]
#[path = "tests/contract_suite.rs"]
mod contract_suite;
#[cfg(test)]
#[path = "tests/discovery.rs"]
mod discovery;
#[cfg(test)]
#[path = "tests/ollama_stream.rs"]
mod ollama_stream;
#[cfg(test)]
mod replay;
#[cfg(test)]
#[path = "tests/retry_integration.rs"]
mod retry_integration;
#[cfg(test)]
#[path = "tests/unreachable.rs"]
mod unreachable;
