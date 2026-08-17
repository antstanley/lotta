# Task 50 — Local endpoint adapters and native discovery

**Plan:** [plan.md](../plan.md) · **Certificate:** [50-provider_local_endpoint_adapters-certificate.md](50-provider_local_endpoint_adapters-certificate.md)

**Implements:** [06-model-providers.md §Provider classes](../../../06-model-providers.md#provider-classes) · [06-model-providers.md §Conformance](../../../06-model-providers.md#conformance)
**Depends on:** 07, 12, 48
**Produces:** Ollama, Ollama Cloud, LM Studio, and llama.cpp adapters with native endpoint discovery tested separately from the static pi-ai catalog
**Pointers:** `crates/lotta-providers/src/local/ollama.rs`, `local/lmstudio.rs`, `local/llama_cpp.rs`, `local/discovery.rs`; reference: `../letta-code/src/backend/dev/pi-ollama-provider.ts`, `pi-lmstudio-provider.ts`, `pi-llama-cpp-provider.ts`, `pi-local-endpoint-provider.ts`

## Steps

- [x] Implement the Ollama and Ollama Cloud endpoint adapters
- [x] Implement the LM Studio and llama.cpp endpoint adapters
- [x] Implement native endpoint model discovery per adapter, kept separate from the static pi-ai built-in catalog
- [x] Normalize each dialect's stream into `ProviderEvent` and its errors into `ProviderError`
- [x] Replay each dialect's `fixtures/providers/` corpus and diff against the expected trace
- [x] Handle an unreachable endpoint as `ProviderError::Unavailable` without failing process readiness

## Definition of done

- [x] All four local endpoint adapters replay their fixture corpus to the expected normalized trace
- [x] Native endpoint discovery is tested separately from the static pi-ai built-in catalog, and `models.json` is not used as the local-provider catalog
- [x] An unreachable endpoint yields `ProviderError::Unavailable` and does not fail process readiness
- [x] Each adapter passes the shared port-contract suite alongside the fake
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(local::)'` and sees four dialects replay their fixtures, discovery run against a fake endpoint, unavailability handled, and the shared contract suite pass
