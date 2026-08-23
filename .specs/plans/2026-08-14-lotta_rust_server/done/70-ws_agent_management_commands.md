# Task 70 — WebSocket agent management command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [70-ws_agent_management_commands-certificate.md](70-ws_agent_management_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups) · [01-domain-model.md §Agent](../../../01-domain-model.md#agent)
**Depends on:** 20, 23, 28
**Produces:** create shortcut plus list, retrieve, create, update, and delete for agents, backed by the Task 28 query behaviors
**Pointers:** `crates/lotta-app-server/src/ws/groups/agents.rs`; reference: `../letta-code/src/websocket/listener/commands/agents-conversations.ts`, `../letta-code/src/backend/local/local-backend.ts`

## Steps

- [ ] Decode and route the six commands of the §WebSocket command groups Agent management row
- [ ] Create an agent with local MemFS enabled: stamp the Git-memory tag, initialize memory files from `memory_blocks`, create the repository, and compile the default conversation prompt before returning success
- [ ] Serve list with the deterministic ordering and name/query/tag/hidden filters of §Required query patterns
- [ ] Return 404 on an absent agent or a wrong local prefix
- [ ] Enforce `AGENTS_MAX` and emit an agent update snapshot on mutation
- [ ] Round-trip every discriminant in the group against the protocol fixture

## Definition of done

- [ ] All six agent management commands decode, route, and respond, with fixture round-trip coverage
- [ ] Agent creation with local MemFS completes all four side effects before returning success
- [ ] List ordering is deterministic and the filters behave as §Required query patterns specifies
- [ ] An absent agent or a wrong local prefix returns 404, and `AGENTS_MAX` rejects creation at the limit
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::agents::)'` and sees six commands, complete creation side effects, deterministic filtered listing, and the 404 and cap paths pass
