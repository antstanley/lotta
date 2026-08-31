
**Task:** [75-openai_chat_completions_route.md](75-openai_chat_completions_route.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-31 — implementation commit `f95e3a304dc3`

> Verification protocol for Task 75. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 75) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** stateful, header-keyed, and stateless chat completions with `Idempotency-Key` caching, in-flight sharing, and failed-outcome eviction.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must check the idempotency cache before conversation allocation: `02-app-server-api.md` §HTTP API states it checks the cache before conversation allocation, so a retry cannot orphan a conversation.

## Obligations

- **O1 — `X-Letta-Chat-Key` pins one persisted conversation, `X-OpenWebUI-Chat-Id` is accepted for streaming, and a headerless request uses an ephemeral conversation deleted after settlement**
  - *Claim:* The three modes behave as `02-app-server-api.md` §HTTP API specifies.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::chat::keying)'` — expect `chat_key_pins_conversation`, `openwebui_id_accepted_for_streaming`, and `headerless_ephemeral_deleted_after_settlement`. Compare header names with `../letta-code/src/websocket/app-server-openai-common.ts:331,344,354`.
  - *Checks:* Resolve the header lookup — confirm `x-letta-chat-key` takes precedence over `X-OpenWebUI-Chat-Id`, matching the baseline where the explicit key always pins chat identity.
  - *Evidence:* Focused chat coverage passed 26/26. Keying proves explicit `X-Letta-Chat-Key` precedence and streaming `X-OpenWebUI-Chat-Id` acceptance. Real-listener production coverage proves one persistent keyed allocation is retained, while headerless execution uses the canonical ephemeral scope teardown and leaves zero conversation artifacts after settlement, panic, and supervised shutdown.
  - *Status:* **SATISFIED**

- **O2 — Stateful requests send only the newest user input, while headerless requests replay user/assistant transcript content**
  - *Claim:* A stateful follow-up sends one new message; a headerless request replays the prior turns.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::chat::input_selection)'` — expect two cases asserting the message count reaching the provider fake.
  - *Evidence:* The input-selection cases passed within chat 26/26. They prove a keyed request selects only the newest usable user turn and a headerless request preserves user/assistant order and content, including assistant output-text and user text/image history variants. The complete request is bounded before selection.
  - *Status:* **SATISFIED**

- **O3 — Idempotency caching checks before conversation allocation, shares an in-flight turn, replays a successful settled outcome, and evicts a failed one**
  - *Claim:* The four idempotency behaviors hold for both header spellings.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::chat::idempotency)'` — expect `checks_before_allocation` (asserting zero conversations created on a cache hit), `shares_in_flight`, `replays_settled_success`, and `evicts_failed_outcome`, each run for `Idempotency-Key` and `X-Idempotency-Key`.
  - *Evidence:* Chat 26/26 and the real production O7 trace prove the cache claim precedes allocation, concurrent aliases share the same active cell, successful settlement replays, and a failed entry is removed before waiter wakeup so a retry can safely become owner. Exact headerless retries share a structural fingerprint; distinct requests and delimiter-collision candidates do not alias. Standard `Idempotency-Key` has defined precedence over `X-Idempotency-Key`.
  - *Status:* **SATISFIED**

- **O4 — `OPENAI_CHAT_KEYS_MAX` and `CHAT_IDEMPOTENCY_OUTCOMES_MAX` evict FIFO, and an evicted chat key causes the next request to create a new conversation**
  - *Claim:* Both caches are bounded and eviction has the documented observable effect.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::chat::cache_bounds)'` — expect below/at/above cases for both bounds plus `evicted_key_creates_new_conversation`.
  - *Evidence:* Cache-bound cases passed within chat 26/26. Both caches accept below/at bound, reject overflow when every entry is active, never evict an allocating/in-flight owner, and admit overflow only by settled FIFO reuse. A settled chat-key eviction makes a later claim a new owner, which allocates a new conversation rather than aliasing the evicted mapping.
  - *Status:* **SATISFIED**

