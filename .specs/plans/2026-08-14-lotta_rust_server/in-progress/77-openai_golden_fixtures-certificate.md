# Done Certificate — Task 77: OpenAI golden request, response, and SSE fixtures

**Task:** [77-openai_golden_fixtures.md](77-openai_golden_fixtures.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 77. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 77) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** checked-in golden fixtures for all three routes, replayed against the running server with event-by-event SSE comparison.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must compare SSE event by event: `00-overview.md` §Compatibility definition requires golden request/response/SSE fixtures for the three routes, and a whole-body comparison would hide ordering differences.

## Obligations

- **O1 — Golden fixtures exist for all three routes covering JSON and SSE, and the index names every covered case**
  - *Claim:* `fixtures/openai/` contains request/response pairs and SSE captures for the three routes with a machine-readable index.
  - *Evidence to collect:* Run `cargo nextest run --test openai_golden -E 'test(index_is_complete)'` — expect PASS listing at least one case per route and per response mode.
  - *Status:* ☐ unverified

- **O2 — Replaying each golden request against the running server reproduces the recorded response, with SSE compared event by event**
  - *Claim:* Every golden case matches, and an SSE mismatch reports the first diverging event index.
  - *Evidence to collect:* Run `cargo nextest run --test openai_golden` — expect one passing case per fixture; on failure confirm the message names the diverging event index and both events.
  - *Checks:* Resolve the server the replay drives — confirm it is the built `lotta` binary started with `--openai-api`, not an in-process route handler. An in-process handler would skip the Task 15 transport bounds and the shared authentication policy.
  - *Status:* ☐ unverified

- **O3 — Chat-completion cases cover stateful, headerless, and idempotent-retry paths; response cases cover stored, non-stored, and `previous_response_id`**
  - *Claim:* All six named cases are present and pass.
  - *Evidence to collect:* Run `cargo nextest run --test openai_golden -E 'test(case_coverage)'` — expect six named cases derived from the fixture index.
  - *Status:* ☐ unverified

- **O4 — No fixture contains a credential or user content**
  - *Claim:* A scanner over `fixtures/openai/` finds no secret-shaped value and no unsanitized transcript text.
  - *Evidence to collect:* Run `cargo nextest run --test openai_golden -E 'test(is_sanitized)'` — expect PASS; read the scanner and confirm it rejects `sk-`, `Bearer `, and any non-placeholder API key.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run --test openai_golden` against a running server with `--openai-api` and sees every golden fixture match, including event-by-event SSE comparison**
  - *Claim:* The golden suite is green with all six named cases.
  - *Evidence to collect:* Run the command and confirm zero failures and a case count equal to the fixture index size.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/openai/` (Tasks 74–76) serve these fixtures; confirm `openai::chat::idempotency` and `openai::responses::cursor` still pass : ☐ (PRESERVED / REGRESSION)

## Residue

Fixtures are captured against `letta-code@300f923f`; moving the baseline pin requires recapture.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
