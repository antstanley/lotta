# Done Certificate — Task 35: OS sandbox adapters and workspace confinement

**Task:** [35-sandbox_adapters.md](35-sandbox_adapters.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-16

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
  - *Evidence:* The exact selector passed **4/4** on macOS. Real Seatbelt execution made
    `/bin/cat` of a peer secret exit nonzero without emitting the secret, while the same child
    read its own workspace secret and exited zero. Pure tests pin both Seatbelt and Bubblewrap
    argv/mount ordering; concrete Seatbelt paths travel only through `-D` argv definitions.
    Bubblewrap's host case records `NotCurrentPlatform` on macOS and has a Linux-only real case.
    `unsupported_platform_errors_before_exec` returned typed `RuntimeError::Unsupported` and left
    the child marker absent.
  - *Status:* ☑ SATISFIED

- **O2 — Workspace sandbox roots constrain every filesystem tool and every child process, and peer workspaces below an isolation root are hidden**
  - *Claim:* A read outside the root fails for both a direct file tool and a shell child; a peer workspace directory is not listable.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(sandbox::workspace)'` — expect `file_tool_confined`, `child_process_confined`, and `peer_workspace_hidden` to pass.
  - *Checks:* Resolve the root used by the child-process launcher — confirm it is the workspace sandbox root from the runtime scope, not the process cwd. `NAME SHADOWING` risk: a local `root` binding in the launcher must not shadow the scope's sandbox root.
  - *Evidence:* The exact selector passed **7/7**. The real pipeline gate converted peer and
    above-isolation direct reads into durable sandbox-denied outcomes with executor count zero.
    Gemini `dir_path` aliases (`glob_gemini`, `search_file_content`, `list_directory`) and image
    aliases deny peer/outside roots, allow the workspace root, and fail closed on malformed or
    missing paths. Real Seatbelt children could neither read a peer secret nor list the isolation
    root. `WorkspacePolicy` requires canonical existing non-symlink roots with the workspace a
    strict descendant; `validate_request` requires the request's retained confinement root to
    equal that policy root and re-canonicalizes cwd immediately before spawn. Peer and outside cwd
    symlinks fail before the marker process starts.
  - *Status:* ☑ SATISFIED

- **O3 — Every `unsafe` block in the workspace is inside this adapter and carries a `// SAFETY:` proof, an invariant test, and a named review owner**
  - *Claim:* Grep finds `unsafe` only under `crates/lotta-tools/src/sandbox/`, each occurrence preceded by a SAFETY comment.
  - *Evidence to collect:* Grep the workspace for `unsafe ` — expect matches only under `sandbox/`. For each, read the preceding comment and confirm it states the invariant, and find the invariant test that exercises it. Confirm the crate root documents the exception per `architecture-principles.md` §Unsafe code.
  - *Evidence:* Workspace source inspection found **zero operative unsafe blocks**. The adapter
    therefore satisfies this conditional obligation without invoking an exception: `lotta-tools`
    retains `#![forbid(unsafe_code)]`, and its crate documentation records that Seatbelt and
    Bubblewrap use only safe Tokio process APIs. The clean named Task 35 reviewer confirmed no
    SAFETY proof or unsafe invariant test is required when no unsafe block exists.
  - *Status:* ☑ SATISFIED

- **O4 — Sandbox denial produces the `ToolOutcome` sandbox-denial variant, distinct from a permission denial**
  - *Claim:* A tool blocked by the OS sandbox returns sandbox denial, while one blocked by policy returns user denial or a denied result.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(sandbox::outcome_kind)'` — expect PASS distinguishing the two outcomes from Task 08's `ToolOutcome`.
  - *Evidence:* The exact selector passed **3/3**. Sandbox deny returns, persists, and emits
    `ToolOutcome::SandboxDenied`, records failed post-hook status, and traces Sandbox → PostHook →
    Scrub → Clamp → Persist → Emit with no secret substitution or executor call. Permission deny
    remains `PipelineError::PermissionDenied` and stops before sandbox; sandbox infrastructure
    failure remains `PipelineError::Sandbox` with no executor or sink effect.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Clean validation passed format, warning-denied workspace all-target/all-feature
    Clippy, warning-denied private-item workspace Rustdoc, `cargo deny check`, and full workspace
    all-feature tests **1,100/1,100**. Exact Task 34 analyzer and combined permission regressions
    passed **11/11** and **64/64**. All Task 35 production files remain below 1,000 lines, all
    source lines are at most 100 columns, and no added function exceeds 70 lines (`supervise` is
    69). Allocation, PATH, event-channel, output, stdin, timeout, cancellation, closed-receiver,
    and stdout-before-stdin boundaries have behavioral coverage; limits are named constants with
    units last. Pinned parity provenance remains
    `300f923f16cc8eee50656d7da732902c1dea2b65`, with no new third-party dependency.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(sandbox::)'` on macOS or Linux and sees confinement, peer hiding, unsupported-platform erroring, and distinct sandbox-denial outcomes pass**
  - *Claim:* The sandbox module passes on at least one supported platform with the unsupported path also covered.
  - *Evidence to collect:* Run the filter and confirm zero failures; confirm any platform-skipped case records its reason rather than silently passing.
  - *Evidence:* The exact combined selector passed **20/20** on macOS: adapters **4/4**,
    workspace **7/7**, outcome kinds **3/3**, and bounded process supervision **6/6**. It executed
    the real Seatbelt adapter, recorded Bubblewrap's `NotCurrentPlatform` reason, covered explicit
    unsupported behavior, and observed zero failures. The 131,076-byte stdout-before-stdin case
    completed at exit zero, proving concurrent bounded pipe supervision rather than a vacuous
    renderer-only path.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/permissions/` (Task 34) still decides policy before this gate. The exact
  analyzer selector passed **11/11**, combined permissions passed **64/64**, the allowed pipeline
  retains the ten exact stages, and deny/ask stop before sandbox or any later effect : ☑ PRESERVED

## Residue

`00-overview.md` §Non-goals excludes multi-tenant hostile-code isolation; these adapters constrain tools, not a hostile tenant.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: DONE
CONFIDENCE: high
SUMMARY: All six obligations are satisfied and the Task 34 regression is preserved. Seatbelt
enforces real peer confinement on macOS; Bubblewrap has a pinned pure renderer and Linux-only real
path; unsupported hosts fail closed; authoritative workspace roots constrain direct aliases and
child processes; no unsafe exception is needed; sandbox denial remains a distinct durable outcome;
bounded process supervision and every repository quality gate pass; and the clean reviewer found
no remaining code, security, test, or rubric defect.
