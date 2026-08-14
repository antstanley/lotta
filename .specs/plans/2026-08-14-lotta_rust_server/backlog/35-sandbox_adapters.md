# Task 35 — OS sandbox adapters and workspace confinement

**Plan:** [plan.md](../plan.md) · **Certificate:** [35-sandbox_adapters-certificate.md](35-sandbox_adapters-certificate.md)

**Implements:** [05-tools-and-extensions.md §Permissions and sandbox](../../../05-tools-and-extensions.md#permissions-and-sandbox) · [architecture-principles.md §Unsafe code](../../../architecture-principles.md#unsafe-code)
**Depends on:** 34
**Produces:** macOS Seatbelt and Linux Bubblewrap adapters plus an explicit unsupported error, with workspace roots confining all filesystem tools and child processes
**Pointers:** `crates/lotta-tools/src/sandbox/seatbelt.rs`, `sandbox/bwrap.rs`, `sandbox/unsupported.rs`, `sandbox/workspace.rs`; reference: `../letta-code/src/sandbox/seatbelt.ts`, `../letta-code/src/sandbox/bwrap.ts`, `../letta-code/src/sandbox/policy.ts`, `../letta-code/src/sandbox/availability.ts`, `../letta-code/src/tools/impl/shell-sandbox.ts`

## Steps

- [ ] Implement the macOS Seatbelt adapter and the Linux Bubblewrap adapter behind one sandbox port
- [ ] Return an explicit unsupported error on platforms without an enabled sandbox rather than silently running unsandboxed
- [ ] Confine all filesystem tools and child processes to the workspace sandbox root
- [ ] Hide peer workspaces below an isolation root
- [ ] Keep any `unsafe` block inside this adapter, each with a `// SAFETY:` proof, invariant tests, and named review ownership
- [ ] Add negative tests proving a child process cannot read outside the root under each adapter

## Definition of done

- [ ] Both OS adapters enforce confinement and an unsupported platform returns an explicit typed error instead of running unsandboxed
- [ ] Workspace sandbox roots constrain every filesystem tool and every child process, and peer workspaces below an isolation root are hidden
- [ ] Every `unsafe` block in the workspace is inside this adapter and carries a `// SAFETY:` proof, an invariant test, and a named review owner
- [ ] Sandbox denial produces the `ToolOutcome` sandbox-denial variant, distinct from a permission denial
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(sandbox::)'` on macOS or Linux and sees confinement, peer hiding, unsupported-platform erroring, and distinct sandbox-denial outcomes pass
