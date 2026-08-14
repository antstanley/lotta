# Task 82 — Shared channel command surface

**Plan:** [plan.md](../plan.md) · **Certificate:** [82-channel_shared_command_surface-certificate.md](82-channel_shared_command_surface-certificate.md)

**Implements:** [07-channels-and-operations.md §Channel command surface](../../../07-channels-and-operations.md#channel-command-surface)
**Depends on:** 41, 80, 81
**Produces:** the eleven shared operational commands with tiered authorization and help-instead-of-transcript for unsupported commands
**Pointers:** `crates/lotta-channels/src/commands/surface.rs`, `commands/authorization.rs`; reference: `../letta-code/src/channels/commands.ts`, `../letta-code/src/channels/command-surface.ts`, `../letta-code/src/channels/command-runtime-executor.ts`

## Steps

- [ ] Implement `help`, `status`, `whoami`, `pause`, `resume`, `cancel`, `chat`, `feedback`, `model`, `reflection` (alias `reflect`), and `reload`
- [ ] Authorize commands by tier, separately from message admission, using `admin_users` and `user_allowed_commands`
- [ ] Return help for an unsupported command rather than entering the agent transcript
- [ ] Route `cancel` through the runtime cancellation path and `model` through the model-update path
- [ ] Execute commands before ordinary agent ingress, after sender gating
- [ ] Add a test asserting the command set equals the spec list so an added command fails

## Definition of done

- [ ] All eleven shared commands exist with the `reflect` alias, and the set equals the §Channel command surface list exactly
- [ ] Command authorization is tiered separately from message admission, honouring `admin_users` and `user_allowed_commands`
- [ ] An unsupported command returns help rather than entering the agent transcript
- [ ] Commands execute before ordinary agent ingress and after sender gating
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-channels -E 'test(commands::)'` and sees the eleven-command set with alias, tiered authorization, help-not-transcript, and gate-then-command-then-ingress ordering pass
