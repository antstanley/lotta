# Done Certificate — Task 46: Subagents over the bounded sidecar

**Task:** [46-subagents.md](46-subagents.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 46. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 46) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the seven built-in subagent types spawned as a pinned Letta Code subprocess over the shared sidecar, with bounded snapshots and filesystem confinement.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must reuse the Task 42 framing: `05-tools-and-extensions.md` §Subagents calls the sidecar protocol a Lotta adapter contract, not a pre-existing language-neutral port, so a second incompatible framing would be pure divergence.

## Obligations

- **O1 — All seven built-in subagent types exist and each behaves as §Subagents describes for context inheritance**
  - *Claim:* Fork inherits conversation context, general-purpose starts isolated, recall reads historical messages, and reflection edits through a memory worktree merged under a lock.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(subagents::types)'` — expect seven type cases plus the four named context behaviors.
  - *Status:* ☐ unverified

- **O2 — Subagent filesystem and MemFS access is confined to explicit roots, and an escape attempt fails**
  - *Claim:* A subagent cannot read outside its declared roots through either the filesystem or MemFS.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(subagents::confinement)'` — expect a filesystem-escape and a MemFS-escape rejection case.
  - *Checks:* Resolve the reflection subagent's memory root — confirm it is the worktree root from Task 39, not the agent's primary memory root, so a merge conflict cannot corrupt live memory.
  - *Status:* ☐ unverified

- **O3 — Parent status receives bounded state snapshots and stream events, and a silent subagent broadcasts no stream output**
  - *Claim:* Snapshots are bounded in size and a silent subagent produces zero `update_subagent_state` stream deltas.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(subagents::status)'` — expect `snapshot_is_bounded` and `silent_subagent_does_not_broadcast` to pass.
  - *Status:* ☐ unverified

- **O4 — `SUBAGENTS_PER_PARENT_MAX` and `SUBAGENTS_CONCURRENT_PER_PARENT_MAX` both reject at their limits**
  - *Claim:* The 129th subagent for a parent is rejected and the 17th concurrent one waits or is rejected per the bound.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(subagents::bounds)'` — expect below/at/above cases for both constants from `05-tools-and-extensions.md` §Limits.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(subagents::)'` and sees seven types, both confinement rejections, bounded silent snapshots, and both concurrency bounds pass**
  - *Claim:* The subagents module passes over the shared sidecar.
  - *Evidence to collect:* Run the filter and confirm zero failures, then grep `crates/lotta-extensions/src/subagents/` for a private framing implementation — expect none.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-extensions/src/sidecar/` (Task 42) frames subagent traffic; confirm `sidecar::validation` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-tools/src/builtin/worktree/` (Task 39) supplies the reflection worktree; confirm `builtin::worktree::ownership_lock` still passes under concurrent reflection : ☐ (PRESERVED / REGRESSION)

## Residue

`05-tools-and-extensions.md` §Assumptions leaves the parity milestone for replacing the Letta Code subprocess open; this task ships only the compatibility path.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
