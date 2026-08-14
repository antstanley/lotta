# Done Certificate — Task 80: Channel accounts, plugins, and the routing model

**Task:** [80-channel_account_and_routing_model.md](80-channel_account_and_routing_model.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 80. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 80) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the six first-party channel IDs with custom-plugin loading, and the seven-step inbound flow through routing to `runtime_start` and `MessageChannel`.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must keep inbound and outbound separate: `07-channels-and-operations.md` §Account and routing model states a turn can complete without a channel reply.

## Obligations

- **O1 — The six first-party channel IDs exist and `custom` loads a user-defined plugin directory with all six required members**
  - *Claim:* The ID set matches §Account and routing model, and a custom plugin directory missing `channel.json` is rejected.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(accounts::channel_ids) + test(plugins::custom_loader)'` — expect the six-ID case and a loader case per required member.
  - *Status:* ☐ unverified

- **O2 — Accounts persist snake_case and project camelCase, with `group_policy`, `admin_users`, and `user_allowed_commands` present in both**
  - *Claim:* The on-disk and runtime shapes differ in case only, and none of the three fields is dropped.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(accounts::dual_case)'` — expect PASS asserting key names on both sides, reusing the Task 03 entity.
  - *Status:* ☐ unverified

- **O3 — The seven-step inbound flow executes in order, publishing `MessageChannel` and submitting `input` with a stable `client_message_id`**
  - *Claim:* A recorded inbound produces the seven steps in the documented order, and re-delivering the same platform message reuses the same `client_message_id`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(routing::inbound_flow)'` — expect a step-order case and `stable_client_message_id_on_redelivery`, the latter asserting the duplicate is deduplicated by the Task 18 admission chain.
  - *Checks:* Resolve the `client_message_id` derivation — confirm it is derived from stable platform identifiers so a redelivery deduplicates, rather than being freshly generated per attempt.
  - *Status:* ☐ unverified

- **O4 — Inbound delivery and outbound reply are separate: a turn can complete without a channel reply**
  - *Claim:* A turn whose model never calls `MessageChannel` completes normally and sends nothing to the platform.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(routing::reply_is_optional)'` — expect PASS asserting a terminal event and zero outbound platform sends.
  - *Status:* ☐ unverified

- **O5 — `CHANNEL_ACCOUNTS_MAX` and `CHANNEL_ROUTES_MAX` reject at their limits**
  - *Claim:* The 257th account and the 100,001st route are rejected.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(accounts::bounds) + test(routing::bounds)'` — expect below/at/above cases for both constants from `07-channels-and-operations.md` §Operational bounds.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-channels -E 'test(accounts::) + test(routing::) + test(plugins::)'` and sees six channel IDs, dual-case accounts, the seven-step flow with stable IDs, optional reply, and both bounds pass**
  - *Claim:* The account and routing modules pass.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `inbound_flow` step-order case.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/side/channels.rs` (Task 27) persists these files; confirm `side::channels::preserves_plugin_files` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Access control gating is Task 81 and runs before routing; this task implements the routing step it gates.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
