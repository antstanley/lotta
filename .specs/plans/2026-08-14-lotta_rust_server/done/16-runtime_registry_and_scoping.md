# Task 16 — Runtime registry, scoping, and immediate quiescent eviction

**Plan:** [plan.md](../plan.md) · **Certificate:** [16-runtime_registry_and_scoping-certificate.md](16-runtime_registry_and_scoping-certificate.md)

**Implements:** [03-runtime-and-turns.md §Runtime registry](../../../03-runtime-and-turns.md#runtime-registry) · [03-runtime-and-turns.md §Responsibilities](../../../03-runtime-and-turns.md#responsibilities)
**Depends on:** 06, 09
**Produces:** a `(AgentId, ConversationId)`-keyed registry with idempotent creation, an explicit residency predicate, and immediate eviction once quiescent
**Pointers:** `crates/lotta-runtime/src/registry.rs`, `src/scope_context.rs`, `src/worktree_watcher.rs`; reference: `../letta-code/src/websocket/listener/runtime.ts`, `../letta-code/src/websocket/listener/conversation-runtime.ts`, `../letta-code/src/websocket/listener/scope.ts`, `../letta-code/src/websocket/listener/worktree-watcher.ts`

## Steps

- [x] Implement one process-wide `ListenerRuntime` owning a map keyed by `(AgentId, ConversationId)`
- [x] Make runtime creation idempotent: a second start for the same scope returns the existing handle
- [x] Implement the residency predicate from `03-runtime-and-turns.md` §Runtime registry — resident while lifecycle, queue, approval, interrupted-result, or sandbox-subscription state requires it
- [x] Evict a scope immediately once quiescent, with no idle timer on the runtime itself
- [x] Give the worktree watcher its own separate 30-minute idle stop
- [x] Carry per-turn ambient data in Tokio task-local context and pass an explicit snapshot to every spawned task; enforce `RUNTIMES_MAX`

## Definition of done

- [x] The registry is keyed by the `(AgentId, ConversationId)` pair, creation is idempotent, and `RUNTIMES_MAX` rejects `runtime_start` at the limit
- [x] A quiescent runtime is evicted immediately, and residency is decided by the five-part predicate rather than an idle timer
- [x] The worktree watcher has its own 30-minute idle stop, independent of runtime residency
- [x] Per-turn ambient data is task-local and every spawned task receives an explicit snapshot; no background task can reach a conversation implicitly
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(registry::) + test(worktree_watcher::)'` and sees idempotent keying, immediate quiescent eviction, the five residency terms, and the watcher's separate idle stop pass
