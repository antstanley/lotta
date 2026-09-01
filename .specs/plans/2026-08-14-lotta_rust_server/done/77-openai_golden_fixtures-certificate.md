
**Task:** [77-openai_golden_fixtures.md](77-openai_golden_fixtures.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-31 — implementation commit `dafc7dd2c496`

> Verification protocol for Task 77. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 77) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** checked-in golden fixtures for all three routes, replayed against the running server with event-by-event SSE comparison.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must compare SSE event by event: `00-overview.md` §Compatibility definition requires golden request/response/SSE fixtures for the three routes, and a whole-body comparison would hide ordering differences.

## Obligations

- **O1 — Golden fixtures exist for all three routes covering JSON and SSE, and the index names every covered case**
  - *Claim:* `fixtures/openai/` contains request/response pairs and SSE captures for the three routes with a machine-readable index.
  - *Evidence to collect:* Run `cargo nextest run --test openai_golden -E 'test(index_is_complete_strict_and_sanitized)'` — expect PASS validating all 11 indexed cases, every route, and both POST response modes.
  - *Evidence:* The strict schema-v2 index names 11 canonical cases: Models JSON; Chat headerless JSON, streaming SSE, two-step stateful history, and idempotent live-join/replay; Responses non-stored JSON, streaming SSE, stored JSON, `previous_response_id`, and no-idempotency. It pins baseline commit `300f923f16cc8eee50656d7da732902c1dea2b65`, tree `30d2e2cebc7761c153f5a6d136b242365092faa0`, Bun 1.3.14, and SHA-256 provenance for the real handler, common module, lockfile, capture runner/adapter, and the real turn bridge. Capture verifies the archived handler hashes and the bridge markers `runTurnViaListenerRuntime` and `dispatchInboundMessageWhenReady`. Two clean recaptures were byte-identical to each other and to the checked-in index and 11 case files. The complete golden suite passed 22/22 twice.
  - *Status:* **SATISFIED**

- **O2 — Replaying each golden request against the running server reproduces the recorded response, with SSE compared event by event**
  - *Claim:* Every golden case matches, and an SSE mismatch reports the first diverging event index.
  - *Evidence to collect:* Run `cargo nextest run --test openai_golden` — expect one passing case per fixture; on failure confirm the message names the diverging event index and both events.
  - *Checks:* Resolve the server the replay drives — confirm it is the built `lotta` binary started with `--openai-api`, not an in-process route handler. An in-process handler would skip the Task 15 transport bounds and the shared authentication policy.
  - *Evidence:* All 11 indexed cases replay through real TCP against `CARGO_BIN_EXE_lotta server --openai-api` with capability authentication and a real HTTP provider stub; no in-process OpenAI handler is used. JSON responses compare structurally, while SSE compares status, selected headers, dynamic map, and every event in order. The comparator mutation matrix rejects missing, extra, reordered, and duplicated events and reports the first divergent event index with both sides. Every case also matches canonical provider-call, conversation create/retain/delete/hidden/fork, cleanup, and idempotency deltas. One case-wide typed identity registry preserves reuse and distinction across JSON, SSE, provider, cursor, and persistence surfaces; timestamps are normalized by response-attempt ordinal and must remain identical within an attempt. The golden suite passed 22/22 twice.
  - *Status:* **SATISFIED**

- **O3 — Chat-completion cases cover stateful, headerless, and idempotent-retry paths; response cases cover stored, non-stored, and `previous_response_id`**
  - *Claim:* All six named cases are present and pass.
  - *Evidence to collect:* Inspect `fixtures/openai/index.json`, then run `cargo nextest run --test openai_golden` — expect all 11 named replay tests, including the six required behavioral paths and their stateful dependencies.
  - *Evidence:* The index and passing replay cases cover `chat_headerless_json`, `chat_stateful_first` → `chat_stateful_newest`, and `chat_idempotent_retry`; Responses covers `responses_nonstored_json`, `responses_stored_json` → `responses_previous_json`, plus streaming and explicit no-idempotency behavior. Stateful dependency order, stored-cursor materialization, hidden fork linkage, live join, settled replay, and canonical observable deltas are asserted rather than inferred. The complete suite passed 22/22 twice over all 11 canonical cases.
  - *Status:* **SATISFIED**

