# Done Certificate — Task 61: Scheduler firing, missed handling, and queue enqueue

**Task:** [61-scheduler_firing_and_queue_enqueue.md](61-scheduler_firing_and_queue_enqueue.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-21

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
  - *Evidence:* Exact selector passed 1/1. The fire path calls the real Task 18 admission entry (`ListenerRuntime::admit`, `AdmissionRoute::Ordinary`) with kind `cron_prompt`; duplicate detection and bounds are inherited (duplicate replay asserts `Duplicate(Queued)`), the lifecycle owner is never invoked (projection stays `Idle`), and `admit`'s Start branch requires kind `Message`, so a cron item can never start a turn directly.
  - *Status:* ☑ SATISFIED

- **O2 — A schedule whose window passed without firing is marked `missed` with its miss counter incremented**
  - *Claim:* Advancing past the window without a tick marks the schedule missed exactly once.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(scheduler::missed)'` — expect PASS asserting the status and a counter delta of one.
  - *Evidence:* Exact selector passed 1/1. Status becomes `Missed` with a miss-counter delta of exactly one, second tick changes nothing (terminal-status skip plus state-machine guard), persistence goes through the CAS `SchedulePersistence` port, and the window matches the pinned baseline (un-jittered `scheduled_for + 5min`, strict `>`; recurring never misses).
  - *Status:* ☑ SATISFIED

- **O3 — Pending jitter-delayed timers are cancelled on stop or lease loss, leaving no late fire**
  - *Claim:* Stopping the scheduler or losing the lease prevents a pending jittered fire from executing.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(scheduler::cancels_pending_timers)'` — expect two cases (stop and lease loss), each asserting zero admissions after the fake clock advances past the jittered instant.
  - *Evidence:* Exact selector passed 1/1 with three cases: stop and lease loss each leave zero admissions after the fake clock advances past the jittered instant, while a control without stop/eviction fires exactly once. Cancellation is real (`tokio::select!` on the token, pending cleared, delayed fires revalidate the captured runtime generation), and restart cannot re-fire retired one-shots.
  - *Status:* ☑ SATISFIED

- **O4 — A one-shot schedule retires after firing while a recurring schedule stays active, and each fire, miss, and failure is written to the run log**
  - *Claim:* Status transitions and run-log entries match the outcome of each evaluation.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(scheduler::outcomes)'` — expect `one_shot_retires`, `recurring_stays_active`, and a run-log entry assertion per outcome kind.
  - *Evidence:* Exact selector passed 1/1 asserting one-shot retirement (`Fired`, fire_count 1), recurring stays `Active` across two minutes (fire_count 2), and canonical run-log entries per outcome (fire `Ok/Queued/one_off_due`, miss `Skipped/Missed/started_too_late`, failure `Error/Failed/scheduler_error`) with reasons from the pinned `CronRunReason` vocabulary. Real-store evidence: `ScheduleService` over temp SidePaths drives a live tick and reads the entry back through `RunLogStore` (12/12 store-side schedule/scheduler_service tests pass).
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace rustdoc with warnings denied, and `cargo deny check` passed. Full suites: lotta-runtime 377/377, lotta-store 206/206, lotta-domain 79/79, testkit source audit 5/5. New functions stay within 70 lines and 100 columns; constants use units-last names; no production unwrap/expect and no new lint suppressions; new spawns are registered in the architecture audit.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(scheduler::)'` and sees queue-routed firing, missed marking, timer cancellation on stop and lease loss, and one-shot retirement pass**
  - *Claim:* The scheduler module passes with enqueue-through-queue asserted.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `enqueues_through_queue` case.
  - *Evidence:* Broad selector passed 6/6 with `enqueues_through_queue` present, covering queue-routed firing, missed marking, timer cancellation on stop and lease loss, and one-shot retirement. Independent review returned `CORRECT / DONE` after line-by-line baseline parity verification.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/admission.rs` (Task 18) admits these prompts; exact `admission::sources` passed 6/6 alongside a live scheduler tick, and the scheduler regression test verifies FIFO preservation across all five source kinds: ☑ PRESERVED

## Residue

Schedule management commands over WebSocket are Task 68; this task owns evaluation and firing only.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 obligations are discharged by exact selectors and first-hand evidence. Due schedules enqueue `cron_prompt` through the real Task 18 admission chain without ever starting a turn directly; past-window one-shots are marked missed exactly once through CAS persistence; jitter-delayed timers are genuinely cancelled on stop and lease loss; one-shots retire while recurring schedules stay active with canonical run-log entries per outcome; and the Task 18 admission regression is preserved. Jitter is read from the persisted field (creation-time computation belongs to Task 68), the miss window and jitter semantics match the pinned scheduler line-for-line, and composition-root wiring is deferred to Task 85 per the plan. Known coarser failure-reason mapping (`scheduler_error` vs `queue_full`) is noted as a non-blocking refinement.
