# Task 13 — Reference trace corpus extraction

**Plan:** [plan.md](../plan.md) · **Certificate:** [13-fixtures_reference_trace_corpus-certificate.md](13-fixtures_reference_trace_corpus-certificate.md)

**Implements:** [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition) · [architecture-principles.md §Compatibility architecture](../../../architecture-principles.md#compatibility-architecture) · [02-app-server-api.md §Event envelopes and ordering](../../../02-app-server-api.md#event-envelopes-and-ordering)
**Depends on:** 09, 10
**Produces:** a `fixtures/reference-traces/` corpus of baseline command/event traces for the queue, abort, disconnect, stale-lease, retry, idempotency, and crash-recovery reliability surfaces
**Pointers:** `fixtures/reference-traces/`, `tools/capture-reference-traces.mjs`, `crates/lotta-testkit/src/fixtures/traces.rs`; reference: `../letta-code/src/app-server-client.ts`, `../letta-code/src/websocket/listener/AGENTS.md`, `../letta-code/src/websocket/listener/queue-update-transitions.test.ts`, `../letta-code/src/websocket/listener/send-lease.test.ts`

## Steps

- [ ] Drive the pinned TypeScript App Server with `app-server-client` and record command/event traces for each of the seven reliability surfaces named in `00-overview.md` §Compatibility definition
- [ ] Normalize each trace so semantically-equivalent fields (generated UUIDs, timestamps, `idempotency_key`) are matched by rule rather than by literal equality
- [ ] Record the six ordering invariants of `02-app-server-api.md` §Event envelopes and ordering as machine-checkable assertions over a trace
- [ ] Capture a happy-path `runtime_start` → `input` → `stream_delta` → `turn_finished` trace for the vertical slice in Task 21
- [ ] Sanitize prompts, message bodies, and tool inputs out of every trace
- [ ] Expose a comparator that reports the first ordering-invariant violation with the offending event pair

## Definition of done

- [ ] All seven reliability surfaces of `00-overview.md` §Compatibility definition have a recorded trace
- [ ] The six `02-app-server-api.md` §Event envelopes and ordering invariants are executable assertions over a trace
- [ ] Trace comparison matches semantically-equivalent fields by rule, so generated UUIDs, timestamps, and `idempotency_key` values do not cause false failures
- [ ] A happy-path turn trace exists that Task 21 can replay end to end
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-testkit -E 'test(fixtures::traces::)'` and sees the seven reliability traces, six ordering invariants, semantic-equivalence rules, and the slice trace pass
