# Done Certificate — Task 39: Memory and worktree built-in tools

**Task:** [39-builtin_memory_worktree_tools.md](39-builtin_memory_worktree_tools.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 39. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 39) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** memory edit and patch operations with Git commit and path confinement, plus worktree enter/exit with an ownership lock and provisioning.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not expose Letta-Cloud-shaped memory tools: `05-tools-and-extensions.md` §Rust built-ins defines the Memory family as memory edit and patch operations with Git commit and path confinement.

## Obligations

- **O1 — Memory edit and apply-patch mutate through the MemFS port and produce a Git commit, with the working tree left clean**
  - *Claim:* Each operation results in exactly one commit whose tree contains the edit, and `git status` is clean afterwards.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::memory::commits)'` — expect one case per operation asserting commit count and clean status.
  - *Status:* ☐ unverified

- **O2 — Every memory path is confined below the agent memory root; `..`, absolute paths, and symlink escapes are rejected**
  - *Claim:* The three escape classes fail before any write.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::memory::confinement)'` — expect three negative cases, each asserting zero writes reached the port.
  - *Checks:* Resolve the confinement root — confirm it is the agent's `memfs/<agent-id>/memory/` root from Task 29, not the workspace sandbox root. The two roots differ and using the wrong one would let a memory tool write into the workspace.
  - *Status:* ☐ unverified

- **O3 — Worktree enter takes an ownership lock that a second concurrent enter cannot acquire, and exit releases it**
  - *Claim:* A second enter for the same worktree fails while the first holds the lock, and succeeds after exit.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::worktree::ownership_lock)'` — expect `second_enter_blocked` and `enter_after_exit_succeeds` to pass.
  - *Status:* ☐ unverified

- **O4 — Entering a worktree provisions includes, hooks, and settings, and both families are classified sequential**
  - *Claim:* A newly entered worktree contains the provisioned artifacts, and both tool families report the sequential parallel-safety classification.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(builtin::worktree::provisioning) + test(builtin::memory::is_sequential)'` — expect both PASS; the second reads the Task 08 classification field.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(builtin::memory::) + test(builtin::worktree::)'` and sees commits, path confinement, the ownership lock, and provisioning pass**
  - *Claim:* The memory and worktree built-in modules pass.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `ownership_lock` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-memfs/src/repo.rs` (Task 29) performs the commits; confirm `ops::` still passes with tool-driven writes : ☐ (PRESERVED / REGRESSION)

## Residue

Reflection through a memory worktree with merge-under-lock is a subagent behavior (Task 46); this task provides the worktree primitives it uses.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
