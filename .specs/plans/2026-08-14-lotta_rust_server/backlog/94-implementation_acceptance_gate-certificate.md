# Done Certificate — Task 94: Implementation acceptance gate

**Task:** [94-implementation_acceptance_gate.md](94-implementation_acceptance_gate.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 94. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 94) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** one executable gate that fails unless every `00-overview.md` §Implementation acceptance criterion and every §Compatibility definition surface is proven green.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not hard-code the criterion list: reading it from the spec is what makes a future spec addition fail the gate rather than pass unnoticed.

## Obligations

- **O1 — Every one of the six §Implementation acceptance criteria maps to a named suite and the mapping is asserted rather than documented**
  - *Claim:* The gate reads the criterion list and fails if any criterion has no mapped suite.
  - *Evidence to collect:* Run `cargo nextest run --test acceptance -E 'test(criteria_are_mapped)'` — expect PASS with six mapped criteria. Read the test and confirm the criterion list is read from `.specs/00-overview.md` rather than hard-coded, so a seventh criterion fails the gate.
  - *Status:* ☐ unverified

- **O2 — Every one of the nine §Compatibility definition surfaces maps to a named suite and passes**
  - *Claim:* WebSocket protocol, Agent SDK, Desktop, OpenAI API, Persistence, MemFS, Tools, Channels, and Reliability each have a green suite.
  - *Evidence to collect:* Run `cargo nextest run --test acceptance -E 'test(surfaces_are_covered)'` — expect nine passing cases naming their suites.
  - *Status:* ☐ unverified

- **O3 — The gate is one command that fails on any red or missing suite and emits a machine-readable report**
  - *Claim:* Running the gate with one suite forced red fails, and the report names the failing criterion.
  - *Evidence to collect:* Run the gate normally (expect pass) and with an injected failure (expect non-zero exit and a report naming the criterion and suite). Read the emitted report file and confirm it lists all fifteen entries with results.
  - *Checks:* Resolve each mapped suite name — confirm it corresponds to a real test target that exists in the workspace, so a renamed suite fails the mapping rather than silently passing.
  - *Status:* ☐ unverified

- **O4 — The gate runs in CI as a required check**
  - *Claim:* A dedicated workflow runs the gate and is required on the default branch.
  - *Evidence to collect:* Read `.github/workflows/acceptance.yml` and confirm it runs the gate command. Run `gh api repos/:owner/:repo/branches/main/protection/required_status_checks` (or read the repository settings) and confirm the acceptance check is required.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs the acceptance gate command and sees a report listing all six implementation-acceptance criteria and all nine compatibility surfaces with their suites green**
  - *Claim:* The gate passes and its report is complete.
  - *Evidence to collect:* Run the gate and read the emitted report; confirm fifteen entries, each naming a suite and a pass result, and a zero exit code.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- Every suite from Tasks 31, 53, 77, 87–93 is invoked here; confirm each still passes standalone before the gate runs : ☐ (PRESERVED / REGRESSION)

## Residue

The gate proves the criteria are met at a point in time; keeping it green is the ongoing obligation of every later change.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
