# Task 91 — Desktop smoke suite

**Plan:** [plan.md](../plan.md) · **Certificate:** [91-desktop_smoke-certificate.md](91-desktop_smoke-certificate.md)

**Implements:** [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition) · [00-overview.md §Implementation acceptance](../../../00-overview.md#implementation-acceptance)
**Depends on:** 64, 69, 73, 85
**Produces:** a Desktop client connecting, syncing state, executing a turn, approving a tool, changing cwd, and reconnecting against the assembled binary
**Pointers:** `tests/conformance/desktop.rs`, `tests/conformance/harness/desktop_driver.mjs`; reference: `../letta-code/src/app-server-client.ts`, `../letta-code/src/websocket/listener/connection-state-sync.ts`

## Steps

- [ ] Drive the assembled binary with the Desktop App Server client behavior from the pinned checkout, unmodified
- [ ] Connect and sync authoritative snapshots, then execute a turn against a fake provider
- [ ] Trigger a tool approval and approve it, asserting the tool runs exactly once
- [ ] Change the working directory and assert subsequent turns use it
- [ ] Disconnect and reconnect, asserting subscription and sequence recovery with no data loss
- [ ] Assert the whole flow against a real socket and a real binary rather than an in-process harness

## Definition of done

- [ ] The client connects, syncs authoritative snapshots, and executes a turn against the assembled binary over a real socket
- [ ] A tool approval is requested, approved, and the tool executes exactly once
- [ ] A cwd change applies to subsequent turns
- [ ] Disconnect and reconnect recover subscriptions and the event sequence with no lost state
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run --test desktop` against the built binary and watches connect, sync, a turn, a tool approval, a cwd change, and a reconnect complete
