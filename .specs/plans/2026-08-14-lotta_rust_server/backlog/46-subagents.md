# Task 46 — Subagents over the bounded sidecar

**Plan:** [plan.md](../plan.md) · **Certificate:** [46-subagents-certificate.md](46-subagents-certificate.md)

**Implements:** [05-tools-and-extensions.md §Subagents](../../../05-tools-and-extensions.md#subagents)
**Depends on:** 29, 32, 39, 42
**Produces:** the seven built-in subagent types spawned as a pinned Letta Code subprocess over the shared sidecar, with bounded snapshots and filesystem confinement
**Pointers:** `crates/lotta-extensions/src/subagents/types.rs`, `subagents/spawn.rs`, `subagents/snapshot.rs`, `subagents/confinement.rs`; reference: `../letta-code/src/agent/subagents`, `../letta-code/src/tools/impl/task.ts`, `../letta-code/src/tools/impl/tasks`

## Steps

- [ ] Define the seven built-in types: general-purpose, fork, recall, reflection, memory, history-analyzer, and init
- [ ] Accept a request carrying type, description, prompt, model policy, background flag, max turns, tools, memory scope, parent scope, and optional existing agent/conversation
- [ ] Spawn a pinned Letta Code subprocess over the Task 42 sidecar as the first compatibility implementation
- [ ] Send bounded state snapshots and stream events to parent status; keep silent subagents from broadcasting stream output
- [ ] Confine subagent filesystem and MemFS access to explicit roots; fork inherits conversation context, general-purpose starts isolated, recall reads historical messages, reflection edits through a memory worktree and merges under a lock
- [ ] Enforce `SUBAGENTS_PER_PARENT_MAX` and `SUBAGENTS_CONCURRENT_PER_PARENT_MAX`

## Definition of done

- [ ] All seven built-in subagent types exist and each behaves as §Subagents describes for context inheritance
- [ ] Subagent filesystem and MemFS access is confined to explicit roots, and an escape attempt fails
- [ ] Parent status receives bounded state snapshots and stream events, and a silent subagent broadcasts no stream output
- [ ] `SUBAGENTS_PER_PARENT_MAX` and `SUBAGENTS_CONCURRENT_PER_PARENT_MAX` both reject at their limits
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(subagents::)'` and sees seven types, both confinement rejections, bounded silent snapshots, and both concurrency bounds pass
