# Done Certificate — Task 11: Persistence fixture corpus extraction

**Task:** [11-fixtures_persistence_corpus.md](11-fixtures_persistence_corpus.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 11. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 11) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a sanitized `fixtures/persistence/` corpus covering the seven cross-runtime cases, checked in before any store code that it constrains.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must land before Tasks 23–27: `architecture-principles.md` §Assumptions *Compatibility evidence* states prose and Rust types alone cannot prove wire parity, so the corpus has to exist before the code it constrains.

## Obligations

- **O1 — All seven `04-persistence-and-memfs.md` §Migration cross-runtime cases are present as named fixture directories, plus the interrupted-append and interrupted-replacement recovery cases**
  - *Claim:* `fixtures/persistence/` contains a directory per case, and the testkit index names all nine.
  - *Evidence to collect:* List `fixtures/persistence/` and match the directory names against the seven numbered cases in §Migration plus `interrupted_append` and `interrupted_replacement` — expect nine. Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::persistence::index_is_complete)'` — expect PASS.
  - *Status:* ☐ unverified

- **O2 — Both baseline conversation directory key forms appear verbatim in the corpus**
  - *Claim:* The corpus contains a directory named `base64url("default:" + agent-id)` and one named `base64url("conversation:" + conversation-id)` for the same agent.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::persistence::both_key_forms)'` — expect PASS, and read the test to confirm it decodes each directory name and asserts the two distinct prefixes required by `04-persistence-and-memfs.md` §Local-backend directory layout.
  - *Checks:* Resolve the base64url encoder used to generate the fixture directory names — confirm it is the URL-safe encoder matching `../letta-code/src/backend/local/paths.ts`, not standard base64. A padding or alphabet difference would make Task 22's key-form test pass against a wrong corpus.
  - *Status:* ☐ unverified

- **O3 — Side-store fixtures cover `settings.json`, `crons.json`, `runs/<schedule-id>.jsonl`, `providers/auth.json` v1, and a channel directory with `config.yaml`, `accounts.json`, `routing.yaml`, `pairing.yaml`, and `targets.json`**
  - *Claim:* Each path named in `04-persistence-and-memfs.md` §State outside the backend root has a fixture.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::persistence::side_stores)'` — expect one passing case per path in the §State outside the backend root listing.
  - *Status:* ☐ unverified

- **O4 — No fixture contains a credential, a real transcript, or an unsanitized provider payload**
  - *Claim:* A scanner over the corpus finds no secret-shaped value and no unsanitized content.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::persistence::is_sanitized)'` — expect PASS. Read the scanner and confirm it rejects `sk-`, `Bearer `, private-key headers, and any `api_key` value that is not the documented placeholder.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-testkit -E 'test(fixtures::persistence::)'` and sees the nine cases, both key forms, every side store, and the sanitization scan pass**
  - *Claim:* The persistence-fixture test module passes with a case per fixture class.
  - *Evidence to collect:* Run the filter and confirm zero failures and at least nine indexed cases.
  - *Status:* ☐ unverified

## Regression check

- Task 09 fixture indexing and deterministic testkit APIs load this corpus; run `cargo nextest run -p lotta-testkit -E 'test(contract::) | test(fixtures::persistence)'` and confirm both suites pass : ☐ (PRESERVED / REGRESSION)

## Residue

The corpus is generated against `letta-code@300f923f`; moving the baseline pin requires regenerating it. `.specs/README.md` §Assumptions records who approves a baseline move as open.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
