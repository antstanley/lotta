# Task 59 — Runtime bounds and structured observability

**Plan:** [plan.md](../plan.md) · **Certificate:** [59-runtime_bounds_and_observability-certificate.md](59-runtime_bounds_and_observability-certificate.md)

**Implements:** [03-runtime-and-turns.md §Runtime bounds](../../../03-runtime-and-turns.md#runtime-bounds) · [03-runtime-and-turns.md §Observability](../../../03-runtime-and-turns.md#observability)
**Depends on:** 05, 55, 57, 58
**Produces:** the ten runtime bounds enforced with progress assertions, and structured events and metrics that exclude prompts, message bodies, tool inputs, and secrets
**Pointers:** `crates/lotta-runtime/src/bounds.rs`, `src/observe/events.rs`, `observe/metrics.rs`; reference: `../letta-code/src/websocket/listener/listener-constants.ts`, `../letta-code/src/websocket/listener/turn-status.ts`, `../letta-code/src/websocket/listener/protocol-logging.ts`

## Steps

- [ ] Define the ten `03-runtime-and-turns.md` §Runtime bounds constants with their exact names and defaults
- [ ] Assert measurable progress toward one of these bounds in every loop
- [ ] Emit structured events carrying runtime key, connection ID, turn/lease generation, run ID, provider, tool call ID, attempt, queue length, stop reason, and duration
- [ ] Exclude prompt text, message bodies, tool inputs, credentials, and secret-substituted commands by default
- [ ] Emit the ten metric families of §Observability: admissions, queue depth, active turns, cancellation latency, retries, compactions, provider latency, tool duration, stale-lease suppressions, and terminal outcomes
- [ ] Add below/at/above tests for each bound and a hardening-boundary fixture proving ordinary baseline clients are unaffected

## Definition of done

- [ ] All ten §Runtime bounds constants exist with the spec's names and defaults and have below/at/above tests
- [ ] Every loop asserts measurable progress toward one of the bounds
- [ ] Structured events carry the ten §Observability fields and exclude prompt text, message bodies, tool inputs, credentials, and secret-substituted commands
- [ ] All ten §Observability metric families are emitted and named
- [ ] Each Lotta-hardening bound has a boundary fixture proving ordinary baseline clients are unaffected
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(bounds::) + test(observe::)'` and sees ten bounds with boundary coverage, loop progress assertions, ten event fields with five exclusions, and ten metric families pass
