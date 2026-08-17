//! Persistent domain entities and compatibility projections.

mod bounded;
mod catalog;
mod channel;
mod common;
mod core;
mod schedule;
mod transcript;

pub(crate) use crate::bounds::{
    JSON_DEPTH_MAX, JSON_ITEMS_MAX, JSON_PROPERTIES_MAX, STRING_ITEMS_MAX,
    UNBOUNDED_COLLECTION_ITEMS_MAX, UNBOUNDED_MAP_FIELDS_MAX,
};
pub use bounded::{BoundedJsonValue, BoundedMap, BoundedVec};
pub use catalog::{ModelDescriptor, ProviderConnection};
pub use channel::{
    ChannelAccount, ChannelChatType, ChannelRoute, DmPolicy, GroupPolicy, RuntimeChannelAccount,
    RuntimeChannelRoute,
};
pub use common::EntityExtras;
pub use core::{
    Agent, Conversation, LocalMessage, LocalMessageRole, MemoryBlockInput, Run, RunStatus,
};
pub use schedule::{
    IanaTimezone, Schedule, ScheduleCancelReason, ScheduleRunOutcome, ScheduleStatus,
};
pub use transcript::{
    CompactionEntry, CompactionEntryType, MessageEntry, MessageEntryType, ProviderStack,
    SessionEntry, SessionEntryType, TranscriptEntry, TranscriptManifest, TranscriptMessageFormat,
    TranscriptSchemaVersion,
};

pub(crate) mod nullable {
    use serde::{Deserialize, Deserializer};

    #[allow(clippy::option_option, reason = "three-state JSON presence contract")]
    pub(crate) fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

#[cfg(test)]
mod channel_tests;
#[cfg(test)]
mod preserves_unknown_fields;
#[cfg(test)]
mod schedule_tests;
#[cfg(test)]
mod schema_conformance;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod model_fields {
    use super::tests;

    #[test]
    fn agent_fields() {
        tests::model_fields::agent_nested_handle_and_settings_round_trip();
    }

    #[test]
    fn conversation_fields() {
        tests::model_fields::conversation_nested_handle_and_settings_round_trip();
    }
}
