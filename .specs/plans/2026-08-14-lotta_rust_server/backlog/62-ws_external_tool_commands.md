# Task 62 — WebSocket external-tool command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [62-ws_external_tool_commands-certificate.md](62-ws_external_tool_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups) · [01-domain-model.md §External tool registration](../../../01-domain-model.md#external-tool-registration)
**Depends on:** 20, 41
**Produces:** runtime external-tool update and external-tool-call response commands routed to the Task 41 registry with correct correlation
**Pointers:** `crates/lotta-app-server/src/ws/groups/external_tools.rs`; reference: `../letta-code/src/websocket/listener/external-tools.ts`, `../letta-code/src/websocket/listener/external-tool-protocol.ts`

## Steps

- [ ] Decode the runtime external-tool update command and apply it as an atomic registration group
- [ ] Emit the external-tool-call request message carrying runtime, request ID, tool call ID, name, arguments, and optional scope ID
- [ ] Decode the external-tool-call response command and resolve the pending call through its originating connection
- [ ] Reject a response whose request ID, tool call ID, or connection does not match the pending call
- [ ] Surface owner disconnect as a typed result on the pending call
- [ ] Add round-trip tests against the Task 10 protocol fixture entries for this group

## Definition of done

- [ ] The external-tool update command applies atomically and the call request message carries all six correlation fields
- [ ] A response resolves only through its originating connection and is rejected on any correlation mismatch
- [ ] Owner disconnect resolves pending calls with the typed owner-disconnected result
- [ ] Every discriminant in this group has a decode round-trip test against `fixtures/protocol/discriminants.json`
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::external_tools::)'` and sees atomic updates, correlation rejection, owner-disconnect resolution, and fixture round-trips pass