- **O5 — Both JSON and SSE responses are served, and a request with no usable user text or image content is rejected**
  - *Claim:* `stream: false` yields JSON, `stream: true` yields SSE, and an empty-content request is an invalid request.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::chat::responses)'` — expect the JSON case, the SSE case, and the rejection case.
  - *Evidence:* Response coverage passed within chat 26/26: JSON has the pinned completion shape; SSE has ordered late-join history replay, terminal `finish_reason`, and `[DONE]`; settled JSON/SSE projections reuse one exact completion identity; only literal `true` enables streaming; unusable user content is rejected. Real-listener tests also prove exact external `model_not_found` and invalid-request OpenAI envelopes.
  - *Status:* **SATISFIED**

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Fresh chat passed 26/26, full `lotta-app-server` passed 558/558, root passed 48/48, and source audit passed 7/7. Strict workspace check, all-target/all-feature workspace Clippy with warnings denied, `cargo fmt --all --check`, workspace rustdoc with warnings denied, and `cargo deny check` all exited 0. Source audit proves units-last named limits, bounded functions/files/columns, no production panic/unwrap/expect or lint escape, registered supervised tasks, and canonical teardown. Workspace test accounting retains only the established external pinned-sibling SHA conformance exclusions and provider-host cases requiring their dedicated provider environment; no Task 75 or new exclusion is hidden by them.
  - *Status:* **SATISFIED**

- **O7 — Reviewable: a reviewer sends two `curl` requests with the same `Idempotency-Key`, the second while the first is still streaming, and observes one shared turn and one conversation created, then repeats after settlement and observes a replayed outcome**
  - *Claim:* Idempotency sharing and replay are observable from outside the process.
  - *Evidence to collect:* Run the three curls against a running server with `--openai-api`; confirm the server log shows one conversation allocation and one provider turn across all three.
  - *Evidence:* The genuine production-listener O7 test concurrently sends JSON and SSE requests with `Idempotency-Key: o7-key`, then sends settled JSON replay through `X-Idempotency-Key: o7-key`. Both concurrent responses are 200 with their exact media types and the replay is 200, while instrumentation records exactly one conversation allocation, one admission, and one turn (`["admit", "continue"]` exactly once each), no lingering subscription, exactly one canonical ephemeral teardown, and zero retained conversation artifacts. This proves cache-before-allocation, concurrent alias sharing, JSON/SSE projection, settled alias replay, and external lifecycle cleanup on the production route. A clean independent GPT-5.6 Sol review of final commit `f95e3a304dc3` returned `CORRECT / DONE` and confirmed O1–O7.
  - *Status:* **SATISFIED**

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/branches.rs` (Task 55) still executes the canonical turn: real-listener O7 records one admission and one continuation, projection/history variants pass, and the app-server suite is 558/558 — **PRESERVED**.
- `crates/lotta-app-server/src/bounds.rs` (Task 15) still defines both cache bounds: active-safe below/at/above FIFO coverage passes within chat 26/26 and source audit 7/7 confirms named constants — **PRESERVED**.

## Residue

Responses does not implement idempotency-key caching at the pinned baseline; Task 76 must not add it. Workspace test accounting excludes only the established external pinned-sibling SHA conformance cases and provider-host cases requiring a dedicated provider environment; these are not Task 75 failures and no new exclusion is recorded.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: **DONE**
CONFIDENCE: **high**
SUMMARY: Verified 2026-08-31 at implementation commit `f95e3a304dc3`. All O1–O7 are satisfied. Chat passed 26/26, `lotta-app-server` 558/558, root 48/48, and source audit 7/7; strict workspace check, all-target/all-feature Clippy, format, warning-denied rustdoc, and deny are green. The production O7 listener trace proves one cache claim before one conversation allocation, one admission, and one turn shared by concurrent JSON/SSE requests across both idempotency-header aliases, followed by exact settled replay and one canonical teardown with no artifacts. Active owners are never evicted, failed outcomes become safely retryable, supervised panic/shutdown/unread-SSE paths settle waiters and join owners/writers, and projection/history variants are covered. Both downstream regressions are preserved. Only established external pinned-SHA conformance and dedicated-provider-environment exclusions remain, and a clean independent GPT-5.6 Sol review returned `CORRECT / DONE`.
