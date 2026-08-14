# Task 54 — Turn setup resolution order

**Plan:** [plan.md](../plan.md) · **Certificate:** [54-turn_setup_resolution-certificate.md](54-turn_setup_resolution-certificate.md)

**Implements:** [03-runtime-and-turns.md §Turn setup](../../../03-runtime-and-turns.md#turn-setup)
**Depends on:** 18, 28, 30, 32, 34, 36, 41, 44, 45, 47
**Produces:** the ten-step turn setup in the specified order, with the two documented failure modes distinguishable by recovery
**Pointers:** `crates/lotta-runtime/src/turn/setup.rs`, `turn/setup_steps.rs`; reference: `../letta-code/src/websocket/listener/turn-setup.ts`, `cwd.ts`, `memfs-sync.ts`, `skill-injection.ts`, `runtime-workspace-sandbox.ts`

## Steps

- [ ] Resolve agent and conversation, rejecting cross-agent and archived-invalid access
- [ ] Resolve cwd; on a deleted directory fall back and record the original path for a one-time reminder
- [ ] Apply the workspace sandbox and permission mode, then synchronize or initialize local MemFS
- [ ] Compile the system prompt from managed prompt, memory files, available skills, and runtime reminders
- [ ] Resolve conversation model override, agent model, provider connection, context window, and toolset; discover selected skill sources; load agent/global/project mods and hooks
- [ ] Merge built-in, MCP, mod, channel, and controller-owned external tools, build validated provider messages, and emit sending/waiting loop status

## Definition of done

- [ ] Setup executes the ten steps of §Turn setup in order, and a recorded stage log matches the spec sequence
- [ ] A deleted cwd falls back and records the original path for a one-time reminder that fires once
- [ ] Cross-agent and archived-invalid access are rejected before any allocation
- [ ] Failure before provider admission returns a terminal error without appending a user message, while failure after durable input append records a distinguishable interrupted/error outcome
- [ ] The merged tool set contains built-in, MCP, mod, channel, and controller-owned external tools, and the toolset resolution honours the agent's configured toolset and allowlist
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(setup::)'` and sees the ten-step order, deleted-cwd fallback with a one-time reminder, access rejection, the two failure modes, and the five-source tool merge pass
