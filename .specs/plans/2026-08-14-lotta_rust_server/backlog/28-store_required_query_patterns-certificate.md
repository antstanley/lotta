# Done Certificate — Task 28: Required query patterns and message projections

**Task:** [28-store_required_query_patterns.md](28-store_required_query_patterns.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 28. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 28) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the ten required query behaviors including the `letta-msg-`/`ui-msg-` projection mapping and deterministic list ordering.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let a `default` conversation resolve across agents: `01-domain-model.md` §ID scheme states `default` is agent-scoped and not globally unique.

## Obligations

- **O1 — All ten §Required query patterns have an implementation and a test asserting the stated required behavior**
  - *Claim:* Each row of the §Required query patterns table maps to a named test that asserts its behavior, not merely that the query returns.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(query::)'` — expect ten cases named for the ten table rows. Read the test list and confirm the mapping is one-to-one with the table.
  - *Status:* ☐ unverified

- **O2 — Every projection key resolves to the same source local message**
  - *Claim:* Looking up a message by its `letta-msg-` projection ID and by its `ui-msg-` transcript ID returns the same underlying record.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(query::projection::same_source_message)'` — expect PASS; the test asserts identity of the resolved record, not only field equality.
  - *Checks:* Resolve the ID type at each lookup site — confirm the projection lookup takes the `letta-msg-` form and the transcript lookup the `ui-msg-` form. `01-domain-model.md` §ID scheme reserves them for different surfaces and `letta-msg-` is not stored in transcript JSONL.
  - *Status:* ☐ unverified

- **O3 — `default` never crosses agents and a wrong-prefix agent ID returns 404 rather than a lookup miss**
  - *Claim:* Resolving `default` under agent A never returns agent B's conversation, and `agent-cloud-x` yields a 404-class error.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(query::scope_isolation) + test(query::wrong_prefix_404)'` — expect both PASS.
  - *Status:* ☐ unverified

- **O4 — List ordering is deterministic and cursor behavior is stable across repeated calls with unchanged state**
  - *Claim:* Two identical list calls return identical order, and paging with a cursor visits every item exactly once.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-store -E 'test(query::deterministic_order) + test(query::cursor_covers_all)'` — expect both PASS; the cursor case asserts the union of pages equals the full set with no duplicates.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-store -E 'test(query::)'` and sees one passing case per row of `01-domain-model.md` §Required query patterns, including the projection-identity and scope-isolation cases**
  - *Claim:* All ten query behaviors pass.
  - *Evidence to collect:* Run the filter and confirm ten cases and zero failures.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/transcript/load.rs` (Task 25) supplies the active projection these queries read; confirm `transcript::load::tolerances` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

`01-domain-model.md` §Assumptions leaves a derived SQLite index open; this task keeps JSON/JSONL canonical and adds no index.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
