# Task 69 — WebSocket skills and settings command groups

**Plan:** [plan.md](../plan.md) · **Certificate:** [69-ws_skills_and_settings_commands-certificate.md](69-ws_skills_and_settings_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 20, 27, 36
**Produces:** skills enable/disable plus the settings group — cwd map, reflection settings, and experiments
**Pointers:** `crates/lotta-app-server/src/ws/groups/skills.rs`, `ws/groups/settings.rs`; reference: `../letta-code/src/websocket/listener/commands/skills-agents.ts`, `commands/settings.ts`, `../letta-code/src/websocket/listener/remote-settings.ts`, `cwd-change.ts`, `reflection-settings-validation.test.ts`

## Steps

- [x] Decode and route skills enable and disable, updating the runtime's selected skill sources
- [x] Decode and route the settings group: cwd map, reflection settings, and experiments
- [x] Validate reflection settings before persistence and reject invalid values
- [x] Apply a cwd change to subsequent turns and record the original path when the new one is missing
- [x] Emit skills and settings update snapshots after mutating commands
- [x] Persist settings through the Task 27 side store with the correct scope precedence

## Definition of done

- [x] Skills enable and disable update the runtime's selected sources and emit a skills update snapshot
- [x] The settings group covers cwd map, reflection settings, and experiments, each persisted through the correct side-store scope
- [x] Reflection settings are validated before persistence and invalid values are rejected without writing
- [x] A cwd change applies to subsequent turns, and a missing directory records the original path for a one-time reminder
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::skills::) + test(ws::settings::)'` and sees skill enable/disable, three settings scopes, reflection validation, and cwd change pass
