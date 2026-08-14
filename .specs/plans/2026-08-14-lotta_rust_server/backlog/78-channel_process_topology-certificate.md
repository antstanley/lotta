# Done Certificate — Task 78: Channel process topology and control plane

**Task:** [78-channel_process_topology.md](78-channel_process_topology.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

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
  - *Status:* ☐ unverified

- **O2 — Runtime data flows over the App Server WebSocket while management flows over newline-delimited JSON on stdin/stdout, with no crossover**
  - *Claim:* Runtime commands never appear on the control plane and management commands never appear on the WebSocket.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(topology::plane_separation)'` — expect PASS asserting the recorded traffic on each plane contains only its own message classes.
  - *Checks:* Resolve the transport used by `publish_runtime_tools` — confirm it is the stdin/stdout control plane, per `07-channels-and-operations.md` §Process topology, not the WebSocket.
  - *Status:* ☐ unverified

- **O3 — `publish_runtime_tools`, `release_runtime_tools`, and the `/channels` slash command work over the control plane**
  - *Claim:* Publishing exposes channel-owned tools to the runtime, releasing removes them, and `/channels` returns channel state.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(control_plane::)'` — expect three cases; the publish case asserts the tool appears in the runtime registry and the release case asserts its removal.
  - *Status:* ☐ unverified

- **O4 — The host cannot write the local-backend store and owns only `~/.letta/channels`**
  - *Claim:* The child's environment and arguments carry no backend root, and a write attempt outside the channel directory fails.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(topology::store_isolation)'` — expect PASS asserting the absence of the backend root in the child environment and a rejected out-of-scope write.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer starts the server with channels enabled and observes the supervised host connect over loopback, publish runtime tools over the control plane, and reject a backend-root write**
  - *Claim:* The topology is observable end to end.
  - *Evidence to collect:* Start the binary, confirm the child process exists, confirm a `MessageChannel` registration appears in the runtime registry, and confirm the host process has no handle on the backend root.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-extensions/src/sidecar/` (Task 42) validates the control-plane frames; confirm `sidecar::validation` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

`07-channels-and-operations.md` §Assumptions leaves channel-host ownership open; this task supervises the existing host unchanged.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
