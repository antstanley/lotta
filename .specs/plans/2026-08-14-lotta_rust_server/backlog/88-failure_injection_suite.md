# Task 88 — Failure-injection suite

**Plan:** [plan.md](../plan.md) · **Certificate:** [88-failure_injection_suite-certificate.md](88-failure_injection_suite-certificate.md)

**Implements:** [00-overview.md §Implementation acceptance](../../../00-overview.md#implementation-acceptance) · [04-persistence-and-memfs.md §Durability and bounds](../../../04-persistence-and-memfs.md#durability-and-bounds) · [03-runtime-and-turns.md §Runtime bounds](../../../03-runtime-and-turns.md#runtime-bounds)
**Depends on:** 22, 42, 45, 46, 57, 73, 85
**Produces:** proof of baseline queue semantics, Lotta atomic-write and conflict hardening, cancellation, stale-lease suppression, reconnect recovery, and sidecar crash handling
**Pointers:** `tests/conformance/failure_injection.rs`, `tests/conformance/inject/`; reference: `../letta-code/src/websocket/listener/queue-update-transitions.test.ts`, `send-lease.test.ts`, `recovery-lease.test.ts`, `turn-cleanup.test.ts`

## Steps

- [ ] Inject disk-full and partial-write failures and assert atomic-write recovery and distinct typed errors
- [ ] Inject a mid-turn client disconnect and assert clean completion or cancellation with exactly one terminal event
- [ ] Inject provider timeouts and assert the deterministic retry sequence then a typed stop
- [ ] Race a cancellation against a streaming provider and assert exactly one `turn_finished(cancelled)`
- [ ] Drive queue soft/hard tiers and stale-lease suppression against the recorded reference traces
- [ ] Crash the mod host, the provider host, and a subagent and assert cleanup, bounded restart, and a typed error with no host state corruption

## Definition of done

- [ ] Disk-full and partial-write injections produce distinct typed errors and leave the store recoverable
- [ ] The queue soft and hard tiers, stale-lease suppression, and reconnect recovery match the recorded reference traces
- [ ] A cancellation racing a streaming provider yields exactly one `turn_finished(cancelled)` under repeated trials
- [ ] Crashing the mod host, provider host, or a subagent produces cleanup, a bounded restart, and a typed error without host state corruption
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run --test failure_injection` and sees storage, reliability-trace, cancellation-race, and sidecar-crash cases pass against the assembled binary
