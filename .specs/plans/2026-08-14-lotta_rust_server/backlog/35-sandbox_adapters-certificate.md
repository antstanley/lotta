# Done Certificate — Task 35: OS sandbox adapters and workspace confinement

**Task:** [35-sandbox_adapters.md](35-sandbox_adapters.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 35. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 35) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** macOS Seatbelt and Linux Bubblewrap adapters plus an explicit unsupported error, with workspace roots confining all filesystem tools and child processes.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not fall back to unsandboxed execution: `05-tools-and-extensions.md` §Permissions and sandbox requires an explicit unsupported error on platforms without an enabled sandbox.

## Obligations

- **O1 — Both OS adapters enforce confinement and an unsupported platform returns an explicit typed error instead of running unsandboxed**
  - *Claim:* Seatbelt and Bubblewrap each block an out-of-root read; on a platform with neither, the sandbox port returns an unsupported error and the tool does not execute.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(sandbox::adapters)'` — expect a per-adapter blocking case (skipped with a recorded reason off-platform) and `unsupported_platform_errors_before_exec`, the last asserting the executor was never invoked.
  - *Status:* ☐ unverified

- **O2 — Workspace sandbox roots constrain every filesystem tool and every child process, and peer workspaces below an isolation root are hidden**
  - *Claim:* A read outside the root fails for both a direct file tool and a shell child; a peer workspace directory is not listable.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(sandbox::workspace)'` — expect `file_tool_confined`, `child_process_confined`, and `peer_workspace_hidden` to pass.
  - *Checks:* Resolve the root used by the child-process launcher — confirm it is the workspace sandbox root from the runtime scope, not the process cwd. `NAME SHADOWING` risk: a local `root` binding in the launcher must not shadow the scope's sandbox root.
  - *Status:* ☐ unverified

- **O3 — Every `unsafe` block in the workspace is inside this adapter and carries a `// SAFETY:` proof, an invariant test, and a named review owner**
  - *Claim:* Grep finds `unsafe` only under `crates/lotta-tools/src/sandbox/`, each occurrence preceded by a SAFETY comment.
  - *Evidence to collect:* Grep the workspace for `unsafe ` — expect matches only under `sandbox/`. For each, read the preceding comment and confirm it states the invariant, and find the invariant test that exercises it. Confirm the crate root documents the exception per `architecture-principles.md` §Unsafe code.
  - *Status:* ☐ unverified

- **O4 — Sandbox denial produces the `ToolOutcome` sandbox-denial variant, distinct from a permission denial**
  - *Claim:* A tool blocked by the OS sandbox returns sandbox denial, while one blocked by policy returns user denial or a denied result.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(sandbox::outcome_kind)'` — expect PASS distinguishing the two outcomes from Task 08's `ToolOutcome`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(sandbox::)'` on macOS or Linux and sees confinement, peer hiding, unsupported-platform erroring, and distinct sandbox-denial outcomes pass**
  - *Claim:* The sandbox module passes on at least one supported platform with the unsupported path also covered.
  - *Evidence to collect:* Run the filter and confirm zero failures; confirm any platform-skipped case records its reason rather than silently passing.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/permissions/` (Task 34) decides policy before this gate; confirm `permissions::analyzer` still passes with the sandbox adapter attached : ☐ (PRESERVED / REGRESSION)

## Residue

`00-overview.md` §Non-goals excludes multi-tenant hostile-code isolation; these adapters constrain tools, not a hostile tenant.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
