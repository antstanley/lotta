# Done Certificate — Task 65: WebSocket files command group

**Task:** [65-ws_files_commands.md](65-ws_files_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 65. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 65) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** search, grep, list directory, tree, read, write, edit, watch, unwatch, and file operations as listener services under the same path policy as file tools.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must share the tool path policy: divergent confinement between the command surface and the tool surface would let a client reach paths a tool cannot.

## Obligations

- **O1 — All ten file commands decode, route, and respond, with round-trip coverage against the protocol fixture**
  - *Claim:* Each command in the §WebSocket command groups Files row has a passing case.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::files::commands)'` — expect ten cases derived from the fixture group listing.
  - *Status:* ☐ unverified

- **O2 — Every path-taking file command is confined by the same policy and workspace root as the file tools**
  - *Claim:* A traversal, a symlink escape, and an out-of-root absolute path are rejected identically to the tool path.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::files::confinement)'` — expect three rejection cases per path-taking command.
  - *Checks:* Resolve the canonicalizer and permission checker used here — confirm they are the shared Task 34 units, not a listener-local copy. A separate copy would let the command surface diverge from the tool surface.
  - *Status:* ☐ unverified

- **O3 — Watch and unwatch maintain a bounded watcher set per connection and clean up on close**
  - *Claim:* Watchers are capped, unwatch removes one, and closing a connection removes all of its watchers.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::files::watchers)'` — expect `bounded`, `unwatch_removes`, and `close_removes_all` to pass.
  - *Status:* ☐ unverified

- **O4 — Large results are bounded so no response exceeds `WS_FRAME_BYTES_MAX`**
  - *Claim:* A tree or grep over a large directory produces a bounded response rather than an oversized frame.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::files::result_bounds)'` — expect PASS asserting the encoded response size stays under the Task 15 bound.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::files::)'` and sees ten commands, shared confinement, bounded watchers, and bounded results pass**
  - *Claim:* The files group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and ten command cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/builtin/file/` (Task 37) shares the canonicalizer; confirm `builtin::file::confinement` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

File operations here are listener services, not model-facing tools; `05-tools-and-extensions.md` §Assumptions *Tool parity* keeps that distinction.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
