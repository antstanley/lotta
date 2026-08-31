
**Task:** [76-openai_responses_route.md](76-openai_responses_route.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-31 — implementation commit `3371ace70c6c`

> Verification protocol for Task 76. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 76) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the Responses subset with unsigned `resp_letta_` cursors, `previous_response_id` hidden fork, and `501 unsupported_backend` when forking is unavailable.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not add idempotency caching to Responses: `02-app-server-api.md` §HTTP API states only Chat Completions honours the idempotency headers at the pinned baseline.

## Obligations

- **O1 — A successful `store: true` request retains its conversation and returns an unsigned `resp_letta_` base64url cursor carrying version, nonce, agent ID, and conversation ID**
  - *Claim:* The cursor decodes to the four fields and no signature is required or verified.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::responses::cursor)'` — expect PASS decoding the cursor and asserting the four fields, matching `01-domain-model.md` §ID scheme.
  - *Checks:* Resolve the cursor encoder — confirm it is unsigned base64url with no HMAC; §ID scheme describes it as an unsigned state cursor, and adding a signature would break client round trips.
  - *Evidence:* The literal cursor selector passed 1/1 over real TCP. It strips `resp_letta_`, rejects padding, base64url-decodes JSON with exactly `version`, UUID `nonce`, `agent_id`, and `conversation_id`, and confirms the retained conversation. `openai/cursor.rs` uses `URL_SAFE_NO_PAD` over that four-field payload with no signature or HMAC. Broad Responses coverage passed 34/34.
  - *Status:* **SATISFIED**

- **O2 — A non-stored request returns `resp_<uuid>`, deletes a headerless ephemeral conversation, and leaves an `X-Letta-Chat-Key` conversation stateful**
  - *Claim:* The three behaviors hold for `store: false`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::responses::non_stored)'` — expect three cases.
  - *Evidence:* The literal non-stored selector passed 3/3. It proves ordinary IDs use `resp_<uuid>` rather than `resp_letta_`, one canonical ephemeral teardown removes every headerless conversation artifact, and `X-Letta-Chat-Key` retains exactly one stateful conversation. The production traces also end with empty active/runtime registries and the expected retained-artifact counts.
  - *Status:* **SATISFIED**

- **O3 — A valid `previous_response_id` creates a hidden fork, and `501 unsupported_backend` is returned when forking is unavailable**
  - *Claim:* The fork is not visible in the conversation listing, and the unavailable path returns 501 with the named code.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::responses::previous_response_id)'` — expect `creates_hidden_fork` (asserting the fork is hidden from listing) and `unsupported_backend_501`.
  - *Checks:* Resolve the fork call — confirm it is the Task 71 conversation fork, so the transcript rewrite and key-form rules apply.
  - *Evidence:* The literal previous-response selector passed 3/3. A stored source produces a distinct fork whose persisted record has `hidden: true`, while the source remains visible; a non-stored fork is cleaned without deleting retained ancestors; malformed cursors return exact `response_not_found`. The capability-501 case returns exact HTTP 501/`unsupported_backend` before repository lookup, fork, deletion, allocation, execution, owner registration, or setup-lock creation. `execution::allocate` calls the Task 71 `fork_for_openai` repository path, and Task 71 fork regression coverage passed 2/2.
  - *Status:* **SATISFIED**

- **O4 — Failed outcomes are not stored and no idempotency-key caching exists on this route**
  - *Claim:* A failed request leaves no stored response, and an `Idempotency-Key` header has no effect here.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::responses::no_idempotency)'` — expect `failed_outcome_not_stored` and `idempotency_header_ignored`, matching `02-app-server-api.md` §HTTP API (`Responses does not implement idempotency-key caching at the pinned baseline`).
  - *Evidence:* The literal no-idempotency selector passed 2/2. Two concurrent requests carrying the same header receive distinct response IDs, execute two independent admissions/turns, perform two teardowns, and leave zero artifacts. A failed `store: true` JSON response and failed SSE response never expose `resp_letta_`; both clean runtime, fork/store artifacts, and keyed ownership. The production Responses no-idempotency/failure trace independently proves two provider calls and distinct IDs, followed by failed JSON/SSE terminal states with no stored cursor.
  - *Status:* **SATISFIED**

