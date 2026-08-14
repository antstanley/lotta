# Task 92 — ChannelGateway conformance

**Plan:** [plan.md](../plan.md) · **Certificate:** [92-channel_gateway_conformance-certificate.md](92-channel_gateway_conformance-certificate.md)

**Implements:** [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition) · [07-channels-and-operations.md §Channel command surface](../../../07-channels-and-operations.md#channel-command-surface)
**Depends on:** 79, 81, 82, 85
**Produces:** the existing ChannelGateway client driving a Rust App Server runtime through pairing, routing, turn, and reply fixtures
**Pointers:** `tests/conformance/channel_gateway.rs`, `fixtures/channels/`; reference: `../letta-code/src/channels/core-stream.ts`, `../letta-code/src/channels/commands.ts`, `../letta-code/src/types/service-protocol.ts`

## Steps

- [ ] Capture pairing, routing, turn, and reply fixtures from the pinned baseline ChannelGateway
- [ ] Drive the assembled binary with the unmodified ChannelGateway client over the loopback App Server WebSocket and the control plane
- [ ] Exercise all twenty management commands and observe the four push events
- [ ] Exercise the eleven shared operational commands with tiered authorization
- [ ] Assert an outbound reply reaches the platform adapter only when the model calls `MessageChannel`
- [ ] Fail the suite if any command name is unknown to the server

## Definition of done

- [ ] The unmodified ChannelGateway client completes the pairing, routing, turn, and reply fixtures against the assembled binary
- [ ] All twenty management commands are accepted and the four push events are observed
- [ ] The eleven shared operational commands work with tiered authorization
- [ ] An outbound reply reaches the platform adapter only when the model calls `MessageChannel`
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run --test channel_gateway` against the built binary and sees pairing, routing, a turn, and a reply complete with all twenty management commands accepted
