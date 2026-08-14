# Task 69 — WebSocket skills and settings command groups

**Plan:** [plan.md](../plan.md) · **Certificate:** [69-ws_skills_and_settings_commands-certificate.md](69-ws_skills_and_settings_commands-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 20, 27, 36
**Produces:** skills enable/disable plus the settings group — cwd map, reflection settings, and experiments
**Pointers:** `crates/lotta-app-server/src/ws/groups/skills.rs`, `ws/groups/settings.rs`; reference: `../letta-code/src/websocket/listener/commands/skills-agents.ts`, `commands/settings.ts`, `../letta-code/src/websocket/listener/remote-settings.ts`, `cwd-change.ts`, `reflection-settings-validation.test.ts`

## Steps

- [ ] Decode and route skills enable and disable, updating the runtime's selected skill sources
- [ ] Decode and route the settings group: cwd map, reflection settings, and experiments
- [ ] Validate reflection settings before persistence and reject invalid values
- [ ] Apply a cwd change to subsequent turns and record the original path when the new one is missing
- [ ] Emit skills and settings update snapshots after mutating commands
- [ ] Persist settings through the Task 27 side store with the correct scope precedence

## Definition of done

- [ ] Skills enable and disable update the runtime's selected sources and emit a skills update snapshot
- [ ] The settings group covers cwd map, reflection settings, and experiments, each persisted through the correct side-store scope
- [ ] Reflection settings are validated before persistence and invalid values are rejected without writing
- [ ] A cwd change applies to subsequent turns, and a missing directory records the original path for a one-time reminder
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::skills::) + test(ws::settings::)'` and sees skill enable/disable, three settings scopes, reflection validation, and cwd change pass
