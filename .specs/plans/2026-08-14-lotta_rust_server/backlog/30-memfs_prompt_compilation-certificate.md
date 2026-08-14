# Done Certificate — Task 30: System prompt compilation and cache reuse

**Task:** [30-memfs_prompt_compilation.md](30-memfs_prompt_compilation.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 30. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 30) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** `system-prompt.json` with exactly the six compatibility fields and cache reuse keyed on `rawSystemHash` and `memfsRevision`.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not persist model/toolset inputs or a rendered-content hash in `system-prompt.json`: §Prompt compilation fixes the record to six fields for TypeScript round-trip compatibility.

## Obligations

- **O1 — `system-prompt.json` contains exactly the six fields of §Prompt compilation and no others**
  - *Claim:* Serializing the compiled record yields exactly `content`, `coreMemory`, `compiledAt`, `rawSystemHash`, and optionally `midConversationSystemPrompt` and `memfsRevision`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-memfs -E 'test(prompt::record_shape)'` — expect PASS; the test asserts the key set is exactly the allowed set, so an added field fails. Compare against the `fixtures/persistence/` prompt fixture.
  - *Checks:* Resolve what the record does not persist — confirm model/toolset inputs and a rendered-content hash are absent; §Prompt compilation states they are not persisted.
  - *Status:* ☐ unverified

- **O2 — Cache reuse compares only `rawSystemHash` and `memfsRevision`; an unchanged pair skips recompilation and a changed one forces it**
  - *Claim:* Two compilations with an unchanged hash and revision perform one render; changing either forces a second render.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-memfs -E 'test(prompt::cache)'` — expect `unchanged_pair_reuses`, `changed_hash_recompiles`, and `changed_revision_recompiles` to pass, each asserting the render-call count via a counter.
  - *Status:* ☐ unverified

- **O3 — An uncommitted memory working tree is visible to tools but does not become authoritative prompt memory until committed**
  - *Claim:* Writing a memory file without committing leaves the compiled prompt unchanged while the file is readable through the memfs port.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-memfs -E 'test(prompt::uncommitted_not_authoritative)'` — expect PASS; the test asserts the read succeeds and the compiled `content` is unchanged.
  - *Status:* ☐ unverified

- **O4 — A committed memory update is injected mid-conversation where the provider supports system messages, and otherwise applies at the next provider request boundary**
  - *Claim:* Both paths are exercised and produce the documented behavior.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-memfs -E 'test(prompt::mid_conversation_injection)'` — expect a supporting-provider case producing `midConversationSystemPrompt` and a non-supporting case deferring to the next request.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-memfs -E 'test(prompt::)'` and sees the exact six-field record, cache reuse on both keys, uncommitted-not-authoritative, and both mid-conversation paths pass**
  - *Claim:* The prompt module passes against the fixture.
  - *Evidence to collect:* Run the filter and confirm zero failures and that `record_shape` compared against `fixtures/persistence/`.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-memfs/src/repo.rs` (Task 29) supplies the committed revision; confirm `ops::` still passes with the compiler reading revisions : ☐ (PRESERVED / REGRESSION)

## Residue

Skill selection feeding compilation arrives in Task 36; until then the compiler accepts an empty skill set.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
