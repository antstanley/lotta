# Done Certificate — Task 29: MemFS Git repositories and path normalization

**Task:** [29-memfs_git_repositories.md](29-memfs_git_repositories.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 29. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 29) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** one Git repository per agent under the backend root with the baseline label normalization and the full operation set.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not contact Letta Cloud: `04-persistence-and-memfs.md` §MemFS states local backend MemFS has no implicit Letta remote and the server does not synchronize memory with Cloud.

## Obligations

- **O1 — Label normalization matches the baseline: `system/` prefixes are preserved, backslashes and trailing `.md` are normalized, empty/absolute labels collapse, and `.`/`..` segments are rejected**
  - *Claim:* Each normalization rule has a positive and a negative case matching `../letta-code/src/agent/memory-filesystem.ts`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-memfs -E 'test(labels::)'` — expect five rule cases plus `rejects_dot_segments`. Trace: `"notes\\daily.md"` → `system/notes/daily.md`; `"../escape"` → rejected.
  - *Checks:* Resolve the path-join call in the renderer — confirm it operates on the normalized label, not the raw input. A join before normalization would allow traversal outside `memory/`.
  - *Status:* ☐ unverified

- **O2 — Every generated file carries non-empty YAML frontmatter, with a missing description defaulting to `Memory block <label>`**
  - *Claim:* A memory block created without a description still renders valid frontmatter with the fallback text.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-memfs -E 'test(repo::frontmatter_fallback)'` — expect PASS; the test parses the rendered YAML and asserts the description equals `Memory block <label>`.
  - *Status:* ☐ unverified

- **O3 — The full operation set works: status, tree, read, write, delete, rename, history, file-at-revision, diff, commit, worktree reflection/merge, pre-commit validation, and optional post-commit push**
  - *Claim:* Each operation has a passing test and pre-commit validation rejects invalid memory Markdown.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-memfs -E 'test(ops::)'` — expect one case per operation (thirteen) plus `pre_commit_rejects_invalid_markdown`.
  - *Status:* ☐ unverified

- **O4 — No implicit Letta remote is configured, and `MEMORY_FILE_BYTES_MAX`/`MEMORY_FILES_MAX` reject at the limit**
  - *Claim:* A freshly initialized repository has no remote unless the user configured one, and both bounds reject above the limit.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-memfs -E 'test(repo::no_implicit_remote) + test(repo::bounds)'` — expect the remote case to assert `git remote` output is empty, and below/at/above cases for both bounds.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-memfs -E 'test(labels::) + test(ops::) + test(repo::)'` and sees normalization rules, the thirteen operations, frontmatter fallback, no implicit remote, and both bounds pass**
  - *Claim:* The memfs crate's repository and label modules pass.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `no_implicit_remote` case in the summary.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-testkit/src/contract/` (Task 09) `MemFsPort` suite now runs against this adapter; confirm it passes for both the fake and the Git-backed implementation : ☐ (PRESERVED / REGRESSION)

## Residue

Prompt freshness is detected by committed revision at compilation time (Task 30), not by a post-commit hook.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
