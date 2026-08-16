# Done Certificate — Task 40: Planning, task, skill-load, interaction, and LSP built-in tools

**Task:** [40-builtin_planning_task_interaction_lsp_tools.md](40-builtin_planning_task_interaction_lsp_tools.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-16 — DONE

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
  - *Status:* ☒ SATISFIED — exact planning inventory passes; both plan aliases resolve to one executor/state, TodoWrite executes, and all six lifecycle names compose once with no forbidden names.

- **O2 — The skill-load tool reads one registered skill and its companion files, and fails for an unregistered skill**
  - *Claim:* Loading a discovered skill returns its instructions and companions; loading an unknown ID returns a tool-defined error rather than a filesystem read.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::skill::)'` — expect `loads_registered_skill` and `unknown_skill_is_tool_error` to pass, the second asserting zero filesystem reads outside the registry.
  - *Status:* ☒ SATISFIED — 3/3 skill cases load registered instructions/companions with bounds and return a fixed unknown-skill error before any registry-backed read.

- **O3 — The interaction bridge produces approval and ask-user-question requests that pause the caller and resume on response**
  - *Claim:* Both interaction tools suspend until a response arrives and return the response payload.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::interaction::)'` — expect one case per interaction kind, each asserting suspension then resumption.
  - *Status:* ☒ SATISFIED — approval and AskUserQuestion producer futures demonstrably suspend then resume with correlated validated responses; cancellation, timeout, and closure remain distinct.

- **O4 — `ReadLSP` returns diagnostics only, with no definition or reference lookup**
  - *Claim:* The LSP family exposes a single diagnostics tool matching `../letta-code/src/tools/impl/read-lsp.ts`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::lsp::)'` — expect a diagnostics case. Grep for `get_definition` and `get_references` — expect zero matches, since §Rust built-ins scopes LSP to `ReadLSP` diagnostics.
  - *Status:* ☒ SATISFIED — 3/3 LSP cases expose configured bounded diagnostics only; definition/reference identifiers are absent.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☒ SATISFIED — fmt, strict workspace Clippy, workspace nextest 1,278/1,278, private Rustdoc, and `cargo deny check` pass; touched Rust meets file/function/column limits.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(builtin::planning::) + test(builtin::task::) + test(builtin::skill::) + test(builtin::interaction::) + test(builtin::lsp::)'` and sees the exact baseline tool set with no invented names**
  - *Claim:* All five families pass and the invented-name greps return nothing.
  - *Evidence to collect:* Run the filter (expect zero failures) and run the invented-name grep set (expect zero matches).
  - *Status:* ☒ SATISFIED — clean reviewer ran the full Task 40 selector 22/22 and verified exact pinned assets and no invented names.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-extensions/src/skills/` (Task 36) backs the skill loader; confirm `skills::load` still passes when driven through the tool : ☒ PRESERVED — extension suite 29/29 and Task36 load selector 5/5.

## Residue

The interaction bridge emits requests; the runtime approval flow that consumes them is Task 56.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: The exact pinned planning, six-tool lifecycle, skill, interaction-producer, and diagnostics-only LSP surface executes through one Task40 bundle with bounded per-conversation state, transactional task dependencies, validated suspension/resumption, and all repository gates passing.
