# Done Certificate — Task 56: Approvals, resolution validation, and recovery

**Task:** [56-approvals.md](56-approvals.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-18

> Verification protocol for Task 56. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 56) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** approval requests stored before emission, resolution validated against the original schema, and a 24-hour interruption that never becomes an implicit denial.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not auto-deny on timeout: an implicit denial silently changes conversation content in a persisted transcript, which `03-runtime-and-turns.md` §Assumptions *Approval timeout* forbids.

## Obligations

- **O1 — The request is stored before `control_request` is emitted, and resolution validates request ID, tool call ID, lease generation, and edited input against the original schema**
  - *Claim:* A crash between store and emit leaves a recoverable request; a resolution failing any of the four checks is rejected.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(approval::request) + test(approval::resolution_validation)'` — expect `stored_before_emit` and four rejection cases. Read the edited-input case and confirm it validates against the original tool schema, not the edited payload's own shape.
  - *Evidence:* `cargo nextest run -p lotta-runtime -E 'test(approval::)'` passed 26/26, including `request::{stored_before_emit,crash_after_journal_before_ws_replays}` and all five `resolution_validation` rejection cases. `approval/resolve.rs` validates the stored `original_schema` before CAS mutation.
  - *Status:* ☑ SATISFIED

- **O2 — An approval timeout at `APPROVAL_WAIT_MS_MAX` interrupts the turn and replays an explicit expired approval state; it never appends a denial**
  - *Claim:* After 86,400,000 ms the turn reaches a recoverable interrupted terminal state and no denied tool result is appended.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(approval::timeout_interrupts)'` — expect PASS asserting the terminal state is the interrupted one and that the transcript contains no denial entry. The constant is `APPROVAL_WAIT_MS_MAX` from `03-runtime-and-turns.md` §Runtime bounds.
  - *Checks:* Resolve the terminal-state constructor used on timeout — confirm it is the interrupted/expired variant, not the denial path used by an explicit user deny. `03-runtime-and-turns.md` §Assumptions *Approval timeout* requires a recoverable terminal state rather than an implicit denial.
  - *Evidence:* All three `timeout_interrupts` cases passed. `ApprovalBrokerAdapter::await_resolution` uses `APPROVAL_WAIT_MS_MAX = 86_400_000`, persists `Expired`, returns typed timeout, and does not construct or append `UserDenied`.
  - *Status:* ☑ SATISFIED

- **O3 — Allow executes the call exactly once; deny appends a structured denied result and resumes the model where the protocol requires continuation; abort moves to `Cancelling` and a late response cannot clear it**
  - *Claim:* The three resolutions behave as §Approvals specifies, and a late allow after abort is ignored.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(approval::resolutions)'` — expect `allow_executes_once`, `deny_appends_and_resumes`, `abort_moves_to_cancelling`, and `late_response_cannot_clear_cancellation`.
  - *Evidence:* Resolution and branch selectors passed, including concurrent allow-once, deny/resume, late-response rejection, and `aborted_approval_uses_user_cancellation_terminal`. Production tests `production_approval_allow_executes_once` and `production_approval_response_resolves_while_turn_lock_is_held` passed; cancellation traverses `Active → Cancelling → Idle` through `cancel_turn`.
  - *Status:* ☑ SATISFIED

- **O4 — Reconnect `sync` replays unresolved requests, and restart recovery never guesses that a tool ran**
  - *Claim:* After reconnect the client receives the pending request again; after restart the runtime either reconstructs a safe continuation or emits explicit denial/interruption.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(approval::recovery)'` — expect `reconnect_replays_unresolved` and `restart_does_not_assume_execution`, the second asserting the recovered state is explicit rather than inferred.
  - *Evidence:* All six recovery tests passed. Production `sync` replays durable `Pending`, `Expired`, and `Interrupted` requests while a turn holds the registry lock; restart converts unreconstructible `Pending`/`Executing` to explicit `Interrupted` and never re-executes.
  - *Status:* ☑ SATISFIED

- **O5 — `PENDING_APPROVALS_PER_RUNTIME_MAX` fails the turn as an invariant violation at the limit**
  - *Claim:* The 129th pending approval for a runtime is an invariant failure with diagnostics.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(approval::bounds)'` — expect below/at/above cases naming the `01-domain-model.md` §Resource bounds constant.
  - *Evidence:* All four bounds tests passed. The real `LocalStore` journal test `pending_bound_rejects_the_129th_record` also passed; the 129th request returns named `LimitExceeded` before emission.
  - *Status:* ☑ SATISFIED

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace docs, and `cargo deny check` passed. Full workspace nextest ran 1,814 tests: 1,739 passed; all 75 failures are the known external pinned sibling SHA/CLI drift or unchanged planning-asset failures reproduced at the Task 55 parent. Task-local runtime, protocol, app-server, store, tools pipeline, production, and exact certificate selectors pass. New approval/production functions are ≤70 lines and named limits use units-last constants. Real journal security, 8 MiB, per-scope terminal-128, pending-128, and 400-concurrent-writer tests pass.
  - *Status:* ☑ SATISFIED

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(approval::)'` and sees store-before-emit, four resolution validations, interruption-not-denial on timeout, the three resolutions, and both recovery paths pass**
  - *Claim:* The approval module passes with the timeout case asserting interruption.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `timeout_interrupts` case.
  - *Evidence:* Exact reviewer selector passed 26/26 with zero failures and includes store-before-emit, five resolution validations, timeout interruption, resolutions, waiter-race, bounds, and recovery cases.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/branches.rs` (Task 55) emits the approval request; `turn::branches` passed 7/7, including the real approval storage and cancellation terminal paths: ☑ PRESERVED

## Residue

Whether the 24-hour bound should be configurable below a larger hard maximum is recorded as an open question in `03-runtime-and-turns.md` §Assumptions and in `plan.md`.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O7 obligations are discharged by exact runtime, production, protocol, and real LocalStore evidence. Approval responses are durable-before-emit, correlated to the live lease, resolved after ACK, executed or denied exactly once, cancelled through the canonical terminal path, and recovered without inferred execution. Task 55 branch behavior is preserved. The only full-workspace failures are the recorded external pinned-checkout/CLI drift and unchanged parent planning-asset failures.
