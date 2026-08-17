//! Native, single-attempt HTTP/SSE provider implementations.

pub mod anthropic;
#[cfg(test)]
mod byte_bounds;
#[cfg(test)]
mod contract;
#[cfg(test)]
mod corpus;
#[cfg(test)]
mod error_mapping;
#[cfg(test)]
mod image_policy;
#[cfg(test)]
mod lifecycle;
#[cfg(test)]
mod loopback;
pub mod openai_compatible;
#[cfg(test)]
mod reasoning;
#[cfg(test)]
mod replay;
#[cfg(test)]
mod security;
mod shared;
pub mod sse;
