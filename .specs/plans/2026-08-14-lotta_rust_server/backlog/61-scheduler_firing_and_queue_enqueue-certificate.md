# Done Certificate — Task 61: Scheduler firing, missed handling, and queue enqueue

**Task:** [61-scheduler_firing_and_queue_enqueue.md](61-scheduler_firing_and_queue_enqueue.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 61. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 61) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a lease-aware scheduler that enqueues `cron_prompt` through the conversation queue and never bypasses it.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not bypass the conversation queue: `01-domain-model.md` §Schedule and `03-runtime-and-turns.md` §Input and queue flow both require cron prompts to enter through the queue.

## Obligations

- **O1 — A due schedule enqueues a `cron_prompt` through the conversation queue rather than starting a turn directly**
  - *Claim:* Firing produces a queue admission with kind `cron_prompt` and no direct turn start.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(scheduler::enqueues_through_queue)'` — expect PASS asserting the admission chain was called and the lifecycle owner was not invoked directly. `01-domain-model.md` §Schedule states schedule execution enqueues a `cron_prompt` and does not bypass the conversation queue.
  - *Checks:* Resolve the enqueue call — confirm it is the Task 18 admission entry point, not an internal queue push that skips duplicate detection and bounds.
  - *Status:* ☐ unverified

- **O2 — A schedule whose window passed without firing is marked `missed` with its miss counter incremented**
  - *Claim:* Advancing past the window without a tick marks the schedule missed exactly once.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(scheduler::missed)'` — expect PASS asserting the status and a counter delta of one.
  - *Status:* ☐ unverified

- **O3 — Pending jitter-delayed timers are cancelled on stop or lease loss, leaving no late fire**
  - *Claim:* Stopping the scheduler or losing the lease prevents a pending jittered fire from executing.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(scheduler::cancels_pending_timers)'` — expect two cases (stop and lease loss), each asserting zero admissions after the fake clock advances past the jittered instant.
  - *Status:* ☐ unverified

- **O4 — A one-shot schedule retires after firing while a recurring schedule stays active, and each fire, miss, and failure is written to the run log**
  - *Claim:* Status transitions and run-log entries match the outcome of each evaluation.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(scheduler::outcomes)'` — expect `one_shot_retires`, `recurring_stays_active`, and a run-log entry assertion per outcome kind.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(scheduler::)'` and sees queue-routed firing, missed marking, timer cancellation on stop and lease loss, and one-shot retirement pass**
  - *Claim:* The scheduler module passes with enqueue-through-queue asserted.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `enqueues_through_queue` case.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/admission.rs` (Task 18) admits these prompts; confirm `admission::sources` still passes with a live scheduler : ☐ (PRESERVED / REGRESSION)

## Residue

Schedule management commands over WebSocket are Task 68; this task owns evaluation and firing only.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
