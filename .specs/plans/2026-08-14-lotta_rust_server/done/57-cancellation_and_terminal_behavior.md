# Task 57 — Cancellation and terminal behavior

**Plan:** [plan.md](../plan.md) · **Certificate:** [57-cancellation_and_terminal_behavior-certificate.md](57-cancellation_and_terminal_behavior-certificate.md)

**Implements:** [03-runtime-and-turns.md §Cancellation and terminal behavior](../../../03-runtime-and-turns.md#cancellation-and-terminal-behavior)
**Depends on:** 17, 38, 55
**Produces:** the six-step cancellation sequence with interrupted-result normalization, two-stage child kill, and exactly one `turn_finished(cancelled)`
**Pointers:** `crates/lotta-runtime/src/turn/cancel.rs`, `turn/terminal.rs`; reference: `../letta-code/src/websocket/listener/turn-terminal.ts`, `interrupts.ts`, `turn-cleanup.ts`, `turn-completion.ts`, `turn-reflection.test.ts`

## Steps

- [x] Transition `Active` to `Cancelling` and cancel provider and tool tokens
- [x] Normalize unfinished local tool calls to interrupted results
- [x] Stop accepting side effects from the lease except cancellation terminal events
- [x] Wait `TURN_CANCEL_GRACE_MS`, then kill remaining child processes in the runtime scope using the 2,000 ms SIGTERM→SIGKILL grace
- [x] Have the owner emit exactly one `turn_finished(cancelled)` and release the lease
- [x] Persist message and transcript state before announcing completion, and run post-turn reflection and memory push only after terminal projection

## Definition of done

- [x] The six cancellation steps execute in order, and a recorded stage log matches §Cancellation and terminal behavior
- [x] Unfinished local tool calls are normalized to interrupted results rather than left dangling
- [x] Child processes are killed after `TURN_CANCEL_GRACE_MS` using the two-stage SIGTERM→SIGKILL grace, and no orphan survives
- [x] Exactly one `turn_finished(cancelled)` is emitted under concurrent cancellation attempts, and terminal state is persisted before it
- [x] Post-turn reflection and memory push happen after terminal projection and cannot change that turn's outcome
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(cancel::)'` and sees the six-step order, interrupted-result normalization, two-stage child kill, exactly-once terminal emission, and post-terminal reflection pass
