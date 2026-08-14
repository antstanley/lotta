# Task 81 — Channel access control, pairing, and operational bounds

**Plan:** [plan.md](../plan.md) · **Certificate:** [81-channel_access_control_pairing_and_bounds-certificate.md](81-channel_access_control_pairing_and_bounds-certificate.md)

**Implements:** [07-channels-and-operations.md §Access control](../../../07-channels-and-operations.md#access-control) · [07-channels-and-operations.md §Operational bounds](../../../07-channels-and-operations.md#operational-bounds)
**Depends on:** 27, 80
**Produces:** central sender gating before commands or routes, the three-policy access model, single-use bounded pairing codes, and the operational bounds table
**Pointers:** `crates/lotta-channels/src/access_control.rs`, `src/pairing.rs`, `src/bounds.rs`; reference: `../letta-code/src/channels/access-control.ts`, `../letta-code/src/channels/credential-store.ts`, `../letta-code/src/channels/command-surface.ts`

## Steps

- [ ] Gate every inbound message — including groups and auto-routed Slack/Discord traffic — through central sender gating before commands or routes
- [ ] Implement `dm_policy` (`pairing`, `allowlist`, `open`), `group_policy` (`open`, `allowlist`), `allowed_users`, `admin_users`, and `user_allowed_commands`
- [ ] Merge environment allowlists and explicit allow-all flags per baseline behavior, and ensure pairing approval adds permission without removing a stricter global deny
- [ ] Generate pairing codes with cryptographic randomness, expiring after `PAIRING_TTL_SECONDS` and single-use
- [ ] Reuse a sender/account pair's one unexpired code instead of minting another, and cap pending codes at `PAIRINGS_PENDING_PER_CHANNEL_MAX` with expired-first pruning
- [ ] Define the remaining `07-channels-and-operations.md` §Operational bounds constants with below/at/above tests

## Definition of done

- [ ] Central sender gating runs before commands and before routing, for direct, group, and auto-routed traffic
- [ ] `dm_policy`, `group_policy`, `allowed_users`, `admin_users`, and `user_allowed_commands` all affect decisions, and pairing approval never removes a stricter global deny
- [ ] Pairing codes use cryptographic randomness, expire at `PAIRING_TTL_SECONDS`, are single-use, and a sender/account pair reuses its one unexpired code
- [ ] Pending codes are capped at `PAIRINGS_PENDING_PER_CHANNEL_MAX` with expired-first pruning, and there is no source-address rate limiter
- [ ] All `07-channels-and-operations.md` §Operational bounds constants are defined with the table's names and defaults and have below/at/above tests
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-channels -E 'test(access_control::) + test(pairing::) + test(bounds::)'` and sees gating-before-commands, five policy fields, single-use bounded codes, and the operational bounds pass
