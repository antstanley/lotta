# Task 79 — Channel management protocol: the twenty commands and four push events

**Plan:** [plan.md](../plan.md) · **Certificate:** [79-channel_management_protocol-certificate.md](79-channel_management_protocol-certificate.md)

**Implements:** [07-channels-and-operations.md §Channel command surface](../../../07-channels-and-operations.md#channel-command-surface) · [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups)
**Depends on:** 10, 20, 78
**Produces:** exactly the twenty baseline channel service commands and the four push events, with names matching `types/service-protocol.ts` verbatim
**Pointers:** `crates/lotta-channels/src/protocol/commands.rs`, `protocol/events.rs`; reference: `../letta-code/src/types/service-protocol.ts`, `../letta-code/src/websocket/listener/management-protocol-inbound.ts`

## Steps

- [ ] Define exactly the twenty commands: `channels_list`, `channel_accounts_list`, `channel_account_create`, `channel_account_update`, `channel_account_bind`, `channel_account_unbind`, `channel_account_delete`, `channel_account_start`, `channel_account_stop`, `channel_get_config`, `channel_set_config`, `channel_start`, `channel_stop`, `channel_pairings_list`, `channel_pairing_bind`, `channel_routes_list`, `channel_targets_list`, `channel_target_bind`, `channel_route_update`, `channel_route_remove`
- [ ] Define `channels_updated`, `channel_accounts_updated`, `channel_pairings_updated`, and `channel_targets_updated` as push events, not commands
- [ ] Redact secret config values from every App Server snapshot
- [ ] Route each command to the Task 80 account/route surface
- [ ] Assert the command set against the extracted protocol fixture so an added or removed name fails
- [ ] Round-trip every command and event against `fixtures/protocol/discriminants.json`

## Definition of done

- [ ] The command set is exactly the twenty baseline names, verified against the extracted fixture in both directions
- [ ] The four update notifications are push events and are not addressable as commands
- [ ] Secret config values are redacted from App Server snapshots
- [ ] Every command and event round-trips against the protocol fixture
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-channels -E 'test(protocol::)'` and sees exactly twenty commands matching the baseline, four push events, redaction, and full fixture round-trips pass
