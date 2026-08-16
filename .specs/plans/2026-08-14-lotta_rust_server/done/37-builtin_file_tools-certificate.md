# Done Certificate — Task 37: File built-in tools

**Task:** [37-builtin_file_tools.md](37-builtin_file_tools.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-16

> Verification protocol for Task 37. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 37) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the ten file built-ins — read, write, edit, multi-edit, apply-patch, list, glob, grep, image view, and artifact-file read/write — under policy, sandbox, and clamps.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not invent a model-facing tool name: `05-tools-and-extensions.md` §Assumptions *Tool parity* requires model-facing names, schemas, fallbacks, and clamps to follow the pinned registry.

## Obligations

- **O1 — All ten file built-ins named in `05-tools-and-extensions.md` §Rust built-ins exist and execute through the Task 33 pipeline**
  - *Claim:* Read, write, edit, multi-edit, apply-patch, list, glob, grep, image view, and artifact read/write each have a registered definition and a passing execution test.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::file::)'` — expect at least ten execution cases, one per tool. Compare the registered internal names against `../letta-code/src/tools/impl/` filenames.
  - *Evidence:* `builtin::file::` passed 62/62. Its `execution` selector passed 16/16
    substantive Task 33 pipeline cases covering all ten families and all 18 registrations.
  - *Status:* ☑ SATISFIED

- **O2 — Each tool's per-toolset model-facing names match the baseline, including Codex and Gemini variants**
  - *Claim:* For every file tool, the name in each of the six toolsets equals the baseline name table entry.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::file::names_match_baseline)'` — expect PASS, comparing against the table extracted in Task 32 rather than literals; confirm the Gemini variants from `../letta-code/src/tools/impl/read-file-gemini.ts` and `glob-gemini.ts` are covered.
  - *Evidence:* `builtin::file::names_match_baseline` passed 1/1. It derives expected
    model/internal pairs from Task 32 for all six toolsets, validates exactly 18 unique
    registrations, and byte-checks the 18 pinned schemas and descriptions.
  - *Status:* ☑ SATISFIED

- **O3 — Path traversal, symlink escape, and out-of-root write are rejected for every path-taking file tool**
  - *Claim:* Each path-taking tool rejects `..`, a symlink pointing outside the root, and an absolute out-of-root path.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::file::confinement)'` — expect three negative cases per path-taking tool.
  - *Checks:* Resolve the path canonicalization each tool uses — confirm it is the shared Task 34 canonicalizer, not a per-tool ad-hoc normalization. A per-tool variant would diverge from the policy matcher.
  - *Evidence:* `builtin::file::confinement` passed 3/3 live matrices across all 18
    registrations. Traversal, live absolute, and symlink-file/symlink-directory attacks reach
    the shared Task 34 sandbox but not the workspace executor; artifact tools reach their
    explicit capability and reject. Five apply-patch path variants per matrix and pathless
    Glob/Grep are covered. Seven deterministic ordinary-write races and patch transaction races
    preserve interlopers or retain bounded recovery evidence.
  - *Status:* ☑ SATISFIED

- **O4 — Grep clamps at 10,000 model-facing characters and the other families at 30,000/32,000, with overflow written to a file**
  - *Claim:* A large grep result is clamped at the grep-family bound while a large read is clamped at the read-family bound.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::file::clamps)'` — expect the grep and read cases with the two distinct bounds, both routed through the Task 33 clamp module.
  - *Evidence:* `builtin::file::clamps` passed 3/3 through the real pipeline. Exact raw
    overflow was preserved byte-for-byte: Read 80,314 → 30,226 characters, Grep 124,892 →
    10,228, and unclassified LS 50,399 → 32,000. Persisted, emitted, and returned outcomes
    were identical after the single Task 33 clamp stage.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* formatting, strict workspace Clippy, private warnings-denied workspace Rustdoc,
    `cargo deny check`, and the all-feature workspace suite passed; the suite result was
    1,200/1,200. Twenty-four Task 37 production-wired below/at/above tests cover named limits,
    including real 10 MiB Read and actual 1,024-operation patch parsing. Source scans found no
    Task 37 file ≥1,000 lines, function >70 lines, line >100 columns, or lint escape.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(builtin::file::)'` and sees ten tools execute, baseline names match, confinement reject traversal and symlinks, and family clamps apply**
  - *Claim:* The file built-in module passes with all ten tools covered.
  - *Evidence to collect:* Run the filter and confirm zero failures and at least ten execution cases.
  - *Evidence:* A fresh distinct OpenAI reviewer read the complete 6,013-line diff, canonical
    specs, Tasks 32–35 seams, and pinned `300f923f` implementations/assets; it independently
    ran `builtin::file::` 62/62 and `lotta-tools` 194/194 and returned `CORRECT / DONE`, high
    confidence, with no findings or remainder.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/pipeline.rs` (Task 33) executes these tools; confirm
  `pipeline::stage_order` and `clamp::` still pass with real executors attached:
  ☑ PRESERVED — the touched pipeline/security/registry selector passed 105/105; the final
  reviewer independently passed the Task 33 pipeline/clamp/limits selector 26/26.

## Residue

Files WebSocket commands (search, grep, tree, watch, unwatch) are Task 65 and are listener services, not model-facing tools.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All six obligations are satisfied with direct execution, baseline-name, confinement,
clamp, boundary, repository-gate, and independent-review evidence. The ten native file-tool
families are Rust-owned, sequential, capability-confined, pipeline-routed, and pinned-compatible.
