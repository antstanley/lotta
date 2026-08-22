# Done Certificate — Task 66: WebSocket memory command group

**Task:** [66-ws_memory_commands.md](66-ws_memory_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-22

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
  - *Evidence:* Exact selector passed 8/8, one substantive case per command. Each drives a raw frame through the real decode/apply path and asserts field-level response JSON matching the pinned protocol types; command and response discriminants are verified against `fixtures/protocol/discriminants.json`.
  - *Status:* ☑ SATISFIED

- **O2 — Read-at-ref and history resolve against the Git revision graph and reject an unknown revision**
  - *Claim:* A valid revision returns the file at that revision; an unknown revision returns a typed error.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::memory::revisions)'` — expect `reads_at_ref`, `history_lists_revisions`, and `unknown_ref_is_error`.
  - *Evidence:* Exact selector passed 3/3 (`reads_at_ref`, `history_lists_revisions`, `unknown_ref_is_error`). Reads trace through the Task 29 Git revision graph (`file_at_revision` → `resolve_revision` → `git show`; history via `git log`), never local files; unknown revisions return typed errors on both the at-ref and diff paths.
  - *Status:* ☑ SATISFIED

- **O3 — Memory paths are confined identically to the memory tools**
  - *Claim:* The same traversal and absolute-path rejections apply.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::memory::confinement)'` — expect the same rejection classes as `builtin::memory::confinement`.
  - *Checks:* Resolve the confinement root — confirm it is the agent memory root, matching Task 39, not the workspace sandbox root.
  - *Evidence:* Exact selector passed 3/3 with the same rejection classes as `builtin::memory::confinement`. Confinement uses the shared `RepositoryPath` lexical unit (not a listener-local copy), the root is the agent memory root `<backend>/memfs/<agent>/memory` — never the workspace sandbox — and symlink enforcement rides the Task 29 no-follow layer, traced end-to-end from wire frame to typed error.
  - *Status:* ☑ SATISFIED

- **O4 — A mutating command emits a memory update snapshot, and a write's optional push follows its commit**
  - *Claim:* Write, delete, and enable each emit a snapshot, and the push is ordered after the commit.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::memory::snapshots) + test(ws::memory::write_push_ordering)'` — expect three snapshot cases and an ordering assertion matching `../letta-code/src/websocket/listener/commands/memory-write-push-ordering.test.ts`.
  - *Evidence:* Exact selector passed 6/6 (three snapshots + three ordering). Write/delete emit the snapshot before the response and enable answers then snapshots `["*"]`, matching pinned order. The push runs strictly between commit and notify (remote head equals the response sha at notify time), and a failed push leaves the commit intact with success answered.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace rustdoc with warnings denied, and `cargo deny check` passed; full lotta-app-server 284/284 and lotta-memfs 83/83. Workspace health excluding only the recorded external pinned-SHA/CLI family is green. New functions stay within 70 lines and 100 columns; constants use units-last names; no production unwrap/expect and no new lint suppressions.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::memory::)'` and sees eight commands, revision reads, shared confinement, and write-then-push ordering pass**
  - *Claim:* The memory group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and eight command cases.
  - *Evidence:* Broad selector passed 20/20 with zero failures covering eight commands, revision reads, shared confinement, snapshots, and write-then-push ordering. Independent review returned `CORRECT / DONE`; its flagged backend-root wiring defect was fixed with a loud regression guard before certification.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-memfs/src/repo.rs` (Task 29) serves these commands; confirm `ops::` still passes : ☑ PRESERVED

## Residue

Prompt recompilation triggered by a committed memory change is Task 58; this group only mutates and reports.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 obligations are discharged by exact selectors and traced evidence. The eight memory commands decode, route, and respond in pinned shapes with fixture coverage; read-at-ref and history resolve through the Task 29 Git revision graph with typed unknown-revision errors; memory paths are confined by the shared Task 39 unit at the agent memory root with no-follow symlink enforcement; every mutation emits its snapshot in pinned order; and a write's push runs strictly after its commit, surviving push failure. The backend root wiring was corrected to a single memfs segment with a regression guard, and the Task 29 MemFS regression suite is preserved.
