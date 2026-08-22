# Task 63 — WebSocket teleport command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [63-ws_teleport_commands-certificate.md](63-ws_teleport_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 20
**Produces:** `teleport_probe`/`teleport_request`/`teleport_failed` with the two response messages and `input.kind = teleport_continue` continuation
**Pointers:** `crates/lotta-app-server/src/ws/groups/teleport.rs`; reference: `../letta-code/src/websocket/listener/teleport.ts`, `../letta-code/src/websocket/listener/teleport-protocol-inbound.ts`

## Steps

- [x] Decode `teleport_probe`, `teleport_request`, and `teleport_failed`
- [x] Emit `teleport_probe_response` and `teleport_ready`
- [x] Route `input` with `kind = teleport_continue` as a continuation on the active lease rather than a new admission
- [x] Reject a continuation whose lease generation is stale
- [x] Emit `teleport_failed` handling without leaving the runtime in a non-terminal state
- [x] Round-trip every discriminant in the group against the protocol fixture

## Definition of done

- [x] The three commands and two messages decode and encode with the baseline discriminants
- [x] `input.kind = teleport_continue` is treated as a continuation on the active lease, not a new admission
- [x] A continuation carrying a stale lease generation is rejected and emits nothing
- [x] `teleport_failed` leaves the runtime in a defined state with no dangling teleport
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::teleport::)'` and sees the five discriminants, continuation-not-admission, stale-lease rejection, and failure cleanup pass
