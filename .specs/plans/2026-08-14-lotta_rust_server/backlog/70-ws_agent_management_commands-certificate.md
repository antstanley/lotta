# Done Certificate — Task 70: WebSocket agent management command group

**Task:** [70-ws_agent_management_commands.md](70-ws_agent_management_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 70. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 70) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** create shortcut plus list, retrieve, create, update, and delete for agents, backed by the Task 28 query behaviors.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not return success before agent creation's side effects complete: `01-domain-model.md` §Agent requires them before success is returned.

## Obligations

- **O1 — All six agent management commands decode, route, and respond, with fixture round-trip coverage**
  - *Claim:* Each command in the Agent management row has a passing case.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::agents::commands)'` — expect six cases derived from the fixture group listing.
  - *Status:* ☐ unverified

- **O2 — Agent creation with local MemFS completes all four side effects before returning success**
  - *Claim:* The Git-memory tag, initialized memory files, the created repository, and the compiled default prompt all exist when the response is sent.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::agents::create_side_effects)'` — expect PASS asserting all four before the response is observed, per `01-domain-model.md` §Agent.
  - *Checks:* Resolve the ordering of the response emission relative to prompt compilation — confirm compilation completes first; returning early would let a client observe an agent with no compiled prompt.
  - *Status:* ☐ unverified

- **O3 — List ordering is deterministic and the filters behave as §Required query patterns specifies**
  - *Claim:* Repeated list calls return identical order and each filter narrows correctly.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::agents::listing)'` — expect a determinism case plus one case per filter (name, query, tag, hidden).
  - *Status:* ☐ unverified

- **O4 — An absent agent or a wrong local prefix returns 404, and `AGENTS_MAX` rejects creation at the limit**
  - *Claim:* Both lookup failures are 404-class and the creation cap rejects above the limit.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::agents::errors)'` — expect two 404 cases and below/at/above cases for `AGENTS_MAX`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::agents::)'` and sees six commands, complete creation side effects, deterministic filtered listing, and the 404 and cap paths pass**
  - *Claim:* The agent management group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and six command cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/query/` (Task 28) serves the listing; confirm `query::deterministic_order` still passes over the wire : ☐ (PRESERVED / REGRESSION)

## Residue

Conversation-scoped commands are Task 71.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