- **O5 — Streaming follows OpenAI event names and ends in a completed or failed response state**
  - *Claim:* The SSE event names match the OpenAI Responses names and the stream terminates in one of the two states.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::responses::streaming)'` — expect an event-name case and both terminal-state cases.
  - *Evidence:* The literal streaming selector passed 6/6. SSE emits `response.created`, `response.in_progress`, exact output-item/content-part/text delta/done events, monotonic sequence numbers, and exactly one `response.completed` or `response.failed` terminal event, with no Chat `[DONE]` sentinel. The production live-SSE trace observes `response.output_text.delta` before the gated provider is released and observes completion only afterward. Production real-tool JSON/SSE traces expose exact successful and failed tool outputs, call IDs, statuses, ordering, and final text; the separate public WebSocket trace remains unchanged and intentionally omits the internal tool output/result fields. Unread/backpressured SSE does not own execution or block an unrelated response, and disconnect plus shutdown cancels and joins the owner cleanly.
  - *Status:* **SATISFIED**

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Fresh literal selectors passed cursor 1/1, non-stored 3/3, previous-response 3/3, no-idempotency 2/2, and streaming 6/6. Broad Responses passed 34/34; production Responses passed 4/4; `lotta-app-server` passed 586/586; root passed 52/52; `lotta-store` passed 208/208; and source audit passed 7/7. Workspace check, strict all-target/all-feature Clippy with warnings denied, `cargo fmt --all --check`, warning-denied workspace rustdoc, and `cargo deny check` all exited 0. Source audit confirms units-last named bounds, 70-line/100-column limits, no production panic/unwrap/expect or lint escape, and registered supervised teardown. `OPENAI_RESPONSES_OWNERS_MAX` bounds listener-owned work; active owner admission is fail-fast and recovers immediately. Agent-scoped setup is short and serialized through synchronous `SetupLease` RAII, which prunes exact lock entries on success, cancellation, panic, waiter wakeup, and shutdown. Cursor/fork/store cleanup is canonical and no Responses idempotency cache exists.
  - *Status:* **SATISFIED**

- **O7 — Reviewable: a reviewer posts a `store: true` request, decodes the returned `resp_letta_` cursor, then posts a second request with `previous_response_id` set to it and observes a hidden fork continuing the conversation**
  - *Claim:* The cursor and hidden fork behave as specified from outside the process.
  - *Evidence to collect:* Run the two curls; base64url-decode the cursor and confirm the four fields; confirm the follow-up response continues the thread and that the fork is absent from `/v1/models`-visible conversation listings.
  - *Evidence:* The genuine production-listener O7 trace posts a stored request over TCP, decodes its four-field `resp_letta_` cursor, and records one provider call plus exactly one retained source conversation beyond the seeded fixture. A second TCP request with `previous_response_id` returns a distinct stored cursor, creates exactly one hidden fork, and sends the inherited prior user/assistant history plus only the current instructions/input to the provider. Persisted counts prove the source has one user and two assistant records; the fork has two user and four assistant records, with current input exactly once. A third live SSE request forks that stored fork, inherits `remember green`, applies only its current instructions/input, streams ordered events through `response.completed`, then cleans its non-stored fork while the two stored conversations remain. Active/runtime registries are empty after every turn. The production Responses suite passed 4/4, and a clean independent GPT-5.6 Sol review of final commit `3371ace70c6c` returned `CORRECT / DONE` and confirmed O1–O7.
  - *Status:* **SATISFIED**

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/openai/chat.rs` (Task 75) shares the conversation-allocation path; `openai::chat::keying` passed 3/3, proving explicit/stateful and headerless allocation/teardown behavior remains intact — **PRESERVED**.
- `crates/lotta-app-server/src/ws/groups/conversations.rs` (Task 71) supplies fork; `ws::conversations::fork` passed 2/2, proving hidden fork transcript rewrite, key form, visibility, and source preservation remain intact — **PRESERVED**.

## Residue

The cursor is unsigned by design; treating it as a capability token would be a security change requiring a change spec. Responses intentionally has no idempotency-key caching. Workspace accounting retains only the established external pinned-sibling SHA conformance exclusions and provider-host cases requiring their dedicated provider environment; no Task 76 failure or new exclusion is hidden by them.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: **DONE**
CONFIDENCE: **high**
SUMMARY: Verified 2026-08-31 at implementation commit `3371ace70c6c`. All O1–O7 are satisfied. Literal cursor 1/1, non-stored 3/3, previous-response 3/3, no-idempotency 2/2, and streaming 6/6 passed; broad Responses passed 34/34, production Responses 4/4, `lotta-app-server` 586/586, root 52/52, `lotta-store` 208/208, and source audit 7/7. Workspace check, strict Clippy, format, warning-denied rustdoc, and deny are green. Real TCP traces prove the unsigned four-field cursor, capability-first exact 501, hidden inherited-history forks and persisted counts, live SSE before provider release, exact real tool outputs while the public WebSocket shape remains unchanged, canonical cursor/fork/store cleanup, and intentional absence of Responses idempotency. Named owner capacity and setup-lock RAII quiesce on completion, failure, panic, disconnect, and shutdown. Task 75 keying 3/3 and Task 71 fork 2/2 preserve both downstream regressions. A clean independent GPT-5.6 Sol review returned `CORRECT / DONE`.
