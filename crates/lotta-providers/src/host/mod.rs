//! Pinned, isolated pi-ai compatibility provider host.

/// Complete typed credential-free catalog.
pub mod catalog;
/// Real bounded local-process host client.
pub mod client;
/// Production bounded provider OAuth manager.
pub mod oauth;
/// Exact resolved package pin validation.
pub mod pin;
/// Bounded host payload protocol.
pub mod protocol;
/// Public single-attempt provider backed by the real host process.
pub mod provider;

#[cfg(test)]
mod inference;
#[cfg(test)]
mod isolation;
#[cfg(test)]
mod lifecycle;
#[cfg(test)]
mod replay;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod version_pin;
