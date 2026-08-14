# Done Certificate — Task 92: ChannelGateway conformance

**Task:** [92-channel_gateway_conformance.md](92-channel_gateway_conformance.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 92. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 92) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the existing ChannelGateway client driving a Rust App Server runtime through pairing, routing, turn, and reply fixtures.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must use the twenty baseline command names: `00-overview.md` §Compatibility definition requires the existing ChannelGateway client to drive the runtime, and a renamed command shares no vocabulary with it.

## Obligations

- **O1 — The unmodified ChannelGateway client completes the pairing, routing, turn, and reply fixtures against the assembled binary**
  - *Claim:* All four fixture flows pass with no client patch.
  - *Evidence to collect:* Run `cargo nextest run --test channel_gateway` — expect four passing flow cases; confirm `git -C ../letta-code status --porcelain` is clean.
  - *Status:* ☐ unverified

- **O2 — All twenty management commands are accepted and the four push events are observed**
  - *Claim:* Every command name in `types/service-protocol.ts:7-27` succeeds and the four update events arrive.
  - *Evidence to collect:* Run `cargo nextest run --test channel_gateway -E 'test(management_commands)'` — expect twenty cases plus four event cases, driven by the extracted name list rather than literals.
  - *Checks:* Resolve each command dispatch — confirm the server recognizes the name rather than dropping it as unknown; read the server's unknown-discriminant counter and expect zero.
  - *Status:* ☐ unverified

- **O3 — The eleven shared operational commands work with tiered authorization**
  - *Claim:* Each command succeeds for an authorized sender and is refused for an unauthorized one.
  - *Evidence to collect:* Run `cargo nextest run --test channel_gateway -E 'test(shared_commands)'` — expect eleven authorized cases and at least one refusal case.
  - *Status:* ☐ unverified

- **O4 — An outbound reply reaches the platform adapter only when the model calls `MessageChannel`**
  - *Claim:* A turn without the tool call produces no platform send; one with it produces exactly one.
  - *Evidence to collect:* Run `cargo nextest run --test channel_gateway -E 'test(reply_gating)'` — expect both cases asserting the adapter's send count.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run --test channel_gateway` against the built binary and sees pairing, routing, a turn, and a reply complete with all twenty management commands accepted**
  - *Claim:* The ChannelGateway suite is green with an unpatched client.
  - *Evidence to collect:* Run the command, confirm zero failures and twenty management-command cases, and confirm the unknown-discriminant counter is zero.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-channels/src/protocol/` (Task 79) defines the vocabulary; confirm `protocol::command_set_is_exactly_twenty` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-channels/src/access_control.rs` (Task 81); confirm `access_control::gates_first` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Live platform APIs are out of scope; the suite drives the gateway with recorded platform inputs.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
