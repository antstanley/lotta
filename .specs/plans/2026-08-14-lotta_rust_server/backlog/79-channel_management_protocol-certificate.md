# Done Certificate — Task 79: Channel management protocol: the twenty commands and four push events

**Task:** [79-channel_management_protocol.md](79-channel_management_protocol.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 79. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 79) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** exactly the twenty baseline channel service commands and the four push events, with names matching `types/service-protocol.ts` verbatim.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not invent command names: `07-channels-and-operations.md` §Channel command surface states there are exactly twenty, and `00-overview.md` §Compatibility definition requires the existing ChannelGateway client to drive the server.

## Obligations

- **O1 — The command set is exactly the twenty baseline names, verified against the extracted fixture in both directions**
  - *Claim:* There are twenty commands, each name matching `../letta-code/src/types/service-protocol.ts:7-27`, with no extra and none missing.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(protocol::command_set_is_exactly_twenty)'` — expect PASS asserting set equality against the fixture. Read the definition and compare name by name against the baseline file.
  - *Checks:* Resolve each command name's spelling — confirm snake_case baseline names (for example `channel_account_create`), not kebab-case invented equivalents. A single renamed command means an existing ChannelGateway client shares no vocabulary with the server.
  - *Status:* ☐ unverified

- **O2 — The four update notifications are push events and are not addressable as commands**
  - *Claim:* `channels_updated`, `channel_accounts_updated`, `channel_pairings_updated`, and `channel_targets_updated` decode as messages and are rejected as commands.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(protocol::push_events)'` — expect four message cases and four command-rejection cases.
  - *Status:* ☐ unverified

- **O3 — Secret config values are redacted from App Server snapshots**
  - *Claim:* A `channel_get_config` response and any account snapshot omit secret values.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(protocol::redaction)'` — expect PASS asserting a planted secret marker is absent from both response bodies, per `07-channels-and-operations.md` §Access control.
  - *Status:* ☐ unverified

- **O4 — Every command and event round-trips against the protocol fixture**
  - *Claim:* All twenty-four discriminants decode and encode.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(protocol::fixture_round_trip)'` — expect twenty-four cases derived from `fixtures/protocol/discriminants.json`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-channels -E 'test(protocol::)'` and sees exactly twenty commands matching the baseline, four push events, redaction, and full fixture round-trips pass**
  - *Claim:* The channel protocol module passes with set equality asserted.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `command_set_is_exactly_twenty` case.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-protocol/` (Task 10) holds the discriminant fixture; confirm `manifest::every_variant_has_a_fixture` still passes with the channel commands added : ☐ (PRESERVED / REGRESSION)

## Residue

Command semantics live in Tasks 80–82; this task fixes the vocabulary and the wire shapes.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
