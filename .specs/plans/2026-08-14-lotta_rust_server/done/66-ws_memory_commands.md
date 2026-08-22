# Task 66 — WebSocket memory command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [66-ws_memory_commands-certificate.md](66-ws_memory_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 20, 29, 30
**Produces:** list, history, read-at-ref, read, write, delete, diff, and enable MemFS commands over the Task 29 repository
**Pointers:** `crates/lotta-app-server/src/ws/groups/memory.rs`; reference: `../letta-code/src/websocket/listener/commands/memory.ts`, `../letta-code/src/websocket/listener/memfs-sync.ts`, `../letta-code/src/websocket/listener/commands/memory-write-push-ordering.test.ts`

## Steps

- [x] Decode and route the eight memory commands of the §WebSocket command groups Memory row
- [x] Serve read-at-ref and history from the Git revision graph
- [x] Apply memory path confinement identical to the memory tools
- [x] Emit a memory update snapshot after a mutating command
- [x] Order a write and its optional post-commit push so the push follows the commit
- [x] Round-trip every discriminant in the group against the protocol fixture

## Definition of done

- [x] All eight memory commands decode, route, and respond, with fixture round-trip coverage
- [x] Read-at-ref and history resolve against the Git revision graph and reject an unknown revision
- [x] Memory paths are confined identically to the memory tools
- [x] A mutating command emits a memory update snapshot, and a write's optional push follows its commit
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::memory::)'` and sees eight commands, revision reads, shared confinement, and write-then-push ordering pass
