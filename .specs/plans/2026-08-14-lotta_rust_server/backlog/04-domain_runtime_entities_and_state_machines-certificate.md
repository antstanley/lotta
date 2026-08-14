# Done Certificate — Task 04: Domain runtime entities and state machines

**Task:** [04-domain_runtime_entities_and_state_machines.md](04-domain_runtime_entities_and_state_machines.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

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
  - *Evidence to collect:* Read `crates/lotta-domain/src/runtime/turn_state.rs` and count the variants — expect four. Run `cargo nextest run -p lotta-domain -E 'test(runtime::turn_state)'` — expect the five named transition cases above to pass. Serialize each variant and confirm the discriminant strings are `idle`, `command`, `active`, `cancelling`, matching `$defs.ConversationRuntimeSnapshot.turn_state`.
  - *Checks:* Resolve the `Command` variant's consumer: confirm the device/slash/mod command path in `01-domain-model.md` §Turn lifecycle has a state to occupy. Flag `UNRESOLVED` if no test constructs `TurnState::Command`.
  - *Status:* ☐ unverified

- **O2 — A pending approval leaves the state `Active` rather than introducing a terminal or parallel state, and UI projections derive from `TurnState` alone**
  - *Claim:* There is no field outside `TurnState` that can report activity, and an approval-pending runtime reports `Active`.
  - *Evidence to collect:* Grep `crates/lotta-domain/src/runtime/` for `bool` fields named `is_processing`, `busy`, `running`, or `active` — expect zero. Run `cargo nextest run -p lotta-domain -E 'test(runtime::approval_keeps_active)'` — expect PASS.
  - *Status:* ☐ unverified

- **O3 — `InputDisposition` returns the prior disposition for a repeated `client_message_id` instead of executing twice**
  - *Claim:* Submitting the same `client_message_id` twice yields the first call's disposition on the second call and does not produce a second admission.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(runtime::duplicate_client_message_id)'` — expect PASS. Trace: `admit(cm1)` → `Started` → `admit(cm1)` → `Started` (same value, admission count still 1).
  - *Status:* ☐ unverified

- **O4 — `QueueItem` models both wire removal dispositions and both internal drop reasons with the baseline spellings**
  - *Claim:* The wire disposition enum serializes to `dequeued` and `cancelled`; the internal drop-reason enum serializes to `buffer_limit` and `stale_generation`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-domain -E 'test(runtime::queue_item_wire_names)'` — expect PASS. Compare the four literals against `../letta-code/src/types/protocol.ts:478` and `:492`, where `QueueItemDroppedReason = "buffer_limit" | "stale_generation"` is defined.
  - *Checks:* Resolve the drop-reason type used by the queue snapshot encoder — confirm it is the internal reason enum, not the wire disposition enum; `01-domain-model.md` §Queue item keeps them separate.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(runtime::)'` and sees the four-state machine, approval-stays-active, duplicate-disposition, and queue wire-name cases pass**
  - *Claim:* The `runtime::` domain tests pass and include a case for each of the three state machines in `01-domain-model.md` §State machines.
  - *Evidence to collect:* Run the filter and confirm passing cases named for the turn lifecycle, input disposition, and conversation archival machines, with zero failures.
  - *Status:* ☐ unverified

## Regression check

- `Agent`, `Conversation`, and runtime scalar types from Tasks 02–03 are consumed by the state transitions; run `cargo nextest run -p lotta-domain -E 'test(entities::) | test(state::)'` and confirm both suites pass : ☐ (PRESERVED / REGRESSION)

## Residue

Lease issuance and staleness enforcement are Task 17; this task owns only the token type and the state machine shape. Queue admission policy is Task 18.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
