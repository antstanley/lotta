# Done Certificate — Task 63: WebSocket teleport command group

**Task:** [63-ws_teleport_commands.md](63-ws_teleport_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 63. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 63) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** `teleport_probe`/`teleport_request`/`teleport_failed` with the two response messages and `input.kind = teleport_continue` continuation.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not admit a teleport continuation as a new input: `02-app-server-api.md` §WebSocket command groups defines the continuation as `input.kind = teleport_continue`.

## Obligations

- **O1 — The three commands and two messages decode and encode with the baseline discriminants**
  - *Claim:* Each of the five type strings round-trips through the protocol enums.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::fixture_round_trip)'` — expect five cases derived from `fixtures/protocol/discriminants.json`.
  - *Status:* ☐ unverified

- **O2 — `input.kind = teleport_continue` is treated as a continuation on the active lease, not a new admission**
  - *Claim:* A teleport continuation does not create a queue item or a second turn.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::continuation)'` — expect PASS asserting queue length is unchanged and the turn ID is the same.
  - *Checks:* Resolve the admission branch taken for `teleport_continue` — confirm it is the continuation/control branch of `03-runtime-and-turns.md` §Input and queue flow, not the idle-start or enqueue branch.
  - *Status:* ☐ unverified

- **O3 — A continuation carrying a stale lease generation is rejected and emits nothing**
  - *Claim:* A teleport continuation against a superseded lease produces no state change and no event.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::stale_lease)'` — expect PASS asserting zero emissions.
  - *Status:* ☐ unverified

- **O4 — `teleport_failed` leaves the runtime in a defined state with no dangling teleport**
  - *Claim:* After a failure the runtime holds no pending teleport and remains able to accept input.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::failure)'` — expect PASS asserting the pending-teleport set is empty and a subsequent input is admitted.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::)'` and sees the five discriminants, continuation-not-admission, stale-lease rejection, and failure cleanup pass**
  - *Claim:* The teleport group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and five round-trip cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/admission.rs` (Task 18) classifies the continuation; confirm `admission::sources` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Teleport target selection is a client concern; the server implements probe, request, ready, failure, and continuation only.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
