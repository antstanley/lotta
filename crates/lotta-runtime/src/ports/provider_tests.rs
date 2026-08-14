#[path = "provider_tests/harness.rs"]
mod harness;

pub(crate) use harness::{
    async_channel_backpressure_and_cancellation, model_descriptor_is_domain_type,
    provider_error_variants, provider_event_variants, provider_request_aggregate_bytes_are_bounded,
    provider_request_fields_have_exact_types, request_for_structural_test,
};

#[path = "provider_tests/streaming_invariants.rs"]
pub(crate) mod streaming_invariants;
