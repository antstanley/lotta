# Done Certificate — Task 60: Schedule store, cron parsing, and run logs

**Task:** [60-schedule_store_and_run_logs.md](60-schedule_store_and_run_logs.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 60. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 60) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** `crons.json` persistence for the full Schedule shape with cron and interval parsing, IANA timezones, and rotated per-schedule run logs.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not narrow to five-field cron only: `01-domain-model.md` §Schedule specifies a cron expression plus an IANA timezone, and the baseline also parses intervals.

## Obligations

- **O1 — The full `$defs.Schedule` shape persists and loads from `crons.json`, round-tripping the fixture without losing a field**
  - *Claim:* All 25 required fields survive a write-read cycle against the corpus fixture.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(schedule::store_round_trip)'` — expect PASS validating against `$defs.Schedule` rather than a literal.
  - *Status:* ☐ unverified

- **O2 — Cron expressions and the baseline interval forms both parse, and next-fire times resolve in the schedule's IANA timezone including a DST transition**
  - *Claim:* A cron expression, an interval expression, and a DST-crossing schedule each resolve to the expected instant.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(schedule::parsing)'` — expect `parses_cron`, `parses_interval` (matching `../letta-code/src/cron/parse-interval.ts`), and `resolves_across_dst`.
  - *Checks:* Resolve the timezone applied to next-fire computation — confirm it is the schedule's `timezone` field, not the process local zone. `01-domain-model.md` §Schedule pins the IANA timezone to the schedule.
  - *Status:* ☐ unverified

- **O3 — `jitter_offset_ms` follows the baseline rules for recurring, one-shot at :00/:30, and other one-shots**
  - *Claim:* The three cases produce a bounded late offset, a negative early offset up to 90 s, and zero respectively.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(schedule::jitter_offset)'` — expect three cases; compare bounds against `../letta-code/src/cron/cron-file.ts` `computeJitter`.
  - *Checks:* Resolve this `jitter` — confirm it is schedule jitter and shares no code or constant with the provider retry path, which `06-model-providers.md` §Retry and fallback requires to be jitter-free.
  - *Status:* ☐ unverified

- **O4 — The four-state lifecycle, fire/miss/fail counters, and `cancel_reason` behave as `01-domain-model.md` §Schedule specifies**
  - *Claim:* Transitions among `active`, `fired`, `missed`, and `cancelled` update the right counters and record a cancel reason.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(schedule::lifecycle)'` — expect one case per transition with counter assertions.
  - *Status:* ☐ unverified

- **O5 — Run history appends to `runs/<schedule-id>.jsonl` and rotates at both bounds**
  - *Claim:* The log rotates at `SCHEDULE_RUN_LOG_KEEP_LINES` lines and `SCHEDULE_RUN_LOG_BYTES_MAX` bytes.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(schedule::run_log)'` — expect below/at/above cases for both bounds from `01-domain-model.md` §Resource bounds.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(schedule::)'` and sees the 25-field round trip, cron and interval parsing across DST, baseline jitter offsets, the four-state lifecycle, and run-log rotation pass**
  - *Claim:* The schedule module passes against the fixture.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `store_round_trip` case validating against the schema.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/side/crons.rs` (Task 27) persists this file; confirm `side::paths` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Firing and enqueue behavior are Task 61; the WebSocket schedule commands are Task 68.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
