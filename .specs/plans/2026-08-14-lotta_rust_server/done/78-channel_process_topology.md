# Task 78 — Channel process topology and control plane

**Plan:** [plan.md](../plan.md) · **Certificate:** [78-channel_process_topology-certificate.md](78-channel_process_topology-certificate.md)

**Implements:** [07-channels-and-operations.md §Process topology](../../../07-channels-and-operations.md#process-topology) · [07-channels-and-operations.md §Responsibilities](../../../07-channels-and-operations.md#responsibilities)
**Depends on:** 20, 42
**Produces:** a supervised channel-host child speaking App Server WebSocket for runtime data and newline-delimited JSON over stdin/stdout for management
**Pointers:** `crates/lotta-channels/src/topology.rs`, `src/control_plane.rs`, `src/supervisor.rs`; reference: `../letta-code/src/cli/subcommands/listen.tsx`, `../letta-code/src/websocket/listener/management-protocol-inbound.ts`, `../letta-code/src/websocket/listener/channel-runtime-tools.ts`

## Steps

- [x] Supervise the channel host as a child process in the same service, sharing the persistent channel configuration and a loopback-scoped App Server token
- [x] Carry runtime data over the public App Server WebSocket: the host registers `MessageChannel`, starts routed runtimes, submits input, and consumes stream/state events
- [x] Carry management over a newline-delimited JSON control plane on the supervised stdin/stdout
- [x] Implement `publish_runtime_tools`, `release_runtime_tools`, and the `/channels` slash command on the control plane
- [x] Keep the host out of the local-backend store; it owns only `~/.letta/channels`
- [x] Bound and validate control-plane frames using the Task 42 validation properties

## Definition of done

- [x] The channel host runs as a supervised child using a loopback-scoped App Server token and the shared persistent channel configuration
- [x] Runtime data flows over the App Server WebSocket while management flows over newline-delimited JSON on stdin/stdout, with no crossover
- [x] `publish_runtime_tools`, `release_runtime_tools`, and the `/channels` slash command work over the control plane
- [x] The host cannot write the local-backend store and owns only `~/.letta/channels`
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer starts the server with channels enabled and observes the supervised host connect over loopback, publish runtime tools over the control plane, and reject a backend-root write
