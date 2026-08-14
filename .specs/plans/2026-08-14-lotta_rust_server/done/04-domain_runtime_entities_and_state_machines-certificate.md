# Done Certificate — Task 04: Domain runtime entities and state machines

**Task:** [04-domain_runtime_entities_and_state_machines.md](04-domain_runtime_entities_and_state_machines.md) · **Plan:** [plan.md](../plan.md)
**State:** Final validated 2026-08-14

> Verification protocol for Task 04. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 04) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the four-state turn machine, lease tokens, and runtime projections that make contradictory activity states unrepresentable.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not reduce `TurnState` below four variants: `canonical-types.schema.json` `$defs.ConversationRuntimeSnapshot.turn_state` enumerates `idle`, `command`, `active`, `cancelling`, and a three-variant enum makes the snapshot unrepresentable.

## Obligations

- **O1 — `TurnState` has exactly the four variants `Idle`, `Command`, `Active`, `Cancelling`, and every legal and illegal transition of the `01-domain-model.md` §Turn lifecycle diagram is covered by a test**
  - *Claim:* The enum is four-variant and exhaustive; `idle → command → idle`, `idle → active → idle`, and `active → cancelling → idle` succeed while `idle → cancelling` and `command → active` fail.
  - *Evidence to collect:* Read `crates/lotta-domain/src/runtime/turn_state.rs` and count the variants — expect four. Run `cargo nextest run -p lotta-domain -E 'test(runtime::turn_state)'` — expect an exhaustive transition-matrix case plus the five named examples above to pass. Confirm the matrix covers every ordered pair of distinct states, permitting only the lifecycle-diagram edges. Serialize each projected state kind and confirm the discriminant strings are `idle`, `command`, `active`, `cancelling`, matching `$defs.ConversationRuntimeSnapshot.turn_state`.
  - *Checks:* Resolve the `Command` variant's consumer: confirm the device/slash/mod command path in `01-domain-model.md` §Turn lifecycle has a state to occupy. Flag `UNRESOLVED` if no test constructs `TurnState::Command`.
  - *Status:* SATISFIED — final independent inspection found exactly four private `TurnState` variants and no safe bypass: callers receive only `TurnStateView`/`TurnStateKind` and lifecycle-owner methods, while `TurnLease` construction and fields remain private. The exact selector passed 6/6; the matrix covers all 12 ordered distinct-state pairs and only the six legal edges. Current/stale/cross-owner checks compare the complete owner+generation token before every lease-bearing mutation, and failed checks preserve state, generation, projections, and stop reason. The dedicated exhaustion test passed: generation `u64::MAX - 1` mints and settles the final lease at `u64::MAX`, then both command and turn starts return typed `TurnLeaseExhaustedError` atomically; generation hooks are `#[cfg(test)] pub(super)` only.

- **O2 — A pending approval leaves the state `Active` rather than introducing a terminal or parallel state, and UI projections derive from `TurnState` alone**
  - *Claim:* There is no field outside `TurnState` that can report activity, and an approval-pending runtime reports `Active`.
  - *Evidence to collect:* Grep `crates/lotta-domain/src/runtime/` for `bool` fields named `is_processing`, `busy`, `running`, or `active` — expect zero. Run `cargo nextest run -p lotta-domain -E 'test(runtime::approval_keeps_active)'` — expect PASS.
  - *Status:* SATISFIED — final exact approval selector passed 1/1; no parallel activity boolean exists. `is_processing`, loop status, and active run IDs derive from private `TurnState`. Pending approval remains `Active`. Opaque validated `StopReason` owner state is cleared only on successful turn begin and set only after successful current-lease terminal settlement; illegal, stale, cross-owner, and exhausted starts leave it unchanged.

