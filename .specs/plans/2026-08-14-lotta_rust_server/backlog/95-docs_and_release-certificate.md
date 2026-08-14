# Done Certificate — Task 95: Documentation and release readiness

**Task:** [95-docs_and_release.md](95-docs_and_release.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 95. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 95) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** rustdoc without warnings, an architecture and operations guide, a compatibility statement, and a release checklist whose items are executable commands.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not make the release gate a sign-off: a checklist item that is not a command with an expected result cannot be mechanically checked.

## Obligations

- **O1 — `cargo doc --workspace --no-deps` is clean with denied warnings and every crate root documents its responsibility, ports, dependencies, and forbidden dependencies**
  - *Claim:* Documentation builds warning-free and each of the 13 crate roots carries the four documented properties.
  - *Evidence to collect:* Run `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` — expect exit code 0. Read each crate root doc comment and confirm the four properties, per `development-guidelines.md` §Documentation.
  - *Checks:* Resolve the crate list the documentation check iterates — confirm it is derived from `cargo metadata` workspace members, so a crate added later fails the check rather than being silently undocumented.
  - *Status:* ☐ unverified

- **O2 — `docs/compatibility.md` names the pinned baseline, every §Compatibility definition surface, and every labelled Lotta hardening**
  - *Claim:* The document names `letta-code@300f923f` version `0.30.20`, the nine surfaces, and each hardening row from the spec bound tables.
  - *Evidence to collect:* Read `docs/compatibility.md` and check the baseline pin against `.specs/README.md`, the nine surfaces against `00-overview.md` §Compatibility definition, and the hardening list against the rows marked Lotta hardening in the `01`–`07` bound tables.
  - *Status:* ☐ unverified

- **O3 — Every `RELEASE-CHECKLIST.md` item is an executable command with an expected result, including the acceptance gate**
  - *Claim:* No checklist item is a prose instruction; each names a command and the result that satisfies it.
  - *Evidence to collect:* Read `RELEASE-CHECKLIST.md` and confirm every item contains a command in backticks and an expected outcome. Run each command in order and confirm each produces its stated result, including the Task 94 gate.
  - *Status:* ☐ unverified

- **O4 — The license and attribution position for fixtures derived from Apache-2.0 Letta Code is stated**
  - *Claim:* The repository states its Apache-2.0 license and the attribution position for derived fixtures, consistent with `NOTICE`.
  - *Evidence to collect:* Read `docs/compatibility.md` and `NOTICE` and confirm they agree on the Apache-2.0 license and the derivation of fixtures from `letta-ai/letta-code` at the pinned commit.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs every command in `RELEASE-CHECKLIST.md` top to bottom and sees each produce its stated expected result, ending with a green acceptance gate**
  - *Claim:* The checklist is executable end to end.
  - *Evidence to collect:* Run each checklist command in order and record its exit code and output; confirm each matches the stated expectation and the final gate exits 0.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- Every crate gains or changes rustdoc; confirm `cargo doc --workspace --no-deps` with denied warnings still passes for each crate individually : ☐ (PRESERVED / REGRESSION)

## Residue

`00-overview.md` §Assumptions leaves the concurrent-baseline support window open; the compatibility document records the single pinned baseline and defers the window.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
