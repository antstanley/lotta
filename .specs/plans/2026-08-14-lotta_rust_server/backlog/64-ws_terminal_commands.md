# Task 64 — WebSocket terminal command group

**Plan:** [plan.md](../plan.md) · **Certificate:** [64-ws_terminal_commands-certificate.md](64-ws_terminal_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 20, 38
**Produces:** `terminal_spawn`/`terminal_input`/`terminal_resize`/`terminal_kill` scoped by connection and `terminal_id`, with the two-second Strict-Mode reuse window
**Pointers:** `crates/lotta-app-server/src/ws/groups/terminal.rs`; reference: `../letta-code/src/websocket/terminal-handler.ts`, `../letta-code/src/websocket/listener/process-services.ts`

## Steps

- [ ] Scope terminal sessions by connection and `terminal_id`
- [ ] Implement spawn with `cols`, `rows`, and optional cwd, emitting `terminal_spawned` on success and `terminal_exited` on spawn failure or exit
- [ ] Emit `terminal_output` for session output
- [ ] Make input and resize for an absent session no-ops
- [ ] Reuse a live session younger than two seconds on repeat spawn and ignore kill during that window, to tolerate React Strict Mode
- [ ] Kill a connection's sessions on connection cleanup

## Definition of done

- [ ] Sessions are scoped by connection and `terminal_id`; one connection cannot address another's session
- [ ] Spawn emits `terminal_spawned`, output emits `terminal_output`, and both exit and spawn failure emit `terminal_exited`
- [ ] Input and resize for an absent session are no-ops rather than errors
- [ ] A live session younger than two seconds is reused on repeat spawn and ignores kill during that window
- [ ] Connection cleanup kills that connection's sessions and leaves no orphan process
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::terminal::)'` and sees connection scoping, the lifecycle messages, absent-session no-ops, the two-second reuse window, and cleanup pass
