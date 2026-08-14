# Task 07 — Normalized provider port contract

**Plan:** [plan.md](../plan.md) · **Certificate:** [07-provider_port_contract-certificate.md](07-provider_port_contract-certificate.md)

**Implements:** [06-model-providers.md §Normalized provider port](../../../06-model-providers.md#normalized-provider-port) · [06-model-providers.md §Streaming invariants](../../../06-model-providers.md#streaming-invariants) · [06-model-providers.md §Responsibilities](../../../06-model-providers.md#responsibilities) · [canonical-types.schema.json $defs.ModelDescriptor](../../../canonical-types.schema.json)
**Depends on:** 06
**Produces:** the `ProviderRequest`/`ProviderEvent`/`ProviderError` contract exactly as `06-model-providers.md` specifies it, so no vendor type can reach the runtime
**Pointers:** `crates/lotta-runtime/src/ports/provider.rs`, `crates/lotta-runtime/src/ports/provider_event.rs`; reference: `../letta-code/src/backend/dev/pi-stream-adapter.ts`, `../letta-code/src/backend/dev/pi-api-streams.ts`, `../letta-code/src/backend/dev/local-provider-errors.ts`

## Steps

- [ ] Define `ProviderRequest` with model handle/settings, system prompt, ordered messages, tool definitions and tool choice, image parts, context and output limits, reasoning controls, and cancellation/deadline
- [ ] Define `ProviderEvent` with exactly `TextDelta`, `ReasoningDelta`, `RedactedReasoning`, `ToolCallStart`, `ToolCallArgumentsDelta`, `ToolCallEnd`, `Usage`, `ProviderMetadata`, `Stop`, and `Error`
- [ ] Define `ProviderError` with exactly the twelve stable kinds: authentication, authorization, invalid request, rate limit, quota, timeout, context overflow, overloaded, unavailable, protocol, cancelled, unknown
- [ ] Reuse the `$defs.ModelDescriptor` type from Task 03 rather than redefining a descriptor here
- [ ] Encode the eight `06-model-providers.md` §Streaming invariants as trait-level contract tests: text/reasoning order, stable tool-call IDs, bounded partial-argument buffering validated only at tool-call end, monotonic usage, single stop, cancellation suppressing late events, image policy, and metadata persisted without secrets
- [ ] Add a bound for buffered partial tool-call arguments and assert it before allocation

## Definition of done

- [ ] `ProviderEvent` has exactly the ten variants of `06-model-providers.md` §Normalized provider port, including both reasoning variants and `ProviderMetadata`
- [ ] `ProviderError` carries exactly the twelve stable kinds and no vendor error type crosses the port
- [ ] `ModelDescriptor` is the `canonical-types.schema.json` shape (`handle`, `provider_id`, `available`, optional `context_window`, `model_settings`) and is not redefined in the port
- [ ] All eight `06-model-providers.md` §Streaming invariants are expressed as executable contract tests over the port
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(ports::provider)'` and sees the ten-variant event enum, twelve-kind error enum, domain `ModelDescriptor`, and eight streaming-invariant cases pass
