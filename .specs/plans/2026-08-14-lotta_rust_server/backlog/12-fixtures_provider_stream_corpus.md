# Task 12 — Provider stream fixture corpus extraction

**Plan:** [plan.md](../plan.md) · **Certificate:** [12-fixtures_provider_stream_corpus-certificate.md](12-fixtures_provider_stream_corpus-certificate.md)

**Implements:** [06-model-providers.md §Conformance](../../../06-model-providers.md#conformance) · [architecture-principles.md §Compatibility architecture](../../../architecture-principles.md#compatibility-architecture) · [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition)
**Depends on:** 07, 09
**Produces:** a sanitized `fixtures/providers/` corpus of captured baseline streams plus their expected normalized `ProviderEvent` traces, checked in before any adapter
**Pointers:** `fixtures/providers/`, `tools/capture-provider-streams.mjs`, `crates/lotta-testkit/src/fixtures/providers.rs`; reference: `../letta-code/src/backend/dev/pi-api-streams.ts`, `../letta-code/src/backend/dev/pi-stream-adapter.ts`, `../letta-code/src/backend/dev/pi-image-elision.ts`

## Steps

- [ ] Capture sanitized raw streams from the pinned TypeScript baseline for the OpenAI-compatible, Anthropic, Ollama, LM Studio, and llama.cpp dialects
- [ ] Record the expected normalized `ProviderEvent` trace for each captured stream using the Task 07 contract
- [ ] Cover the eleven `06-model-providers.md` §Conformance contract dimensions: request mapping, event order, tool-call assembly, usage, cancellation, errors, timeout, context overflow, retry-after, and image policy
- [ ] Include reasoning and redacted-reasoning streams so `ReasoningDelta`/`RedactedReasoning` have coverage
- [ ] Include one error stream per `ProviderError` kind that a live provider can produce
- [ ] Expose a replay helper that drives any `ProviderPort` implementation and diffs against the expected trace

## Definition of done

- [ ] Each of the five dialects has at least one captured raw stream and a matching expected normalized trace
- [ ] The corpus covers every `06-model-providers.md` §Conformance dimension including cancellation, context overflow, retry-after, and image policy
- [ ] Reasoning and redacted-reasoning streams are present, so an adapter dropping them fails replay
- [ ] The replay helper drives an arbitrary `ProviderPort` and reports the first diverging event with its index
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-testkit -E 'test(fixtures::providers::)'` and sees five dialects, ten covered dimensions, reasoning coverage, and divergence reporting pass
