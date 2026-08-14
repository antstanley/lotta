# Task 85 — Composition root and the eight-step shutdown

**Plan:** [plan.md](../plan.md) · **Certificate:** [85-composition_root_and_shutdown-certificate.md](85-composition_root_and_shutdown-certificate.md)

**Implements:** [07-channels-and-operations.md §Shutdown and restart](../../../07-channels-and-operations.md#shutdown-and-restart) · [architecture-principles.md §Workspace layout](../../../architecture-principles.md#workspace-layout) · [architecture-principles.md §Dependency graph](../../../architecture-principles.md#dependency-graph) · [00-overview.md §System shape](../../../00-overview.md#system-shape)
**Depends on:** 20, 42, 55, 57, 78, 83, 84
**Produces:** the workspace-root `src/main.rs` binary composing every adapter and executing the eight-step SIGTERM/SIGINT sequence, with lazy restart reload
**Pointers:** `src/main.rs`, `src/composition.rs`, `src/shutdown.rs`; reference: `../letta-code/src/websocket/app-server.ts`, `../letta-code/src/cli/subcommands/listen.tsx`, `../letta-code/src/websocket/listener/lifecycle.ts`

## Steps

- [ ] Compose concrete adapters into the runtime's port traits in the workspace-root `src/main.rs`, with no business logic in the binary
- [ ] Select adapters by feature flags at composition time without altering domain semantics
- [ ] Implement the eight-step shutdown of `07-channels-and-operations.md` §Shutdown and restart in order, bounded by `SHUTDOWN_GRACE_MS`
- [ ] Implement restart behavior: reload agents lazily, validate transcript manifests, restore schedules, recompile prompts only when inputs changed, and leave channels to the channel host
- [ ] Assert no adapter crate is reachable from `lotta-runtime` after composition
- [ ] Add an integration test driving a real SIGTERM against the running binary

## Definition of done

- [ ] `src/main.rs` at the workspace root is the only composition point and contains no business logic
- [ ] All eight shutdown steps execute in order on SIGTERM and SIGINT, bounded by `SHUTDOWN_GRACE_MS`
- [ ] Shutdown abandons no child process or compatibility host, and releases the store lock
- [ ] Restart reloads agents lazily, validates transcript manifests, restores schedules, and recompiles prompts only when inputs changed
- [ ] There is no configuration-reload signal handler; only SIGTERM and SIGINT are handled
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer starts the built binary, sends SIGTERM, and observes the eight-step sequence in the log, exit within the grace bound, no orphan process, and a released store lock
