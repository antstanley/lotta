# Done Certificate — Task 93: Reliability trace conformance

**Task:** [93-reliability_trace_conformance.md](93-reliability_trace_conformance.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 93. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 93) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the seven reliability traces replayed against the assembled binary with matching queue, abort, disconnect, stale-lease, retry, idempotency, and crash-recovery behavior.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not accept a nondeterministic retry sequence: `00-overview.md` §Compatibility definition requires retry traces to match, which jitter would make impossible.

## Obligations

- **O1 — All seven reliability traces replay against the assembled binary and match under the semantic comparator**
  - *Claim:* Each recorded trace reproduces on the Rust server, modulo rule-matched fields.
  - *Evidence to collect:* Run `cargo nextest run --test reliability_traces` — expect seven passing cases; on failure confirm the message names the first diverging event index and both events.
  - *Status:* ☐ unverified

- **O2 — The six ordering invariants hold over every observed trace**
  - *Claim:* Each replay's captured trace passes all six invariants.
  - *Evidence to collect:* Run `cargo nextest run --test reliability_traces -E 'test(ordering)'` — expect six invariant cases per trace or an aggregated case reporting per-invariant results.
  - *Status:* ☐ unverified

- **O3 — Retry delay sequences are deterministic across runs with a controlled clock**
  - *Claim:* Replaying the retry trace twice yields identical delay sequences.
  - *Evidence to collect:* Run the retry case twice and diff the recorded delays — expect equality, confirming the Task 48 jitter-free policy end to end.
  - *Checks:* Resolve the clock the server uses during the replay — confirm it is the injectable `Clock` port, so the determinism assertion is meaningful rather than incidental.
  - *Status:* ☐ unverified

- **O4 — A repeated `client_message_id` returns the prior disposition and does not execute twice**
  - *Claim:* The idempotency trace shows one execution for two identical admissions.
  - *Evidence to collect:* Run `cargo nextest run --test reliability_traces -E 'test(idempotency)'` — expect PASS asserting one provider turn for two identical inputs.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run --test reliability_traces` against the built binary and sees all seven recorded traces reproduce, including deterministic retry timing**
  - *Claim:* The reliability suite is green.
  - *Evidence to collect:* Run the command and confirm zero failures and seven trace cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/retry/policy.rs` (Task 48); confirm `retry::deterministic_delays` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-app-server/src/ws/sync.rs` (Task 73); confirm `ws::sync::replays_snapshots` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Traces are pinned to `letta-code@300f923f`; a baseline move requires recapture through Task 13.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
