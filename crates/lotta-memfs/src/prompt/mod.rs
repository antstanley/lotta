//! Owning prompt compilation API over committed `MemFS` revisions.

mod cache;
mod compile;
mod input;
mod record;
mod render;
mod skills;
mod util;

pub use cache::{CacheDelivery, CacheRoot, DeliveryCapability};
pub use compile::PromptCompiler;
pub use input::{PromptInputs, PromptSections, PromptSkill, PromptText};
pub use record::CompiledPromptRecord;

#[cfg(test)]
mod evidence;
#[cfg(test)]
mod mid_conversation_injection;
#[cfg(test)]
mod record_shape;
#[cfg(test)]
mod render_fixtures;
#[cfg(test)]
mod skills_fixtures;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod uncommitted_not_authoritative;
