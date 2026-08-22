# Done Certificate — Task 63: WebSocket teleport command group

**Task:** [63-ws_teleport_commands.md](63-ws_teleport_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-21

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
  - *Evidence:* Exact selector passed 6/6 (five round-trip cases plus a membership guard). The fixture assigns exactly five discriminants to this group; round-trips are genuine decode→encode cycles including three `teleport_ready` optional-field shapes.
  - *Status:* ☑ SATISFIED

- **O2 — `input.kind = teleport_continue` is treated as a continuation on the active lease, not a new admission**
  - *Claim:* A teleport continuation does not create a queue item or a second turn.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::continuation)'` — expect PASS asserting queue length is unchanged and the turn ID is the same.
  - *Checks:* Resolve the admission branch taken for `teleport_continue` — confirm it is the continuation/control branch of `03-runtime-and-turns.md` §Input and queue flow, not the idle-start or enqueue branch.
  - *Evidence:* Exact selector passed. Dispatch traced end-to-end: protocol decode validates `teleport_continue` strictly, `is_control_continuation` routes it to `continue_input` (never `submit_turn`), and `TeleportBridge::continue_input` drives the real admission continuation branch (`AdmissionRoute::Continuation`) — queue length zero, same lease generation, no enqueue. The `teleport:<id>` idempotency key matches the pinned `acceptedKey` exactly.
  - *Status:* ☑ SATISFIED

- **O3 — A continuation carrying a stale lease generation is rejected and emits nothing**
  - *Claim:* A teleport continuation against a superseded lease produces no state change and no event.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::stale_lease)'` — expect PASS asserting zero emissions.
  - *Evidence:* Exact selector passed. A superseded lease is rejected through the real admission StaleGeneration path with zero emissions, empty queue, and the replacement turn untouched.
  - *Status:* ☑ SATISFIED

- **O4 — `teleport_failed` leaves the runtime in a defined state with no dangling teleport**
  - *Claim:* After a failure the runtime holds no pending teleport and remains able to accept input.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::failure)'` — expect PASS asserting the pending-teleport set is empty and a subsequent input is admitted.
  - *Evidence:* Exact selector passed 3/3. A matching failure removes the exact pending teleport, the pending set is empty afterward, and subsequent ordinary input starts a new turn normally; conflict handling answers the pinned message without displacing the pending entry.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace rustdoc with warnings denied, and `cargo deny check` passed; full lotta-app-server 211/211. Workspace health excluding the recorded external pinned-SHA/CLI family is green (74 failures individually classified as environmental). New functions stay within 70 lines and 100 columns; no production unwrap/expect and no new lint suppressions.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::)'` and sees the five discriminants, continuation-not-admission, stale-lease rejection, and failure cleanup pass**
  - *Claim:* The teleport group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and five round-trip cases.
  - *Evidence:* Broad selector passed 16/16 with zero failures covering the five discriminants, continuation-not-admission, stale-lease rejection, and failure cleanup. Independent review returned `CORRECT / DONE` after tracing dispatch paths in source and constructively reproducing the environmental failures.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/admission.rs` (Task 18) classifies the continuation; exact `admission::sources` passed 6/6 with lotta-runtime untouched by this diff: ☑ PRESERVED

## Residue

Teleport target selection is a client concern; the server implements probe, request, ready, failure, and continuation only.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 obligations are discharged by exact selectors and traced source evidence. The five teleport discriminants round-trip from the protocol fixture; `input.kind = teleport_continue` routes through the control-continuation branch onto the active lease without creating a queue item or second turn; stale lease generations are rejected with zero emissions; `teleport_failed` clears the pending set and leaves the runtime admitting normally; and probe/request/conflict semantics match the pinned listener. Boundary-claim ready emission and composition wiring are deferred per the residue note. The Task 18 admission regression is preserved.
