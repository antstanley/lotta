# Done Certificate — Task 57: Cancellation and terminal behavior

**Task:** [57-cancellation_and_terminal_behavior.md](57-cancellation_and_terminal_behavior.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-18

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
  - *Evidence:* Exact selector passed 1/1. `six_exact_steps` records `ClaimAndCancel → NormalizeUnfinished → SuppressEffects → KillChildren → PersistTerminal → EmitAndRelease`; post-turn work starts only after the terminal receipt.
  - *Status:* ☑ SATISFIED

- **O2 — Unfinished local tool calls are normalized to interrupted results rather than left dangling**
  - *Claim:* A tool in flight at cancellation yields the interruption `ToolOutcome` and an appended interrupted result.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(cancel::normalizes_unfinished_tools)'` — expect PASS asserting the outcome variant and the appended result.
  - *Checks:* Resolve the outcome constructed for an interrupted tool — confirm it is `ToolOutcome::Interruption` (Task 08), not `ToolOutcome::Timeout`; §Cancellation and terminal behavior step 2 requires interruption specifically.
  - *Evidence:* Exact selector passed 1/1. The mixed local/controller test tracks provider-order Pending/Completed state, drains only unfinished calls once, appends/emits canonical `ToolOutcome::Interruption`, and suppresses late results.
  - *Status:* ☑ SATISFIED

- **O3 — Child processes are killed after `TURN_CANCEL_GRACE_MS` using the two-stage SIGTERM→SIGKILL grace, and no orphan survives**
  - *Claim:* Remaining children are terminated after 10,000 ms with a 2,000 ms kill grace, and the process group is empty afterwards.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(cancel::child_kill)'` — expect PASS with the fake clock advanced through both bounds and a process-group emptiness assertion.
  - *Evidence:* Exact runtime selector passed. The real production shell registration test uses the same manager identity as the executor, spawns a parent and descendant, proves exact scope/lease isolation, drives Task 38's 2,000 ms TERM→KILL escalation after the 10,000 ms cancellation grace, reaps both processes, and proves the empty-owner fast path. All three real shell/two-stage tests passed.
  - *Status:* ☑ SATISFIED

- **O4 — Exactly one `turn_finished(cancelled)` is emitted under concurrent cancellation attempts, and terminal state is persisted before it**
  - *Claim:* Three concurrent aborts produce one terminal event, emitted after persistence completes.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(cancel::exactly_once)'` — expect PASS asserting an event count of one and that the store write precedes the emission in the recorded order.
  - *Evidence:* Exact unit selector passed 1/1 with three barrier threads and one unique claim. `production_three_concurrent_abort_paths_terminal_release_then_pump_once` passed 100 internal repetitions through real service/controller/effects paths, recording actual `claim → persist → cancelled → release → pump`; every count is exactly one and generic completion is absent.
  - *Status:* ☑ SATISFIED

- **O5 — Post-turn reflection and memory push happen after terminal projection and cannot change that turn's outcome**
  - *Claim:* A reflection failure after the terminal event leaves the recorded outcome unchanged.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(cancel::reflection_after_terminal)'` — expect PASS with an injected reflection failure and an unchanged terminal record.
  - *Evidence:* Exact selector passed 1/1. The durable secure `runtime/post-turn-jobs.json` queue has revision-checked root-lock transactions, stable claims, stale recovery, retries, bounds, and capability registration. All 8 post-turn tests passed without leaks, including both job kinds, failure retry, concurrency, security, and terminal-outcome invariance.
  - *Status:* ☑ SATISFIED

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace docs, and `cargo deny check` passed. Runtime 284/284, tools 288/288, app-server 180/180, production 24/24, post-turn 8/8, and exact certificate selectors pass. Full-workspace failures remain confined to the recorded external pinned sibling SHA/CLI environment drift. Task 57 source uses units-last constants and stays within the 70-line/100-column task limits.
  - *Status:* ☑ SATISFIED

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(cancel::)'` and sees the six-step order, interrupted-result normalization, two-stage child kill, exactly-once terminal emission, and post-terminal reflection pass**
  - *Claim:* The cancellation module passes with no orphan process.
  - *Evidence to collect:* Run the filter and confirm zero failures and the harness's process-leak assertion is clean.
  - *Evidence:* Exact reviewer filter passed 5/5 with nonzero step-order, normalization, child-kill, exactly-once, and reflection cases and no LEAK status. Independent reviewer task_137 returned `CORRECT / DONE`.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/lifecycle.rs` (Task 17) owns the `Cancelling` state; `lifecycle::stale_lease_suppression` passed 4/4: ☑ PRESERVED
- `crates/lotta-tools/src/builtin/shell/` (Task 38) owns the two-stage kill; both `builtin::shell::two_stage_kill` tests and the production shared-manager cancellation test passed 3/3: ☑ PRESERVED

## Residue

`03-runtime-and-turns.md` §Assumptions leaves whether in-flight provider streams must resume from run IDs open; this task only guarantees a clean terminal state.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O7 obligations are discharged by exact selectors and production evidence. One cancellation claimant owns the six ordered stages, interruption normalization, real scoped child escalation/reaping, durable terminal emission and release, queue pumping, and post-terminal durable jobs. Task 17 and Task 38 regressions are preserved; only the recorded external pinned-checkout drift remains outside this task.
