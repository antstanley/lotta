# Task 68 — WebSocket schedules command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [68-ws_schedule_commands-certificate.md](68-ws_schedule_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 20, 60, 61
**Produces:** cron list, add, get, runs, trigger, update, delete, and delete-all commands over the Task 60 store and Task 61 scheduler
**Pointers:** `crates/lotta-app-server/src/ws/groups/schedules.rs`; reference: `../letta-code/src/websocket/listener/commands/cron.ts`, `../letta-code/src/types/schedule-protocol.ts`

## Steps

- [x] Decode and route the eight commands of the §WebSocket command groups Schedules row
- [x] Serve `runs` from the per-schedule run log with bounded output
- [x] Route `trigger` through the Task 61 firing path so it enqueues through the conversation queue
- [x] Validate cron/interval expressions and IANA timezones on add and update
- [x] Emit a cron update snapshot after every mutating command
- [x] Round-trip every discriminant in the group against the protocol fixture

## Definition of done

- [x] All eight schedule commands decode, route, and respond, with fixture round-trip coverage
- [x] `trigger` enqueues through the conversation queue rather than starting a turn directly
- [x] Add and update validate the cron/interval expression and the IANA timezone, rejecting invalid values without persisting
- [x] `runs` returns bounded run history and a mutating command emits a cron update snapshot
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::schedules::)'` and sees eight commands, queue-routed trigger, expression and timezone validation, and bounded runs pass
