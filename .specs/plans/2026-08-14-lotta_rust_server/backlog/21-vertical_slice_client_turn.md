# Task 21 — Vertical slice: the baseline client drives a turn against fake ports

**Plan:** [plan.md](../plan.md) · **Certificate:** [21-vertical_slice_client_turn-certificate.md](21-vertical_slice_client_turn-certificate.md)

**Implements:** [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition) · [00-overview.md §System shape](../../../00-overview.md#system-shape) · [02-app-server-api.md §Core lifecycle](../../../02-app-server-api.md#core-lifecycle)
**Depends on:** 09, 13, 20
**Produces:** the unmodified baseline `app-server-client` completing `runtime_start` → `input` → `stream_delta` → `turn_finished` against the Rust server with fake ports, matching the recorded reference trace
**Pointers:** `tests/slice/client_turn.rs`, `tests/slice/harness/`, `fixtures/reference-traces/slice_happy_turn.json`; reference: `../letta-code/src/app-server-client.ts`, `../letta-code/src/types/app-server-protocol.ts`

## Steps

- [ ] Build a harness that starts the Rust listener on an ephemeral loopback port with a capability token and fake store, provider, and tool ports
- [ ] Drive it with the unmodified `@letta-ai/letta-code` `app-server-client` from the pinned checkout, with no client patches
- [ ] Complete the full `02-app-server-api.md` §Core lifecycle exchange and record the observed trace
- [ ] Compare the observed trace against `fixtures/reference-traces/slice_happy_turn.json` using the Task 13 semantic comparator
- [ ] Assert the six ordering invariants over the observed trace
- [ ] Wire the slice into CI so every later task is reviewed through a live client

## Definition of done

- [ ] The unmodified baseline `app-server-client` connects, starts a runtime, sends input, receives stream deltas, and observes `turn_finished` against the Rust server
- [ ] The observed trace matches `fixtures/reference-traces/slice_happy_turn.json` under the semantic comparator
- [ ] All six §Event envelopes and ordering invariants hold over the live trace, not only over synthetic events
- [ ] The slice runs in CI on every push and fails the build when it breaks
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run --test slice` and watches the baseline client complete a turn against the Rust binary, then confirms the captured trace matches the reference