- **O4 — No fixture contains a credential or user content**
  - *Claim:* A scanner over `fixtures/openai/` finds no secret-shaped value and no unsanitized transcript text.
  - *Evidence to collect:* Run `cargo nextest run --test openai_golden -E 'test(index_is_complete_strict_and_sanitized) | test(/sanitize::tests/)'` — expect PASS; read the scanner and confirm it rejects `sk-`, `Bearer `, non-placeholder secret fields, and unsanitized content.
  - *Evidence:* The exhaustive scanner reads the original index and every original fixture value before strict deserialization, recursively allowlists the entire corpus, rejects symlinks, traversal, noncanonical or unindexed paths, stale hashes, unknown fields, over-limit bytes/depth/events, and unapproved entries. It rejects `sk-`, `Bearer `, cookies, token/key fields, private paths, raw UUIDs/dates, free text on every content surface, malformed or field-misplaced placeholders, and non-placeholder authorization values. Mutation tests plant secrets and content across request headers/bodies, JSON, SSE, provider observables, cursor data, paths, hashes, recursive entries, and symlinks and confirm rejection. Both byte-identical recaptures and both 22/22 golden runs passed the exhaustive scan.
  - *Status:* **SATISFIED**

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Golden passed 22/22 twice; `lotta-testkit` passed 210/210; `lotta-app-server` passed 586/586; Chat passed 26/26; and root passed 52/52. Workspace check, `cargo fmt --all --check`, strict all-target/all-feature workspace Clippy with warnings denied, warning-denied workspace rustdoc, and `cargo deny check` all exited 0. Task 77 source audit parses the exact 10 Rust and 3 script sources, pins Bun 1.3.14 for script syntax, and enforces files ≤1,000 lines, production functions ≤70 lines, lines ≤100 columns, and units-last named constants. Workspace accounting records only the accepted external pinned-sibling SHA conformance exclusion; it hides no Task 77 failure and adds no other exclusion.
  - *Status:* **SATISFIED**

- **O6 — Reviewable: a reviewer runs `cargo nextest run --test openai_golden` against a running server with `--openai-api` and sees every golden fixture match, including event-by-event SSE comparison**
  - *Claim:* The golden suite is green for all 11 indexed cases, including all six required behavioral paths.
  - *Evidence to collect:* Run the command and confirm zero failures and a case count equal to the fixture index size.
  - *Evidence:* The production-binary TCP replay passed 22/22 twice and executed one named replay test for each of the 11 indexed cases, with dependency cases replayed in canonical order. It proves exact JSON/SSE outputs, event-index comparison, canonical observables, case-wide IDs, attempt-scoped timestamps, cursor/fork relationships, idempotent live join and settled replay, and cleanup against the running `lotta --openai-api` binary. A clean independent GPT-5.6 Sol review of final commit `dafc7dd2c496` returned `CORRECT / DONE`.
  - *Status:* **SATISFIED**

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/openai/` (Tasks 74–76) serves these fixtures; the literal `openai::chat::idempotency` and `openai::responses::cursor` regressions pass within Chat 26/26 and `lotta-app-server` 586/586, while the 11 production-binary golden replays exercise both surfaces over TCP — **PRESERVED**.

## Residue

Fixtures remain pinned to baseline commit `300f923f16cc8eee50656d7da732902c1dea2b65` and tree `30d2e2cebc7761c153f5a6d136b242365092faa0`; moving either pin requires a fresh deterministic recapture. Workspace accounting records only the accepted external pinned-sibling SHA conformance exclusion; no Task 77 failure or additional exclusion is hidden by it.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: **DONE**
CONFIDENCE: **high**
SUMMARY: Verified 2026-08-31 at implementation commit `dafc7dd2c496`. All O1–O6 are satisfied and regression is preserved. Eleven canonical cases were captured twice, byte-identically, from the real pinned commit/tree, handler, and turn bridge. Golden passed 22/22 twice through the production `lotta --openai-api` binary over TCP with exact JSON and event-by-event SSE comparison, canonical observable deltas, case-wide typed IDs, and attempt-scoped timestamps. The exhaustive sanitizer and mutation matrix are green. `lotta-testkit` passed 210/210, `lotta-app-server` 586/586, Chat 26/26, and root 52/52; workspace check, format, strict Clippy, warning-denied rustdoc, and deny are green. Only the accepted external pinned-sibling SHA conformance exclusion remains. A clean independent GPT-5.6 Sol review returned `CORRECT / DONE`.
