# Done Certificate — Task 68: WebSocket schedules command group

**Task:** [68-ws_schedule_commands.md](68-ws_schedule_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-22

> Verification protocol for Task 68. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 68) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** cron list, add, get, runs, trigger, update, delete, and delete-all commands over the Task 60 store and Task 61 scheduler.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let `trigger` bypass the queue: `01-domain-model.md` §Schedule requires schedule execution to enqueue a `cron_prompt`.

## Obligations

- **O1 — All eight schedule commands decode, route, and respond, with fixture round-trip coverage**
  - *Claim:* Each command in the Schedules row has a passing case.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::schedules::commands)'` — expect eight cases derived from the fixture group listing.
  - *Evidence:* Exact selector passed 8/8, one substantive case per command. All nine outbound messages match the pinned schedule-protocol shapes field-for-field; `covers_exactly_this_group` proves exactly-once membership of the group's 8 command and 9 message discriminants with genuine decode→encode round-trips.
  - *Status:* ☑ SATISFIED

- **O2 — `trigger` enqueues through the conversation queue rather than starting a turn directly**
  - *Claim:* A triggered schedule produces a `cron_prompt` admission, not a direct turn start.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::schedules::trigger_enqueues)'` — expect PASS asserting the admission chain was used.
  - *Checks:* Resolve the trigger implementation — confirm it calls the Task 61 firing path, not a direct lifecycle start; `01-domain-model.md` §Schedule forbids bypassing the queue.
  - *Evidence:* Exact selector passed 2/2. Trigger calls the extracted Task 61 firing path (`ScheduleScheduler::trigger`) — the same `fire()` as scheduled ticks, additive-only extraction with zero tick-path changes — producing a `cron_prompt` queue admission while the lifecycle stays `Idle`; replay admits as `Duplicate(Queued)` and a fired run-log entry references its queue item.
  - *Status:* ☑ SATISFIED

- **O3 — Add and update validate the cron/interval expression and the IANA timezone, rejecting invalid values without persisting**
  - *Claim:* An invalid expression and an unknown timezone are both rejected and nothing is written.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::schedules::validation)'` — expect two rejection cases asserting zero writes.
  - *Evidence:* Exact selector passed 4/4 (exceeding the two expected). Invalid cron expressions, invalid intervals, and unknown IANA timezones are rejected on both add and update with byte-identical store bytes and no file creation; validation completes entirely before store access.
  - *Status:* ☑ SATISFIED

- **O4 — `runs` returns bounded run history and a mutating command emits a cron update snapshot**
  - *Claim:* The runs response respects the log bounds, and add, update, delete, and delete-all each emit a snapshot.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::schedules::runs_and_snapshots)'` — expect a bounded-output case and four snapshot cases.
  - *Evidence:* Exact selector passed 6/6. Runs pagination matches pinned semantics (default 50, clamp [1,200], newest-first, runId filter, tail null nextOffset) under the frame bound; add/update/delete/delete-all each emit `crons_updated` in pinned order, while absent deletions correctly emit none.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace rustdoc with warnings denied, and `cargo deny check` passed; full lotta-app-server 341/341. Workspace residue remains confined to the recorded external pinned-SHA/CLI family. New functions stay within 70 lines and 100 columns; constants use units-last names; option-option allows carry why-comments per domain precedent; no production unwrap/expect.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::schedules::)'` and sees eight commands, queue-routed trigger, expression and timezone validation, and bounded runs pass**
  - *Claim:* The schedules group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and eight command cases.
  - *Evidence:* Broad selector passed 28/28 with zero failures covering eight commands, queue-routed trigger, expression and timezone validation, bounded runs, and snapshots. Independent review returned `CORRECT / DONE`, verifying the scheduler extraction is additive and the occurrence identity is shared with scheduled ticks.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/schedule/scheduler.rs` (Task 61) handles triggers; confirm `scheduler::enqueues_through_queue` still passes : ☑ PRESERVED

## Residue

Schedule evaluation timing remains the scheduler's; this group only manages schedule records and manual triggers.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 obligations are discharged by exact selectors and traced evidence. The eight schedule commands decode, route, and respond in pinned shapes with fixture round-trips; trigger routes through the extracted Task 61 firing path so manual runs enqueue through the conversation queue exactly like scheduled ticks; cron/interval and timezone validation precedes all store access with zero writes on rejection; runs history paginates within bounds matching the pinned reader; and every mutation emits its cron update snapshot. Schedule evaluation timing remains the scheduler's per the residue note, and the Task 61 regression gate is preserved.
