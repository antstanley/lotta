# Task 67 — WebSocket models and provider command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [67-ws_models_and_provider_commands-certificate.md](67-ws_models_and_provider_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 20, 47, 52
**Produces:** list models, list/connect/disconnect providers, usage read, and update model/toolset commands over the Task 47 and 52 surfaces
**Pointers:** `crates/lotta-app-server/src/ws/groups/models.rs`; reference: `../letta-code/src/websocket/listener/commands/model-catalog.ts`, `commands/connect-providers.ts`, `commands/model-toolset.ts`, `commands/chatgpt-usage.ts`

## Steps

- [ ] Decode and route the six commands of the §WebSocket command groups Models/providers row
- [ ] Report connection readiness in `list_models` and redact credentials from every response
- [ ] Route connect and disconnect through Task 52, including the forced-disconnect path
- [ ] Route update model and update toolset through Tasks 47 and 32, validating availability before persistence
- [ ] Emit a provider update snapshot after a mutating command
- [ ] Round-trip every discriminant in the group against the protocol fixture

## Definition of done

- [ ] All six commands decode, route, and respond, with fixture round-trip coverage
- [ ] No response in this group contains a credential, and `list_models` reports readiness
- [ ] Disconnect refuses during an active turn unless forced, and the forced path cancels affected turns first
- [ ] Update model validates availability before persistence and update toolset resolves through the Task 32 registry
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::models::)'` and sees six commands, credential-free responses, guarded disconnect, and validated updates pass
