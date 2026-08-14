# Done Certificate — Task 75: OpenAI `/v1/chat/completions` statefulness and idempotency

**Task:** [75-openai_chat_completions_route.md](75-openai_chat_completions_route.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

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
  - *Status:* ☐ unverified

- **O2 — Stateful requests send only the newest user input, while headerless requests replay user/assistant transcript content**
  - *Claim:* A stateful follow-up sends one new message; a headerless request replays the prior turns.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::chat::input_selection)'` — expect two cases asserting the message count reaching the provider fake.
  - *Status:* ☐ unverified

- **O3 — Idempotency caching checks before conversation allocation, shares an in-flight turn, replays a successful settled outcome, and evicts a failed one**
  - *Claim:* The four idempotency behaviors hold for both header spellings.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::chat::idempotency)'` — expect `checks_before_allocation` (asserting zero conversations created on a cache hit), `shares_in_flight`, `replays_settled_success`, and `evicts_failed_outcome`, each run for `Idempotency-Key` and `X-Idempotency-Key`.
  - *Status:* ☐ unverified

- **O4 — `OPENAI_CHAT_KEYS_MAX` and `CHAT_IDEMPOTENCY_OUTCOMES_MAX` evict FIFO, and an evicted chat key causes the next request to create a new conversation**
  - *Claim:* Both caches are bounded and eviction has the documented observable effect.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::chat::cache_bounds)'` — expect below/at/above cases for both bounds plus `evicted_key_creates_new_conversation`.
  - *Status:* ☐ unverified

- **O5 — Both JSON and SSE responses are served, and a request with no usable user text or image content is rejected**
  - *Claim:* `stream: false` yields JSON, `stream: true` yields SSE, and an empty-content request is an invalid request.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::chat::responses)'` — expect the JSON case, the SSE case, and the rejection case.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer sends two `curl` requests with the same `Idempotency-Key`, the second while the first is still streaming, and observes one shared turn and one conversation created, then repeats after settlement and observes a replayed outcome**
  - *Claim:* Idempotency sharing and replay are observable from outside the process.
  - *Evidence to collect:* Run the three curls against a running server with `--openai-api`; confirm the server log shows one conversation allocation and one provider turn across all three.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/branches.rs` (Task 55) executes the turn; confirm `turn::branches` still passes when driven over HTTP : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-app-server/src/bounds.rs` (Task 15) defines both caches' bounds; confirm `bounds::` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Responses does not implement idempotency-key caching at the pinned baseline; Task 76 must not add it.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
