# Done Certificate — Task 88: Failure-injection suite

**Task:** [88-failure_injection_suite.md](88-failure_injection_suite.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 88. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 88) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** proof of baseline queue semantics, Lotta atomic-write and conflict hardening, cancellation, stale-lease suppression, reconnect recovery, and sidecar crash handling.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not weaken exactly-once terminal emission under races: `02-app-server-api.md` §Event envelopes and ordering invariant 4 is the property this suite exists to defend.

## Obligations

- **O1 — Disk-full and partial-write injections produce distinct typed errors and leave the store recoverable**
  - *Claim:* Each injection maps to its own error kind and the on-disk state is either the old or the new version, never a torn one.
  - *Evidence to collect:* Run `cargo nextest run --test failure_injection -E 'test(storage::)'` — expect a case per injection asserting the error kind and a post-condition that the file parses as one of the two versions.
  - *Status:* ☐ unverified

- **O2 — The queue soft and hard tiers, stale-lease suppression, and reconnect recovery match the recorded reference traces**
  - *Claim:* Replaying each `fixtures/reference-traces/` reliability capture against the running server reproduces the recorded behavior.
  - *Evidence to collect:* Run `cargo nextest run --test failure_injection -E 'test(reliability::)'` — expect one case per reliability surface (queue, abort, disconnect, stale lease, retry, idempotency, crash recovery) compared under the Task 13 semantic comparator.
  - *Checks:* Resolve the comparison rule for `idempotency_key` — confirm the suite asserts per-emission uniqueness, not cross-replay stability.
  - *Status:* ☐ unverified

- **O3 — A cancellation racing a streaming provider yields exactly one `turn_finished(cancelled)` under repeated trials**
  - *Claim:* Across many randomized-timing trials the terminal event count is always one.
  - *Evidence to collect:* Run `cargo nextest run --test failure_injection -E 'test(cancellation::race)'` — expect PASS over at least 100 fake-clock-scheduled trials with a terminal-count assertion of exactly one each.
  - *Status:* ☐ unverified

- **O4 — Crashing the mod host, provider host, or a subagent produces cleanup, a bounded restart, and a typed error without host state corruption**
  - *Claim:* Each crash resolves in-flight work with a typed error, restarts within `SIDECAR_RESTARTS_PER_HOUR_MAX`, and leaves registrations consistent.
  - *Evidence to collect:* Run `cargo nextest run --test failure_injection -E 'test(sidecar::)'` — expect three crash cases, each asserting the typed error, the restart bound, and a consistent registry snapshot.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run --test failure_injection` and sees storage, reliability-trace, cancellation-race, and sidecar-crash cases pass against the assembled binary**
  - *Claim:* The failure-injection suite is green and covers all seven reliability surfaces.
  - *Evidence to collect:* Run the command and confirm zero failures and seven `reliability::` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/cancel.rs` (Task 57) and `queue.rs` (Task 18) are stressed here; confirm `cancel::exactly_once` and `queue::hard_tier` still pass in isolation : ☐ (PRESERVED / REGRESSION)

## Residue

This suite satisfies `00-overview.md` §Implementation acceptance criterion 5.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
