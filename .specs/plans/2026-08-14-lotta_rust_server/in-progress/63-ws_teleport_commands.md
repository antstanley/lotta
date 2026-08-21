# Task 63 — WebSocket teleport command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [63-ws_teleport_commands-certificate.md](63-ws_teleport_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 20
**Produces:** `teleport_probe`/`teleport_request`/`teleport_failed` with the two response messages and `input.kind = teleport_continue` continuation
**Pointers:** `crates/lotta-app-server/src/ws/groups/teleport.rs`; reference: `../letta-code/src/websocket/listener/teleport.ts`, `../letta-code/src/websocket/listener/teleport-protocol-inbound.ts`

## Steps

- [ ] Decode `teleport_probe`, `teleport_request`, and `teleport_failed`
- [ ] Emit `teleport_probe_response` and `teleport_ready`
- [ ] Route `input` with `kind = teleport_continue` as a continuation on the active lease rather than a new admission
- [ ] Reject a continuation whose lease generation is stale
- [ ] Emit `teleport_failed` handling without leaving the runtime in a non-terminal state
- [ ] Round-trip every discriminant in the group against the protocol fixture

## Definition of done

- [ ] The three commands and two messages decode and encode with the baseline discriminants
- [ ] `input.kind = teleport_continue` is treated as a continuation on the active lease, not a new admission
- [ ] A continuation carrying a stale lease generation is rejected and emits nothing
- [ ] `teleport_failed` leaves the runtime in a defined state with no dangling teleport
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::)'` and sees the five discriminants, continuation-not-admission, stale-lease rejection, and failure cleanup pass
