# Task 72 — Device commands, introspection, and outbound message groups

**Plan:** [plan.md](../plan.md) · **Certificate:** [72-ws_device_introspection_and_outbound_groups-certificate.md](72-ws_device_introspection_and_outbound_groups-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups) · [02-app-server-api.md §Outbound message groups](../../../02-app-server-api.md#outbound-message-groups)
**Depends on:** 18, 20, 44, 45
**Produces:** slash/mod command execution, remove queue item, branch search/checkout, secret list/apply, `app_server_info`, and coverage of all six outbound message groups
**Pointers:** `crates/lotta-app-server/src/ws/groups/device.rs`, `ws/groups/introspection.rs`, `ws/outbound.rs`; reference: `../letta-code/src/websocket/listener/commands/app-server-info.ts`, `commands/git-branches.ts`, `commands/secrets.ts`, `../letta-code/src/websocket/listener/mod-commands.ts`, `background-process-snapshot.ts`, `../letta-code/src/types/app-server-info.ts`

## Steps

- [ ] Decode and route the five device commands: execute slash/mod command, remove queue item, branch search, branch checkout, and secret list/apply
- [ ] Route remove-queue-item through the Task 18 queue so the wire disposition and snapshot are emitted
- [ ] Route slash and mod command execution through the Task 45 mod command registry
- [ ] Implement `app_server_info` reporting `APP_SERVER_PROTOCOL_VERSION = 1` and the supported command set
- [ ] Emit background-process snapshots as listener state messages rather than model-facing tools
- [ ] Assert every §Outbound message groups row has at least one emitted message type covered by a test

## Definition of done

- [ ] All five device commands decode, route, and respond, with fixture round-trip coverage
- [ ] Remove-queue-item goes through the queue and emits the `dequeued` or `cancelled` disposition with an authoritative snapshot
- [ ] `app_server_info` reports protocol version 1 and the supported command set, and requires authentication
- [ ] Every §Outbound message groups row has at least one message type emitted and covered by a test
- [ ] Background-process snapshots are emitted as listener state messages and are not registered as model-facing tools
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::device::) + test(ws::introspection::) + test(ws::outbound::)'` and sees five device commands, queue-routed removal, protocol version 1, and all six outbound groups covered