- **O3 — `InputDisposition` returns the prior disposition for a repeated `client_message_id` instead of executing twice**
  - *Claim:* Submitting the same `client_message_id` twice yields the first call's disposition on the second call and does not produce a second admission.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(runtime::duplicate_client_message_id)'` — expect PASS. Trace: `admit(cm1)` → `Started` → `admit(cm1)` → `Started` (same value, admission count still 1).
  - *Status:* SATISFIED — final exact selector passed 1/1; `cm-1` replayed `Started` after a proposed `Rejected`, admission count remained 1, and history remains bounded to 300 entries.

- **O4 — `QueueItem` models both wire removal dispositions and both internal drop reasons with the baseline spellings**
  - *Claim:* The wire disposition enum serializes to `dequeued` and `cancelled`; the internal drop-reason enum serializes to `buffer_limit` and `stale_generation`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(runtime::queue_item_wire_names)'` — expect PASS. Compare the four literals against `../letta-code/src/types/protocol.ts:478` and `:492`, where `QueueItemDroppedReason = "buffer_limit" | "stale_generation"` is defined.
  - *Checks:* Resolve the drop-reason type used by the queue snapshot encoder — confirm it is the internal reason enum, not the wire disposition enum; `01-domain-model.md` §Queue item keeps them separate.
  - *Status:* SATISFIED — final exact selector passed 1/1; wire names are `dequeued`/`cancelled`, internal names are `buffer_limit`/`stale_generation`, and the enums remain distinct types.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* SATISFIED — final gates all passed: `cargo fmt --all --check`; workspace/all-target/all-feature Clippy with `-D warnings`; full domain nextest 65/65; workspace nextest 66/66; `cargo deny check`; and workspace rustdoc with `-D warnings`. The final diff contains only the six Task-04 domain files, adds no dependency, and has no premature later-task implementation. Named bounds, 100-column and 70-line function limits, public docs, direct-clock/unsafe audits, and forbidden production `expect`/`unwrap`/panic/todo/unimplemented audits are clean. Generation exhaustion and empty turn IDs both return typed propagated errors atomically; deterministic generation-boundary support is confined to `#[cfg(test)] pub(super)` methods.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(runtime::)'` and sees the four-state machine, approval-stays-active, duplicate-disposition, and queue wire-name cases pass**
  - *Claim:* The `runtime::` domain tests pass and include a case for each of the three state machines in `01-domain-model.md` §State machines.
  - *Evidence to collect:* Run the filter and confirm passing cases named for the turn lifecycle, input disposition, and conversation archival machines, with zero failures.
  - *Status:* SATISFIED — final exact `test(runtime::)` selector passed 18/18, including lifecycle matrix/projections, generation exhaustion, empty-turn-ID atomic rejection, stale/cross-owner/atomic preservation, stop reason, approval, duplicate disposition, queue names, archival, and all five canonical typed-shape/schema checks. Minimal and complete samples validate against the live schema and typed round trips; required/null/extras and queue-bound semantics are covered.

## Regression check

- Final full domain tests passed 64/64 and workspace tests passed 65/65. The direct Task 02/03 selector over `ids::`, `scalars::`, and `scope::` passed 14/14: **PRESERVED**.

## Residue

No premature Task 17/18 implementation, unnecessary abstraction, or dependency was found. Task 04 appropriately owns opaque lease identity and lifecycle validation needed to prevent bypasses; queue policy beyond bounded duplicate history remains deferred. No correctness or completeness residue remains.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: DONE
CONFIDENCE: high
SUMMARY: Final independent combined gate rates correctness CORRECT and completeness DONE. O1–O6 are SATISFIED and regression is PRESERVED. The prior state/lease bypass, missing stop-reason ownership, and generation-exhaustion production `expect` findings are fixed without new gaps: the owner/state machine and full lease token are opaque, every mutation is lease/state checked and atomic on failure, final-generation behavior is typed and tested for both starts without production hooks, all canonical runtime shapes and state machines conform, and every required test, lint, deny, rustdoc, diff/API/bound/forbidden/hard-limit audit passes.
