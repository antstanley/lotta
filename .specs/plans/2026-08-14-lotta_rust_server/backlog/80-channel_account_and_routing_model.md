# Task 80 — Channel accounts, plugins, and the routing model

**Plan:** [plan.md](../plan.md) · **Certificate:** [80-channel_account_and_routing_model-certificate.md](80-channel_account_and_routing_model-certificate.md)

**Implements:** [07-channels-and-operations.md §Account and routing model](../../../07-channels-and-operations.md#account-and-routing-model) · [01-domain-model.md §Channel account and route](../../../01-domain-model.md#channel-account-and-route)
**Depends on:** 03, 27, 79
**Produces:** the six first-party channel IDs with custom-plugin loading, and the seven-step inbound flow through routing to `runtime_start` and `MessageChannel`
**Pointers:** `crates/lotta-channels/src/accounts.rs`, `src/routing.rs`, `src/plugins/custom.rs`; reference: `../letta-code/src/channels/accounts.ts`, `../letta-code/src/channels/config.ts`, `../letta-code/src/channels/custom`, `../letta-code/src/channels/core-stream.ts`

## Steps

- [ ] Define the first-party channel IDs `telegram`, `slack`, `discord`, `custom`, `whatsapp`, and `signal`, with `custom` as the built-in loader for user-defined plugins
- [ ] Load a custom plugin directory containing `channel.json`, an entry module, accounts, routing, pairing, and runtime dependencies
- [ ] Persist accounts as snake_case `accounts.json` and project them as camelCase at runtime
- [ ] Implement the seven-step inbound flow: normalize, gate, run slash commands, resolve routing or mint a pairing code, `runtime_start` plus `MessageChannel` publication plus `input` with a stable `client_message_id`, lifecycle/progress presentation, and outbound reply through `MessageChannel`
- [ ] Keep inbound delivery and outbound reply separate so a turn can complete without a channel reply
- [ ] Enforce `CHANNEL_ACCOUNTS_MAX` and `CHANNEL_ROUTES_MAX`

## Definition of done

- [ ] The six first-party channel IDs exist and `custom` loads a user-defined plugin directory with all six required members
- [ ] Accounts persist snake_case and project camelCase, with `group_policy`, `admin_users`, and `user_allowed_commands` present in both
- [ ] The seven-step inbound flow executes in order, publishing `MessageChannel` and submitting `input` with a stable `client_message_id`
- [ ] Inbound delivery and outbound reply are separate: a turn can complete without a channel reply
- [ ] `CHANNEL_ACCOUNTS_MAX` and `CHANNEL_ROUTES_MAX` reject at their limits
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-channels -E 'test(accounts::) + test(routing::) + test(plugins::)'` and sees six channel IDs, dual-case accounts, the seven-step flow with stable IDs, optional reply, and both bounds pass
