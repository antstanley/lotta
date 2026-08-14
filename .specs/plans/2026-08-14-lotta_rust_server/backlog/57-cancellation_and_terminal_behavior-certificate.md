# Done Certificate — Task 57: Cancellation and terminal behavior

**Task:** [57-cancellation_and_terminal_behavior.md](57-cancellation_and_terminal_behavior.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 57. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 57) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the six-step cancellation sequence with interrupted-result normalization, two-stage child kill, and exactly one `turn_finished(cancelled)`.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not emit more than one terminal event: `02-app-server-api.md` §Event envelopes and ordering invariant 4 requires exactly once per admitted turn.

## Obligations

- **O1 — The six cancellation steps execute in order, and a recorded stage log matches §Cancellation and terminal behavior**
  - *Claim:* Cancelling a turn produces the six documented steps in sequence.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(cancel::step_order)'` — expect PASS comparing the recorded log against the six steps.
  - *Status:* ☐ unverified

- **O2 — Unfinished local tool calls are normalized to interrupted results rather than left dangling**
  - *Claim:* A tool in flight at cancellation yields the interruption `ToolOutcome` and an appended interrupted result.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(cancel::normalizes_unfinished_tools)'` — expect PASS asserting the outcome variant and the appended result.
  - *Checks:* Resolve the outcome constructed for an interrupted tool — confirm it is `ToolOutcome::Interruption` (Task 08), not `ToolOutcome::Timeout`; §Cancellation and terminal behavior step 2 requires interruption specifically.
  - *Status:* ☐ unverified

- **O3 — Child processes are killed after `TURN_CANCEL_GRACE_MS` using the two-stage SIGTERM→SIGKILL grace, and no orphan survives**
  - *Claim:* Remaining children are terminated after 10,000 ms with a 2,000 ms kill grace, and the process group is empty afterwards.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(cancel::child_kill)'` — expect PASS with the fake clock advanced through both bounds and a process-group emptiness assertion.
  - *Status:* ☐ unverified

- **O4 — Exactly one `turn_finished(cancelled)` is emitted under concurrent cancellation attempts, and terminal state is persisted before it**
  - *Claim:* Three concurrent aborts produce one terminal event, emitted after persistence completes.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(cancel::exactly_once)'` — expect PASS asserting an event count of one and that the store write precedes the emission in the recorded order.
  - *Status:* ☐ unverified

- **O5 — Post-turn reflection and memory push happen after terminal projection and cannot change that turn's outcome**
  - *Claim:* A reflection failure after the terminal event leaves the recorded outcome unchanged.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(cancel::reflection_after_terminal)'` — expect PASS with an injected reflection failure and an unchanged terminal record.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(cancel::)'` and sees the six-step order, interrupted-result normalization, two-stage child kill, exactly-once terminal emission, and post-terminal reflection pass**
  - *Claim:* The cancellation module passes with no orphan process.
  - *Evidence to collect:* Run the filter and confirm zero failures and the harness's process-leak assertion is clean.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/lifecycle.rs` (Task 17) owns the `Cancelling` state; confirm `lifecycle::stale_lease_suppression` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-tools/src/builtin/shell/` (Task 38) owns the two-stage kill; confirm `builtin::shell::two_stage_kill` still passes when driven from cancellation : ☐ (PRESERVED / REGRESSION)

## Residue

`03-runtime-and-turns.md` §Assumptions leaves whether in-flight provider streams must resume from run IDs open; this task only guarantees a clean terminal state.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
