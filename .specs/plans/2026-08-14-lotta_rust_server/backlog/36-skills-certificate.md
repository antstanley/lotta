# Done Certificate — Task 36: Skill discovery precedence, frontmatter fallbacks, and selection

**Task:** [36-skills.md](36-skills.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 36. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 36) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** skill discovery in the exact four-level precedence with the baseline optional-frontmatter fallbacks and runtime source restriction.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not invert precedence: `05-tools-and-extensions.md` §Skills orders project above agent, and inverting it silently shadows project skills.

## Obligations

- **O1 — Discovery precedence is project, then agent, then global, then bundled, with both legacy fallback paths honoured**
  - *Claim:* A skill present in all four sources resolves to the project copy; `.skills` is used when `.agents/skills` is absent; `$MEMORY_DIR/skills` is read when the agent path is absent.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(skills::precedence)'` — expect `project_wins_over_agent`, `agent_wins_over_global`, `global_wins_over_bundled`, `legacy_dot_skills_fallback`, and `memory_dir_fallback` to pass.
  - *Checks:* Resolve the ordering comparator — confirm project sorts above agent. `05-tools-and-extensions.md` §Skills lists project first; an agent-first order silently shadows project skills.
  - *Status:* ☐ unverified

- **O2 — Optional frontmatter falls back exactly as the baseline does: ID and name from the path, description from the first body paragraph, then `No description available`**
  - *Claim:* A `SKILL.md` with no frontmatter still loads with a path-derived ID and name and a body-derived description; an empty body yields the literal fallback string.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(skills::frontmatter_fallbacks)'` — expect three cases; the last asserts the exact string `No description available`.
  - *Status:* ☐ unverified

- **O3 — Runtime selection restricts sources without reordering the remaining precedence**
  - *Claim:* Restricting to agent and global yields agent over global, and excludes project and bundled entirely.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(skills::source_restriction)'` — expect PASS asserting both the exclusion and the retained order.
  - *Status:* ☐ unverified

- **O4 — Loading a skill reads its complete instructions and companion files, and skill scripts run under the same permission and sandbox policy as direct tools**
  - *Claim:* A skill with companions loads all of them, and a skill script attempting an out-of-root read is denied by the same gate a direct tool hits.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(skills::load) + test(skills::script_policy)'` — expect both PASS; the second asserts the denial originates from the Task 34/35 gate.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(skills::)'` and sees project-over-agent precedence, both legacy fallbacks, the three frontmatter fallbacks, source restriction, and script policy pass**
  - *Claim:* The skills module passes with precedence asserted in the spec's order.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `project_wins_over_agent` case.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-memfs/src/prompt/compile.rs` (Task 30) now receives a non-empty skill set; confirm `prompt::cache` still passes when the skill set changes : ☐ (PRESERVED / REGRESSION)

## Residue

The model-facing skill-load tool is Task 40; enable/disable commands are Task 69.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
