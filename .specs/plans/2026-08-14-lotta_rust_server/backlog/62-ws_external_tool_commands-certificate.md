# Done Certificate — Task 62: WebSocket external-tool command group

**Task:** [62-ws_external_tool_commands.md](62-ws_external_tool_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 62. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 62) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** runtime external-tool update and external-tool-call response commands routed to the Task 41 registry with correct correlation.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not resolve a call through a different connection: `05-tools-and-extensions.md` §External tools and MCP requires resolution through the originating connection.

## Obligations

- **O1 — The external-tool update command applies atomically and the call request message carries all six correlation fields**
  - *Claim:* An update group with an invalid member changes nothing, and the emitted call request contains runtime, request ID, tool call ID, name, arguments, and scope ID.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::external_tools::update)'` — expect `atomic_group` and `call_request_fields` to pass.
  - *Status:* ☐ unverified

- **O2 — A response resolves only through its originating connection and is rejected on any correlation mismatch**
  - *Claim:* A response from another connection, a wrong request ID, or a wrong tool call ID is rejected and the call stays pending.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::external_tools::correlation)'` — expect three rejection cases, each asserting the call remains pending.
  - *Checks:* Resolve the connection handle used to complete the call — confirm it is the pending call's recorded originator, not the connection that sent the response.
  - *Status:* ☐ unverified

- **O3 — Owner disconnect resolves pending calls with the typed owner-disconnected result**
  - *Claim:* Dropping the owning connection completes each pending call with that outcome rather than leaving it hanging.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::external_tools::owner_disconnect)'` — expect PASS asserting every pending call resolved with the owner-disconnected outcome.
  - *Status:* ☐ unverified

- **O4 — Every discriminant in this group has a decode round-trip test against `fixtures/protocol/discriminants.json`**
  - *Claim:* The group's command and message discriminants all round-trip.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::external_tools::fixture_round_trip)'` — expect one case per discriminant in the group, derived from the fixture.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::external_tools::)'` and sees atomic updates, correlation rejection, owner-disconnect resolution, and fixture round-trips pass**
  - *Claim:* The external-tool command group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and three correlation cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/external/` (Task 41) owns the registry; confirm `external::owner_disconnect` still passes when driven over the wire : ☐ (PRESERVED / REGRESSION)

## Residue

Registration at `runtime_start` is handled by Task 20's `runtime_start` path; this task owns the update and response commands.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
