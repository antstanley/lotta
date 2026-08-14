# Task 53 — Provider limits and adapter conformance gate

**Plan:** [plan.md](../plan.md) · **Certificate:** [53-provider_limits_and_conformance-certificate.md](53-provider_limits_and_conformance-certificate.md)

**Implements:** [06-model-providers.md §Limits](../../../06-model-providers.md#limits) · [06-model-providers.md §Conformance](../../../06-model-providers.md#conformance) · [06-model-providers.md §Context and compaction](../../../06-model-providers.md#context-and-compaction)
**Depends on:** 05, 12, 49, 50, 51
**Produces:** every `06-model-providers.md` limit enforced and the gate proving a native adapter and the compatibility host emit equivalent traces for the same fixture
**Pointers:** `crates/lotta-providers/src/limits.rs`, `tests/conformance/provider_equivalence.rs`, `crates/lotta-providers/src/context.rs`; reference: `../letta-code/src/backend/dev/context-window-overflow.ts`, `../letta-code/src/backend/local/local-context-estimate.ts`

## Steps

- [ ] Define the ten `06-model-providers.md` §Limits constants with the table's names and defaults
- [ ] Compute the effective context window as the minimum of configured server maximum, model catalog value, agent setting, and conversation override
- [ ] Make token estimation provider-aware where available and conservatively approximate otherwise
- [ ] Implement the equivalence gate: for each fixture, run the native adapter and the compatibility host and assert identical normalized traces
- [ ] Assert the eleven §Conformance contract dimensions are all exercised by the gate
- [ ] Report repeated context overflow as terminal with measured/estimated details and no message content

## Definition of done

- [ ] All ten §Limits constants exist with the table's names and defaults and have below/at/above tests
- [ ] The effective context window is the minimum of all four sources, and repeated overflow is terminal with measured/estimated details and no message content
- [ ] For every fixture, the native adapter and the compatibility host emit equivalent normalized traces
- [ ] All ten §Conformance contract dimensions are exercised by the gate, and the gate fails if a dimension has no case
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(limits::) + test(context::)'` and `cargo nextest run --test provider_equivalence` and sees the ten limits, the four-source context minimum, and native-versus-host trace equivalence pass
