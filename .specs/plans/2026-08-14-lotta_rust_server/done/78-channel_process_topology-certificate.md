# Done Certificate — Task 78: Channel process topology and control plane

**Task:** [78-channel_process_topology.md](78-channel_process_topology.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-09-01 — implementation commit `2efe0bda16b4`

> Verification protocol for Task 78. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 78) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a supervised channel-host child speaking App Server WebSocket for runtime data and newline-delimited JSON over stdin/stdout for management.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must keep the two planes separate: `07-channels-and-operations.md` §Assumptions *Channel topology* assigns runtime data to the WebSocket and management to stdin/stdout.

## Obligations

- **O1 — The channel host runs as a supervised child using a loopback-scoped App Server token and the shared persistent channel configuration**
  - *Claim:* The host is spawned by the server, authenticates over loopback with its own token, and reads channel config from `~/.letta/channels`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(topology::supervision)'` — expect `spawns_child`, `authenticates_over_loopback`, and `reads_shared_channel_config`.
  - *Evidence:* The canonical state store loads `config.yaml`, `accounts.json`, and `routing.yaml` under the shared channels root and derives only enabled, configured, account-backed restorable routes. `topology::tests::restoration_requires_exact_enabled_configured_account` pins that rule, and `task78_real_runtime_data_plane` proves startup automatically restores and composes two canonical persisted routes. `built_lotta_channel_host_authenticates_starts_runtime_and_reaps` supervises the actual built `lotta channel-host`, observes a bearer-authenticated loopback WebSocket upgrade and runtime start, kills generation one, observes a distinct authenticated replacement generation with restored runtime tools, then shuts down and reaps it. `full_built_server_auto_spawns_rotates_and_reaps_channel_child` starts the complete built server with no synthetic child injection and proves automatic spawn, per-generation dynamic loopback capability, forced-child restart, old-child reap, and final-child reap on server shutdown. The Task 78 integration suite passed 3/3 and `lotta-channels` passed 15/15.
  - *Status:* **SATISFIED**

- **O2 — Runtime data flows over the App Server WebSocket while management flows over newline-delimited JSON on stdin/stdout, with no crossover**
  - *Claim:* Runtime commands never appear on the control plane and management commands never appear on the WebSocket.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(topology::plane_separation)'` — expect PASS asserting the recorded traffic on each plane contains only its own message classes.
  - *Checks:* Resolve the transport used by `publish_runtime_tools` — confirm it is the stdin/stdout control plane, per `07-channels-and-operations.md` §Process topology, not the WebSocket.
  - *Evidence:* `topology::tests::records_both_planes_and_rejects_negative_crossover` records both transports and rejects runtime frames on management and management frames on runtime. `task78_real_runtime_data_plane` binds a real dedicated loopback App Server listener, authenticates the host with its scoped capability, restores two routes, and drives real WebSocket `runtime_start`, inbound input, ordered `input_accepted`/`stream_delta`/loop-state/terminal events, and outbound `MessageChannel` delivery for both routes. Its separate duplex stdin/stdout stream carries only strict NDJSON `ready`, `/channels`, publish, release, shutdown, and completion management frames. The host sends one management publish and release while all runtime input, events, and tool delivery stay on the dedicated listener WebSocket; negative timeout assertions prove no extra or crossed traffic. The integration suite passed 3/3 and `lotta-channels` passed 15/15.
  - *Status:* **SATISFIED**

- **O3 — `publish_runtime_tools`, `release_runtime_tools`, and the `/channels` slash command work over the control plane**
  - *Claim:* Publishing exposes channel-owned tools to the runtime, releasing removes them, and `/channels` returns channel state.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(control_plane::)'` — expect three cases; the publish case asserts the tool appears in the runtime registry and the release case asserts its removal.
  - *Evidence:* The strict tagged NDJSON protocol admits only its exact frame structures, version, generation, owner, capability, request direction, correlation, deadline, and replay identity. `control_plane::tests::publish_release_channels_registry_effects` proves exact `/channels` state, canonical publish, and owner-scoped release; the codec/session tests reject partial, multiple, oversized, malformed, unknown-field, wrong-direction, expired, over-capacity, and replayed frames. Publication is not a parallel registry: `ChannelExternalToolManager` installs channel descriptors into the canonical `lotta_tools::ToolRegistry`, and `task78_real_runtime_data_plane` composes that same registry into a production `RegistrySnapshot`, executes the published `MessageChannel` through the canonical tool pipeline, observes exact delivered results, then proves release removes the runtime registration. The management trace is exactly one channels response, one publish, one release, and terminal `shutdown_complete`. `lotta-channels` passed 15/15.
  - *Status:* **SATISFIED**

