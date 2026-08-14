# Task 49 — Native OpenAI-compatible and Anthropic adapters

**Plan:** [plan.md](../plan.md) · **Certificate:** [49-provider_native_api_adapters-certificate.md](49-provider_native_api_adapters-certificate.md)

**Implements:** [06-model-providers.md §Provider classes](../../../06-model-providers.md#provider-classes) · [06-model-providers.md §Streaming invariants](../../../06-model-providers.md#streaming-invariants)
**Depends on:** 07, 12, 48
**Produces:** two native adapters whose normalized traces match the captured baseline fixtures event for event
**Pointers:** `crates/lotta-providers/src/native/openai_compatible.rs`, `native/anthropic.rs`, `native/sse.rs`; reference: `../letta-code/src/backend/dev/pi-openai-compatible-provider.ts`, `../letta-code/src/backend/dev/pi-api-streams.ts`, `../letta-code/src/backend/dev/pi-image-elision.ts`

## Steps

- [ ] Implement the OpenAI-compatible adapter over `reqwest` and SSE, mapping `ProviderRequest` to the vendor request
- [ ] Implement the Anthropic Messages adapter with its own request and stream mapping
- [ ] Translate both vendor streams into the Task 07 `ProviderEvent` enum, including reasoning and redacted-reasoning where the vendor emits them
- [ ] Map vendor errors to the twelve `ProviderError` kinds without leaking vendor types
- [ ] Apply the request's strict/drop image policy and `PROVIDER_REQUEST_BYTES_MAX`/`PROVIDER_RESPONSE_EVENT_BYTES_MAX`
- [ ] Replay every `fixtures/providers/openai*` and `anthropic*` fixture through the adapter and diff against the expected trace

## Definition of done

- [ ] Both adapters replay their `fixtures/providers/` corpus and produce the expected normalized trace event for event
- [ ] Vendor errors map to the twelve `ProviderError` kinds and no vendor type crosses the port
- [ ] Reasoning and redacted-reasoning events survive translation where the vendor emits them
- [ ] The strict/drop image policy is applied and `PROVIDER_REQUEST_BYTES_MAX`/`PROVIDER_RESPONSE_EVENT_BYTES_MAX` are enforced
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(native::)'` and sees every OpenAI-compatible and Anthropic fixture replay to its expected trace with correct error kinds and image policy
