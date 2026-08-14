# Task 20 — WebSocket runtime commands and event envelopes

**Plan:** [plan.md](../plan.md) · **Certificate:** [20-ws_runtime_commands_and_event_envelopes-certificate.md](20-ws_runtime_commands_and_event_envelopes-certificate.md)

**Implements:** [02-app-server-api.md §Core lifecycle](../../../02-app-server-api.md#core-lifecycle) · [02-app-server-api.md §Event envelopes and ordering](../../../02-app-server-api.md#event-envelopes-and-ordering) · [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 10, 15, 16, 18, 19
**Produces:** the Runtime command group decoded, routed, and answered with correctly enveloped, ordered events on a per-connection sequence
**Pointers:** `crates/lotta-app-server/src/ws/router.rs`, `src/ws/envelope.rs`, `src/ws/connection.rs`; reference: `../letta-code/src/websocket/listener/protocol-inbound.ts`, `../letta-code/src/websocket/listener/protocol-outbound.ts`, `../letta-code/src/websocket/listener/outbound-wire.ts`, `../letta-code/src/websocket/listener/connection.ts`

## Steps

- [ ] Route the Runtime group — `runtime_start`, `input`, `sync`, `abort_message`, `change_device_state` — to the process or scoped runtime
- [ ] Validate `runtime_start`'s mutually exclusive choices before allocating anything
- [ ] Emit `input_accepted` carrying `started` or `queued` before any event caused by that input
- [ ] Stamp broadcast frames with `runtime { agent_id, conversation_id, acting_user_id? }`, a per-connection monotonic `event_seq`, RFC3339 `emitted_at`, and a per-emission `idempotency_key`
- [ ] Leave connection-specific and management responses unstamped by the runtime envelope, carrying request correlation only
- [ ] Enforce `RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX` and stable connection-ordinal broadcast order

## Definition of done

- [ ] The five Runtime-group commands decode, route, and answer, and `runtime_start` rejects mutually exclusive choices before allocation
- [ ] The seven broadcast frame types carry the runtime envelope, a monotonic per-connection `event_seq`, `emitted_at`, and a per-emission `idempotency_key`; connection-specific responses carry none of these
- [ ] All six §Event envelopes and ordering invariants hold under test, including `input_accepted` before caused events and exactly-once `turn_finished` after the final delta
- [ ] `RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX` rejects the 257th subscription and broadcast preserves stable connection-ordinal order
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::)'` and sees the Runtime group, envelope stamping, all six ordering invariants, and the subscription cap pass
