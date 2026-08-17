//! Bounded MCP configuration, JSON-RPC transports, credentials, and atomic discovery.

/// Public MCP client and validated configuration.
pub mod client;
/// Task 43 credential-containment certificate.
#[cfg(test)]
#[path = "credentials_certificate.rs"]
mod credentials_certificate;
/// Agent-scoped discovery and Task 32 registry integration.
pub mod discovery;
/// Credential-store and OAuth token lifecycle ports.
pub mod oauth;
/// Concrete stdio, legacy SSE, and streamable HTTP protocol adapters.
pub mod transport;

pub use lotta_domain::bounds::{MCP_SERVERS_PER_AGENT_MAX, MCP_TOOLS_PER_SERVER_MAX};

#[cfg(test)]
#[path = "certificate.rs"]
mod certificate;
#[cfg(test)]
#[path = "discovery_certificate.rs"]
mod discovery_certificate;
#[cfg(test)]
#[path = "refresh_certificate.rs"]
mod refresh_certificate;
#[cfg(test)]
mod tests;
#[cfg(test)]
#[path = "transport_certificate.rs"]
mod transport_certificate;
