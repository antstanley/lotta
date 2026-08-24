# Done Certificate — Task 71: WebSocket conversation management command group

**Task:** [71-ws_conversation_management_commands.md](71-ws_conversation_management_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-24

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
  - *Evidence:* Exact selector passed 8/8, one substantive case per command. Responses use dedicated pinned WS DTOs — flattened stored-message objects and pinned message-type strings with golden JSON assertions — matching the pinned protocol field-for-field.
  - *Status:* ☑ SATISFIED

- **O2 — Fork performs a full transcript rewrite into a new conversation key and leaves the source conversation unchanged**
  - *Claim:* The forked conversation has its own directory and history; the source transcript is byte-identical afterwards.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::fork)'` — expect PASS asserting a new key form directory and an unchanged source hash. `04-persistence-and-memfs.md` §Transcript contract lists fork as one of the three full-rewrite cases.
  - *Evidence:* Exact selector passed 2/2. Fork writes a new key-form directory via one atomic staged rewrite carrying inherited history; the source transcript is SHA-256 hash-asserted byte-identical afterwards.
  - *Status:* ☑ SATISFIED

- **O3 — Compact delegates to the Task 58 flow under the turn lease and writes exactly one compaction entry**
  - *Claim:* A compact command produces one transcript compaction entry and updates in-context IDs.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::compact)'` — expect PASS asserting one entry appended and the ID list updated.
  - *Checks:* Resolve the compaction call — confirm it is the Task 58 lease-serialized flow, not a direct transcript write; a direct write would bypass mod lifecycle callbacks and lease checks.
  - *Evidence:* Exact selector passed 5/5. Compact routes through the Task 58 lease-serialized `CompactionService` flow via the authoritative production runtime authority (real registry lifecycle, registered production compaction service) — exactly one entry appended, in-context IDs updated, an active turn blocks with zero durable writes (hash-proven), and lease release is async/deterministic under registry contention (mutation-verified test).
  - *Status:* ☑ SATISFIED

- **O4 — Messages honours asc/desc, `before`/`after`, `limit`, and the return-message-type filter**
  - *Claim:* Each parameter changes the result as §Required query patterns specifies.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::messages)'` — expect one case per parameter (four) plus a combined case.
  - *Evidence:* Exact selector passed 8/8 covering asc/desc ordering, before/after cursors, limit+has_more, return-message-type filtering, and a combined case over pinned DTO shapes.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace rustdoc with warnings denied, and `cargo deny check` passed; full lotta-app-server 446/446. Workspace residue remains confined to the recorded external pinned-SHA/CLI family. New functions stay within 70 lines and 100 columns; constants use units-last names; no production unwrap/expect and no new lint suppressions.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::conversations::)'` and sees eight commands, fork with an unchanged source, lease-serialized compact, and message filtering pass**
  - *Claim:* The conversation management group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and eight command cases.
  - *Evidence:* Broad selector passed 32/32 with zero failures covering eight commands, fork with unchanged source, lease-serialized compact (including the contention regression test), and message filtering. Independent round-3 Sol-xHigh verification returned `CORRECT / DONE`.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/compaction/` (Task 58) serves compact; exact `compaction::effects` passed 6/6: ☑ PRESERVED
- `crates/lotta-store/src/transcript/` (Task 24) performs the fork rewrite; exact `transcript::no_rewrite_on_compaction` passed 1/1: ☑ PRESERVED

## Residue

The Responses API's hidden fork (Task 76) reuses this fork implementation.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 obligations are discharged by exact selectors and traced evidence across three review rounds. The eight conversation commands decode, route, and respond through pinned WS DTOs with golden JSON coverage; fork rewrites the transcript atomically into a new key-form directory leaving the source byte-identical; compact delegates to the Task 58 lease-serialized flow through the authoritative production runtime authority — an active turn blocks with zero writes and lease release is async/deterministic under registry contention (mutation-verified); messages honour asc/desc, before/after, limit, and type filtering over pinned shapes. Conversation IDs are allocated atomically under the store lock with the cap enforced once, recompile always renders fresh content, and both regression suites are preserved.
