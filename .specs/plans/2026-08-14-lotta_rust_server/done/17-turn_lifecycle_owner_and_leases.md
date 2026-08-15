# Task 17 — Turn lifecycle owner and lease-guarded effects

**Plan:** [plan.md](../plan.md) · **Certificate:** [17-turn_lifecycle_owner_and_leases-certificate.md](17-turn_lifecycle_owner_and_leases-certificate.md)

**Implements:** [03-runtime-and-turns.md §Lifecycle owner](../../../03-runtime-and-turns.md#lifecycle-owner) · [01-domain-model.md §Turn and lease](../../../01-domain-model.md#turn-and-lease)
**Depends on:** 16
**Produces:** one lifecycle owner per scope that issues leases and suppresses every effect from a stale lease at each awaited boundary
**Pointers:** `crates/lotta-runtime/src/lifecycle.rs`, `src/lease.rs`; reference: `../letta-code/src/websocket/listener/turn-lifecycle.ts`, `../letta-code/src/websocket/listener/send-lease.test.ts`, `../letta-code/src/websocket/listener/lifecycle.ts`

## Steps

- [x] Make the lifecycle owner the only writer of `TurnState`, with UI projections derived rather than stored
- [x] Issue an unforgeable generation lease on every transition out of `Idle`
- [x] Implement the four post-await checks of `03-runtime-and-turns.md` §Lifecycle owner: listener active, runtime present, lease current, cancellation policy permits
- [x] Suppress persistence writes, tool/protocol/channel emissions, and lease release when the captured lease is stale
- [x] Fail impossible lifecycle states as invariant violations with diagnostics rather than repairing them
- [x] Add tests proving a replacement turn is never released or contaminated by its predecessor

## Definition of done

- [x] The lifecycle owner is the only writer of `TurnState`, and every transition out of `Idle` mints a new lease generation
- [x] Every awaited boundary re-checks all four conditions, and a stale lease writes nothing, emits nothing, and releases nothing
- [x] Impossible lifecycle states fail as invariant violations with diagnostics instead of being repaired
- [x] UI projections (`is_processing`, `loop_status`, active run IDs, last stop reason) are derived from `TurnState` and cannot contradict it
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(lifecycle::)'` and sees lease minting, the four-part stale-lease suppression, the invariant-violation case, and derived projections pass
