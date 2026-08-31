# Task 73 — Snapshot sync, reconnect, and restart recovery

**Plan:** [plan.md](../plan.md) · **Certificate:** [73-ws_sync_snapshot_recovery-certificate.md](73-ws_sync_snapshot_recovery-certificate.md)

**Implements:** [02-app-server-api.md §Core lifecycle](../../../02-app-server-api.md#core-lifecycle) · [02-app-server-api.md §Event envelopes and ordering](../../../02-app-server-api.md#event-envelopes-and-ordering) · [03-runtime-and-turns.md §Approvals](../../../03-runtime-and-turns.md#approvals)
**Depends on:** 18, 20, 56
**Produces:** `sync` replaying authoritative snapshots rather than an event-ID delta, with reconnect subscription and sequence recovery and restart approval recovery
**Pointers:** `crates/lotta-app-server/src/ws/sync.rs`, `ws/recovery.rs`; reference: `../letta-code/src/websocket/listener/recovery-sync.ts`, `recovery.ts`, `connection-state-sync.ts`, `recovery-lease.test.ts`

## Steps

- [x] Implement `sync` as a replay of authoritative snapshots — device status, loop status, queue, subagent state, and pending approvals — followed by `sync_response`
- [x] Treat all state updates as snapshots rather than diffs, except queue removals which carry explicit ordered transitions
- [x] Recover subscriptions and the next per-connection event sequence on reconnect
- [x] Replay unresolved approvals through the Task 56 recovery path
- [x] Repair a missing terminal tool event with the next authoritative loop snapshot
- [x] Prove `idempotency_key` is not used for replay deduplication

## Definition of done

- [x] `sync` replays authoritative snapshots and does not attempt a delta from a client-supplied last event ID
- [x] State updates are snapshots, not diffs, except queue removals which carry explicit ordered transitions
- [x] Reconnect recovers subscriptions and the next event sequence, and unresolved approvals are replayed
- [x] A missing terminal tool event is repaired by the next authoritative loop snapshot
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::sync::) + test(ws::recovery::)'` and sees snapshot replay with no event cursor, snapshot semantics, reconnect recovery, and tool-end repair pass
