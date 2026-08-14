# Task 93 — Reliability trace conformance

**Plan:** [plan.md](../plan.md) · **Certificate:** [93-reliability_trace_conformance-certificate.md](93-reliability_trace_conformance-certificate.md)

**Implements:** [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition) · [02-app-server-api.md §Event envelopes and ordering](../../../02-app-server-api.md#event-envelopes-and-ordering)
**Depends on:** 13, 73, 85
**Produces:** the seven reliability traces replayed against the assembled binary with matching queue, abort, disconnect, stale-lease, retry, idempotency, and crash-recovery behavior
**Pointers:** `tests/conformance/reliability_traces.rs`; reference: `fixtures/reference-traces/`, `../letta-code/src/websocket/listener/AGENTS.md`

## Steps

- [ ] Replay each `fixtures/reference-traces/` capture against the assembled binary
- [ ] Compare observed and recorded traces under the semantic-equivalence rules
- [ ] Assert the six ordering invariants over each observed trace
- [ ] Assert deterministic retry delay sequences with a controlled clock
- [ ] Assert idempotent admission for repeated `client_message_id` values
- [ ] Report the first divergence with its event index and both events

## Definition of done

- [ ] All seven reliability traces replay against the assembled binary and match under the semantic comparator
- [ ] The six ordering invariants hold over every observed trace
- [ ] Retry delay sequences are deterministic across runs with a controlled clock
- [ ] A repeated `client_message_id` returns the prior disposition and does not execute twice
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run --test reliability_traces` against the built binary and sees all seven recorded traces reproduce, including deterministic retry timing
