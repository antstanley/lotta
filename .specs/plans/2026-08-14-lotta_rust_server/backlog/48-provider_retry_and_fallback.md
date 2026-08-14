# Task 48 — Runtime-owned retry policy and explicit fallback

**Plan:** [plan.md](../plan.md) · **Certificate:** [48-provider_retry_and_fallback-certificate.md](48-provider_retry_and_fallback-certificate.md)

**Implements:** [06-model-providers.md §Retry and fallback](../../../06-model-providers.md#retry-and-fallback)
**Depends on:** 07, 09
**Produces:** a deterministic, jitter-free retry policy owned by the runtime, plus explicit transport fallback that never mutates the persisted model
**Pointers:** `crates/lotta-runtime/src/retry/policy.rs`, `retry/fallback.rs`; reference: `../letta-code/src/backend/dev/provider-turn-executor.ts`, `../letta-code/src/backend/dev/local-provider-errors.ts`, `../letta-code/src/websocket/listener/provider-fallback.ts`

## Steps

- [ ] Declare the policy's attempts, retry-after handling, capped exponential backoff for transient/busy errors, linear delay for empty responses, and total deadline
- [ ] Emit no jitter anywhere in the provider-turn retry path, matching the pinned baseline
- [ ] Cap backoff at `PROVIDER_BACKOFF_MS_MAX` and attempts at `PROVIDER_RETRIES_MAX`
- [ ] Make authentication, invalid request, unsupported model, and schema errors non-retryable
- [ ] Implement explicit transport/provider fallback that emits a retry event naming source and destination transport and never modifies the persisted model
- [ ] Keep retry ownership in the runtime so adapters cannot nest their own retries

## Definition of done

- [ ] The retry path contains no jitter and produces a byte-identical delay sequence across runs under a fake clock
- [ ] The policy implements all four delay behaviors: retry-after handling, capped exponential for transient/busy, linear for empty responses, and a total deadline
- [ ] `PROVIDER_RETRIES_MAX` and `PROVIDER_BACKOFF_MS_MAX` carry the spec defaults, and authentication, invalid-request, unsupported-model, and schema errors do not retry
- [ ] Fallback is explicit, emits a retry event naming source and destination transport, and never modifies the persisted model
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(retry::)'` and sees deterministic jitter-free delays, all four delay shapes, both bounds, non-retryable kinds, and model-preserving fallback pass
