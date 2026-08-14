# Done Certificate — Task 90: Agent SDK conformance

**Task:** [90-sdk_conformance.md](90-sdk_conformance.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 90. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 90) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the remote-backend conformance suite creating, resuming, streaming, compacting, forking, and deleting local agents and conversations without client patches.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not patch the SDK: `00-overview.md` §Implementation acceptance criterion 4 requires the suites to pass without client patches.

## Obligations

- **O1 — All six SDK operations succeed against the assembled binary with an unmodified client**
  - *Claim:* Create, resume, stream, compact, fork, and delete each complete, with no patch applied to the SDK package.
  - *Evidence to collect:* Run `cargo nextest run --test sdk` — expect six passing operation cases. Run `git -C ../letta-code status --porcelain` and confirm a clean tree, and read the harness to confirm no monkey-patching.
  - *Status:* ☐ unverified

- **O2 — The observed event stream satisfies the six ordering invariants**
  - *Claim:* The captured trace passes the Task 13 comparator's invariant set.
  - *Evidence to collect:* Run `cargo nextest run --test sdk -E 'test(ordering)'` — expect six passing invariant cases over the live capture.
  - *Status:* ☐ unverified

- **O3 — The suite runs against both a fresh root and a corpus-populated root**
  - *Claim:* Both starting states produce the same six passing operations.
  - *Evidence to collect:* Run `cargo nextest run --test sdk -E 'test(fresh_root) + test(populated_root)'` — expect both configurations to pass.
  - *Status:* ☐ unverified

- **O4 — The suite fails if the SDK sends a command the server drops as unknown**
  - *Claim:* Unknown-command drops are counted and a non-zero count fails the suite.
  - *Evidence to collect:* Run `cargo nextest run --test sdk -E 'test(no_unknown_commands)'` — expect PASS; read the assertion and confirm it reads the server's unknown-discriminant counter rather than inferring from client behavior.
  - *Checks:* Resolve the counter the assertion reads — confirm it is incremented by the Task 10 decode policy's unknown-type branch, so a silently dropped command is visible.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run --test sdk` against the built binary and watches the unmodified Agent SDK create, resume, stream, compact, fork, and delete a local agent**
  - *Claim:* The SDK suite is green with an unpatched client.
  - *Evidence to collect:* Run the command, confirm zero failures, and confirm `git -C ../letta-code status --porcelain` reports nothing.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/ws/` (Tasks 20, 70, 71, 73) serve these operations; confirm `ws::ordering_invariants` and `ws::conversations::fork` still pass : ☐ (PRESERVED / REGRESSION)

## Residue

This suite covers half of `00-overview.md` §Implementation acceptance criterion 4; the Desktop half is Task 91.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