- **O4 — The host cannot write the local-backend store and owns only `~/.letta/channels`**
  - *Claim:* The child's environment and arguments carry no backend root, and a write attempt outside the channel directory fails.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(topology::store_isolation)'` — expect PASS asserting the absence of the backend root in the child environment and a rejected out-of-scope write.
  - *Evidence:* The production supervisor launches each actual channel-host generation with the canonical channels root as its isolated writable root and does not pass the local-backend root through child arguments or environment. The isolation is active in the same child that authenticates, restores routes, publishes tools, and serves the WebSocket data plane—not a detached probe. In `full_built_server_auto_spawns_rotates_and_reaps_channel_child`, both generation-one and generation-two structured `startup_isolation` events assert `channels_write: true` and `backend_sibling_write_denied: true`; the complete lifecycle cannot advance to channels response/publication unless that in-child probe succeeds. Topology tests additionally reject symlink escape and hard-linked state and confirm token redaction. The integration suite passed 3/3 and `lotta-channels` passed 15/15.
  - *Status:* **SATISFIED**

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Final focused and broad evidence passed Task 78 integrations 3/3, `lotta-channels` 15/15, `lotta-app-server` 590/590, and source audit 10/10. The Task 78 audit manifest exhaustively computes and matches all 20 production sources with no missing, unexpected, or duplicate path and mutation-proves omission detection. The syntax-aware hard-limit gate enforces files ≤1,000 lines, production functions ≤70 lines, lines ≤100 columns, units-last named constants, and no lint-suppression escape across that no-gap set. `cargo fmt --all --check`, strict all-target/all-feature workspace Clippy with warnings denied, warning-denied workspace rustdoc, and `cargo deny check` are green. Workspace accounting records only the established external pinned-sibling SHA conformance exclusion; it hides no Task 78 failure and adds no other exclusion.
  - *Status:* **SATISFIED**

- **O6 — Reviewable: a reviewer starts the server with channels enabled and observes the supervised host connect over loopback, publish runtime tools over the control plane, and reject a backend-root write**
  - *Claim:* The topology is observable end to end.
  - *Evidence to collect:* Start the binary, confirm the child process exists, confirm a `MessageChannel` registration appears in the runtime registry, and confirm the host process has no handle on the backend root.
  - *Evidence:* `full_built_server_auto_spawns_rotates_and_reaps_channel_child` is a full production-server O6 trace. For each of two actual child generations it captures structured `child_connected`, `startup_isolation`, `channels_response`, `runtime_tools_published`, and `message_channel_registered` events with exact generation, PID, owner, runtime, channel snapshot, and manager-tool fields. After generation one is killed it proves ordered `runtime_tools_released` → `capability_revoked` → `child_reaped` before generation-two publication; on SIGINT it proves the same ordered release/revoke/reap lifecycle for generation two and verifies that no child remains. The external run artifact `target/task78-o6/final.out` records the dedicated loopback WebSocket URL, actual channel-host PID, `MessageChannel registered: true`, and `control publish: true`, with empty `final.err`. The active same-child startup event proves channels-root write success and backend-sibling write denial. The integration suite passed 3/3. A clean independent GPT-5.6 Sol review of final commit `2efe0bda16b4` returned `CORRECT / DONE`.
  - *Status:* **SATISFIED**

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-extensions/src/sidecar/` (Task 42) validates the control-plane frames; `sidecar::validation` remains green under the final strict NDJSON framing and validation behavior — **PRESERVED**.

## Residue

`07-channels-and-operations.md` §Assumptions leaves channel-host ownership open; this task supervises the existing host unchanged. Workspace accounting records only the established external pinned-sibling SHA conformance exclusion; no Task 78 failure or additional exclusion is hidden by it.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: **DONE**
CONFIDENCE: **high**
SUMMARY: Verified 2026-09-01 at implementation commit `2efe0bda16b4`. All O1–O6 are satisfied and the Task 42 regression is preserved. Canonical persisted channel state auto-restores; the full built server supervises an actual child with dynamic loopback authentication, restart, capability rotation, ordered release/revoke/reap, and final shutdown reap. A real dedicated listener carries the complete runtime WebSocket data plane while strict NDJSON carries only management `/channels`, publish, release, and lifecycle frames. Published channel tools use the canonical `ToolRegistry`; both actual child generations actively prove channels-root write access and backend-sibling denial. Full-server O6 structured lifecycle and the external O6 artifact are exact. Integrations passed 3/3, `lotta-channels` 15/15, `lotta-app-server` 590/590, and source audit 10/10 with an exhaustive 20-source no-gap manifest and file/function/line limits of 1,000/70/100. Format, strict Clippy, warning-denied rustdoc, and deny are green. Only the established external pinned-sibling SHA conformance exclusion remains. A clean independent GPT-5.6 Sol review returned `CORRECT / DONE`.
