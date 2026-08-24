# Done Certificate — Task 71: WebSocket conversation management command group

**Task:** [71-ws_conversation_management_commands.md](71-ws_conversation_management_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 71. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 71) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** list, retrieve, create, update, recompile, fork, messages, and compact for conversations, including the transcript rewrite that fork requires.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let compact bypass the turn lease: `03-runtime-and-turns.md` §Compaction and prompt refresh requires compaction to be serialized with the lease.

## Obligations

- **O1 — All eight conversation management commands decode, route, and respond, with fixture round-trip coverage**
  - *Claim:* Each command in the Conversation management row has a passing case.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::commands)'` — expect eight cases derived from the fixture group listing.
  - *Status:* ☐ unverified

- **O2 — Fork performs a full transcript rewrite into a new conversation key and leaves the source conversation unchanged**
  - *Claim:* The forked conversation has its own directory and history; the source transcript is byte-identical afterwards.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::fork)'` — expect PASS asserting a new key form directory and an unchanged source hash. `04-persistence-and-memfs.md` §Transcript contract lists fork as one of the three full-rewrite cases.
  - *Status:* ☐ unverified

- **O3 — Compact delegates to the Task 58 flow under the turn lease and writes exactly one compaction entry**
  - *Claim:* A compact command produces one transcript compaction entry and updates in-context IDs.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::compact)'` — expect PASS asserting one entry appended and the ID list updated.
  - *Checks:* Resolve the compaction call — confirm it is the Task 58 lease-serialized flow, not a direct transcript write; a direct write would bypass mod lifecycle callbacks and lease checks.
  - *Status:* ☐ unverified

- **O4 — Messages honours asc/desc, `before`/`after`, `limit`, and the return-message-type filter**
  - *Claim:* Each parameter changes the result as §Required query patterns specifies.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::messages)'` — expect one case per parameter (four) plus a combined case.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::)'` and sees eight commands, fork with an unchanged source, lease-serialized compact, and message filtering pass**
  - *Claim:* The conversation management group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and eight command cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/compaction/` (Task 58) serves compact; confirm `compaction::effects` still passes over the wire : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-store/src/transcript/` (Task 24) performs the fork rewrite; confirm `transcript::no_rewrite_on_compaction` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

The Responses API's hidden fork (Task 76) reuses this fork implementation.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
