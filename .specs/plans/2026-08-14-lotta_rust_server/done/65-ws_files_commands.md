# Task 65 — WebSocket files command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [65-ws_files_commands-certificate.md](65-ws_files_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 20, 37
**Produces:** search, grep, list directory, tree, read, write, edit, watch, unwatch, and file operations as listener services under the same path policy as file tools
**Pointers:** `crates/lotta-app-server/src/ws/groups/files.rs`; reference: `../letta-code/src/websocket/listener/file-commands.ts`, `../letta-code/src/websocket/listener/grep-in-files.ts`

## Steps

- [x] Decode and route the ten file commands named in `02-app-server-api.md` §WebSocket command groups
- [x] Apply the Task 34 permission policy and Task 35 workspace confinement to every path
- [x] Implement watch and unwatch with a bounded watcher set per connection
- [x] Bound result sizes so a large tree or grep cannot exceed the transport frame bound
- [x] Clean up watchers on connection close
- [x] Round-trip every discriminant in the group against the protocol fixture

## Definition of done

- [x] All ten file commands decode, route, and respond, with round-trip coverage against the protocol fixture
- [x] Every path-taking file command is confined by the same policy and workspace root as the file tools
- [x] Watch and unwatch maintain a bounded watcher set per connection and clean up on close
- [x] Large results are bounded so no response exceeds `WS_FRAME_BYTES_MAX`
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::files::)'` and sees ten commands, shared confinement, bounded watchers, and bounded results pass
