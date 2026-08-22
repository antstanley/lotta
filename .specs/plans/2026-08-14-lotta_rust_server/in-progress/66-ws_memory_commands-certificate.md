# Done Certificate — Task 66: WebSocket memory command group

**Task:** [66-ws_memory_commands.md](66-ws_memory_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 66. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 66) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** list, history, read-at-ref, read, write, delete, diff, and enable MemFS commands over the Task 29 repository.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not push before commit: the baseline ordering test pins push after commit, and reversing it would publish an uncommitted state.

## Obligations

- **O1 — All eight memory commands decode, route, and respond, with fixture round-trip coverage**
  - *Claim:* Each command in the Memory row has a passing case.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::memory::commands)'` — expect eight cases derived from the fixture group listing.
  - *Status:* ☐ unverified

- **O2 — Read-at-ref and history resolve against the Git revision graph and reject an unknown revision**
  - *Claim:* A valid revision returns the file at that revision; an unknown revision returns a typed error.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::memory::revisions)'` — expect `reads_at_ref`, `history_lists_revisions`, and `unknown_ref_is_error`.
  - *Status:* ☐ unverified

- **O3 — Memory paths are confined identically to the memory tools**
  - *Claim:* The same traversal and absolute-path rejections apply.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::memory::confinement)'` — expect the same rejection classes as `builtin::memory::confinement`.
  - *Checks:* Resolve the confinement root — confirm it is the agent memory root, matching Task 39, not the workspace sandbox root.
  - *Status:* ☐ unverified

- **O4 — A mutating command emits a memory update snapshot, and a write's optional push follows its commit**
  - *Claim:* Write, delete, and enable each emit a snapshot, and the push is ordered after the commit.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::memory::snapshots) + test(ws::memory::write_push_ordering)'` — expect three snapshot cases and an ordering assertion matching `../letta-code/src/websocket/listener/commands/memory-write-push-ordering.test.ts`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::memory::)'` and sees eight commands, revision reads, shared confinement, and write-then-push ordering pass**
  - *Claim:* The memory group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and eight command cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-memfs/src/repo.rs` (Task 29) serves these commands; confirm `ops::` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Prompt recompilation triggered by a committed memory change is Task 58; this group only mutates and reports.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
