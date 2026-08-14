# Task 39 — Memory and worktree built-in tools

**Plan:** [plan.md](../plan.md) · **Certificate:** [39-builtin_memory_worktree_tools-certificate.md](39-builtin_memory_worktree_tools-certificate.md)

**Implements:** [05-tools-and-extensions.md §Rust built-ins](../../../05-tools-and-extensions.md#rust-built-ins)
**Depends on:** 29, 33, 34, 35
**Produces:** memory edit and patch operations with Git commit and path confinement, plus worktree enter/exit with an ownership lock and provisioning
**Pointers:** `crates/lotta-tools/src/builtin/memory/`, `builtin/worktree/`; reference: `../letta-code/src/tools/impl/memory.ts`, `memory-apply-patch.ts`, `enter-worktree.ts`, `exit-worktree.ts`, `worktree-git.ts`, `../letta-code/src/websocket/listener/worktree-ownership.ts`

## Steps

- [ ] Implement memory edit and memory apply-patch operations that commit through the Task 29 MemFS port
- [ ] Confine every memory path below the agent's memory root, reusing the Task 34 canonicalizer
- [ ] Implement worktree enter and exit with an ownership lock so two turns cannot hold one worktree
- [ ] Provision includes, hooks, and settings into a newly entered worktree
- [ ] Classify both families as sequential-only, since they mutate shared Git state
- [ ] Add negative tests for path escape, concurrent worktree entry, and commit failure

## Definition of done

- [ ] Memory edit and apply-patch mutate through the MemFS port and produce a Git commit, with the working tree left clean
- [ ] Every memory path is confined below the agent memory root; `..`, absolute paths, and symlink escapes are rejected
- [ ] Worktree enter takes an ownership lock that a second concurrent enter cannot acquire, and exit releases it
- [ ] Entering a worktree provisions includes, hooks, and settings, and both families are classified sequential
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(builtin::memory::) + test(builtin::worktree::)'` and sees commits, path confinement, the ownership lock, and provisioning pass
