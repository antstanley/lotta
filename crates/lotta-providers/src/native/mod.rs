//! Native, single-attempt HTTP/SSE provider implementations.

pub mod anthropic;
#[cfg(test)]
pub(crate) mod byte_bounds;
#[cfg(test)]
mod contract;
#[cfg(test)]
mod corpus;
#[cfg(test)]
mod error_mapping;
#[cfg(test)]
pub(crate) mod image_policy;
#[cfg(test)]
pub(crate) mod lifecycle;
#[cfg(test)]
pub(crate) mod loopback;
pub mod openai_compatible;
mod registry;
pub use registry::{NativeAdapterFactory, NativeAdapterRegistry, NativeFactoryFuture};
#[cfg(test)]
mod reasoning;
#[cfg(test)]
mod replay;
#[cfg(test)]
pub(crate) mod security;
pub(crate) mod shared;
pub mod sse;
