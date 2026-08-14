# Done Certificate — Task 81: Channel access control, pairing, and operational bounds

**Task:** [81-channel_access_control_pairing_and_bounds.md](81-channel_access_control_pairing_and_bounds.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 81. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 81) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** central sender gating before commands or routes, the three-policy access model, single-use bounded pairing codes, and the operational bounds table.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let pairing widen a stricter global deny: `07-channels-and-operations.md` §Access control states pairing approval adds permission and never removes a stricter global deny.

## Obligations

- **O1 — Central sender gating runs before commands and before routing, for direct, group, and auto-routed traffic**
  - *Claim:* A denied sender's message reaches neither the command surface nor the routing step, in all three traffic classes.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(access_control::gates_first)'` — expect three cases asserting zero command executions and zero routing calls.
  - *Checks:* Resolve the gate's position relative to slash-command execution — confirm gating precedes it. `07-channels-and-operations.md` §Access control requires central gating before commands or routes.
  - *Status:* ☐ unverified

- **O2 — `dm_policy`, `group_policy`, `allowed_users`, `admin_users`, and `user_allowed_commands` all affect decisions, and pairing approval never removes a stricter global deny**
  - *Claim:* Each policy field changes at least one decision, and a globally denied sender stays denied after pairing approval.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(access_control::policies)'` — expect one case per field (five) plus `pairing_does_not_override_global_deny`.
  - *Status:* ☐ unverified

- **O3 — Pairing codes use cryptographic randomness, expire at `PAIRING_TTL_SECONDS`, are single-use, and a sender/account pair reuses its one unexpired code**
  - *Claim:* A code is unpredictable, expires at 900 seconds, cannot be redeemed twice, and a second request returns the same unexpired code.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(pairing::)'` — expect `uses_csprng`, `expires_at_ttl`, `single_use`, and `reuses_unexpired_code`, the TTL case driven by the fake clock.
  - *Checks:* Resolve the randomness source — confirm it is a cryptographic RNG, not a general-purpose PRNG, per §Access control.
  - *Status:* ☐ unverified

- **O4 — Pending codes are capped at `PAIRINGS_PENDING_PER_CHANNEL_MAX` with expired-first pruning, and there is no source-address rate limiter**
  - *Claim:* The 51st pending code prunes expired entries first, then evicts the oldest; no rate limiter is introduced.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(pairing::cap)'` — expect `prunes_expired_first` and `evicts_oldest_after_pruning`. Grep for a rate limiter in the pairing path — expect none, matching §Access control (`the baseline has no source-address rate limiter`).
  - *Status:* ☐ unverified

- **O5 — All `07-channels-and-operations.md` §Operational bounds constants are defined with the table's names and defaults and have below/at/above tests**
  - *Claim:* `CHANNEL_ACCOUNTS_MAX`, `CHANNEL_ROUTES_MAX`, `PAIRINGS_PENDING_PER_CHANNEL_MAX`, `PAIRING_TTL_SECONDS`, `CHANNEL_MESSAGE_BYTES_MAX`, `CHANNEL_MEDIA_BYTES_MAX`, `CHANNEL_DELIVERY_RETRIES_MAX`, `SHUTDOWN_GRACE_MS`, and `SIDECAR_RESTARTS_PER_HOUR_MAX` all exist with the table's values.
  - *Evidence to collect:* Read `crates/lotta-channels/src/bounds.rs` and compare against the §Operational bounds table. Run `cargo nextest run -p lotta-channels -E 'test(bounds::)'` — expect three cases per constant.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-channels -E 'test(access_control::) + test(pairing::) + test(bounds::)'` and sees gating-before-commands, five policy fields, single-use bounded codes, and the operational bounds pass**
  - *Claim:* The access-control, pairing, and bounds modules pass.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `gates_first` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-channels/src/routing.rs` (Task 80) now runs behind the gate; confirm `routing::inbound_flow` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

The absence of a source-address rate limiter matches the baseline; adding one would be a hardening requiring a change spec.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
