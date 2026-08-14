# Done Certificate — Task 82: Shared channel command surface

**Task:** [82-channel_shared_command_surface.md](82-channel_shared_command_surface.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 82. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 82) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the eleven shared operational commands with tiered authorization and help-instead-of-transcript for unsupported commands.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let an unsupported command enter the transcript: `07-channels-and-operations.md` §Channel command surface states unsupported commands return help.

## Obligations

- **O1 — All eleven shared commands exist with the `reflect` alias, and the set equals the §Channel command surface list exactly**
  - *Claim:* The command table has eleven entries plus the alias, matching the spec list with no additions.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(commands::set_matches_spec)'` — expect PASS asserting set equality against the eleven names, and a case confirming `reflect` resolves to `reflection`.
  - *Status:* ☐ unverified

- **O2 — Command authorization is tiered separately from message admission, honouring `admin_users` and `user_allowed_commands`**
  - *Claim:* A sender admitted for messages can still be refused a command, and an admin-tier command is refused to a non-admin.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(commands::authorization)'` — expect `admitted_sender_can_be_refused_command` and `admin_tier_enforced`.
  - *Checks:* Resolve the authorization inputs — confirm they read `admin_users` and `user_allowed_commands` from the account (Task 03), not the message-admission allowlist; §Channel command surface separates the two.
  - *Status:* ☐ unverified

- **O3 — An unsupported command returns help rather than entering the agent transcript**
  - *Claim:* An unknown command produces a help reply and appends nothing to the conversation.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(commands::unsupported_returns_help)'` — expect PASS asserting a help reply and an unchanged transcript.
  - *Status:* ☐ unverified

- **O4 — Commands execute before ordinary agent ingress and after sender gating**
  - *Claim:* A message that is a command never reaches the input admission chain, and a gated sender never reaches command execution.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-channels -E 'test(commands::ordering)'` — expect two cases asserting zero admissions for a command and zero command executions for a gated sender.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-channels -E 'test(commands::)'` and sees the eleven-command set with alias, tiered authorization, help-not-transcript, and gate-then-command-then-ingress ordering pass**
  - *Claim:* The command surface module passes with set equality asserted.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `set_matches_spec` case.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-channels/src/access_control.rs` (Task 81) gates before commands; confirm `access_control::gates_first` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

`chat` and `model` delegate to runtime surfaces owned by Tasks 20 and 67; this task owns the command vocabulary and authorization.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
