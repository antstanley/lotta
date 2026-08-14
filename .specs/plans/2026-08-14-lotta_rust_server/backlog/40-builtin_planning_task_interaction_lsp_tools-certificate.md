# Done Certificate — Task 40: Planning, task, skill-load, interaction, and LSP built-in tools

**Task:** [40-builtin_planning_task_interaction_lsp_tools.md](40-builtin_planning_task_interaction_lsp_tools.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 40. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 40) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** `update_plan`/`UpdatePlan`, `TodoWrite`, the six task lifecycle tools, the skill loader, the approval and ask-user bridge, and `ReadLSP` diagnostics.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not ship tools the model has never been trained to call: `05-tools-and-extensions.md` §Assumptions *Tool parity* pins model-facing names to the reference registry.

## Obligations

- **O1 — The planning and task surface is exactly `update_plan`/`UpdatePlan`, `TodoWrite`, and the six task lifecycle tools — no invented plan or memory-recall tool**
  - *Claim:* The registered internal name set for these families matches the §Rust built-ins list and the baseline `tools/impl/` filenames.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::planning::registered_names)'` — expect PASS comparing against the extracted baseline name table. Grep the crate for `create_plan`, `list_plans`, `archival_memory_`, `core_memory_`, and `memory_recall` — expect zero matches, since none appears in `../letta-code/src/tools/impl/`.
  - *Checks:* Resolve `UpdatePlan` — confirm it is an alias resolving to the same internal implementation as `update_plan`, per §Rust built-ins (`Aliases remain protocol-compatible but call one internal implementation`).
  - *Status:* ☐ unverified

- **O2 — The skill-load tool reads one registered skill and its companion files, and fails for an unregistered skill**
  - *Claim:* Loading a discovered skill returns its instructions and companions; loading an unknown ID returns a tool-defined error rather than a filesystem read.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::skill::)'` — expect `loads_registered_skill` and `unknown_skill_is_tool_error` to pass, the second asserting zero filesystem reads outside the registry.
  - *Status:* ☐ unverified

- **O3 — The interaction bridge produces approval and ask-user-question requests that pause the caller and resume on response**
  - *Claim:* Both interaction tools suspend until a response arrives and return the response payload.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::interaction::)'` — expect one case per interaction kind, each asserting suspension then resumption.
  - *Status:* ☐ unverified

- **O4 — `ReadLSP` returns diagnostics only, with no definition or reference lookup**
  - *Claim:* The LSP family exposes a single diagnostics tool matching `../letta-code/src/tools/impl/read-lsp.ts`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::lsp::)'` — expect a diagnostics case. Grep for `get_definition` and `get_references` — expect zero matches, since §Rust built-ins scopes LSP to `ReadLSP` diagnostics.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(builtin::planning::) + test(builtin::task::) + test(builtin::skill::) + test(builtin::interaction::) + test(builtin::lsp::)'` and sees the exact baseline tool set with no invented names**
  - *Claim:* All five families pass and the invented-name greps return nothing.
  - *Evidence to collect:* Run the filter (expect zero failures) and run the invented-name grep set (expect zero matches).
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-extensions/src/skills/` (Task 36) backs the skill loader; confirm `skills::load` still passes when driven through the tool : ☐ (PRESERVED / REGRESSION)

## Residue

The interaction bridge emits requests; the runtime approval flow that consumes them is Task 56.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
