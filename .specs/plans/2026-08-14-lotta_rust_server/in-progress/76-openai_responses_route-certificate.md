# Done Certificate — Task 76: OpenAI `/v1/responses`, cursors, and hidden fork

**Task:** [76-openai_responses_route.md](76-openai_responses_route.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

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
  - *Status:* ☐ unverified

- **O2 — A non-stored request returns `resp_<uuid>`, deletes a headerless ephemeral conversation, and leaves an `X-Letta-Chat-Key` conversation stateful**
  - *Claim:* The three behaviors hold for `store: false`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::responses::non_stored)'` — expect three cases.
  - *Status:* ☐ unverified

- **O3 — A valid `previous_response_id` creates a hidden fork, and `501 unsupported_backend` is returned when forking is unavailable**
  - *Claim:* The fork is not visible in the conversation listing, and the unavailable path returns 501 with the named code.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::responses::previous_response_id)'` — expect `creates_hidden_fork` (asserting the fork is hidden from listing) and `unsupported_backend_501`.
  - *Checks:* Resolve the fork call — confirm it is the Task 71 conversation fork, so the transcript rewrite and key-form rules apply.
  - *Status:* ☐ unverified

- **O4 — Failed outcomes are not stored and no idempotency-key caching exists on this route**
  - *Claim:* A failed request leaves no stored response, and an `Idempotency-Key` header has no effect here.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::responses::no_idempotency)'` — expect `failed_outcome_not_stored` and `idempotency_header_ignored`, matching `02-app-server-api.md` §HTTP API (`Responses does not implement idempotency-key caching at the pinned baseline`).
  - *Status:* ☐ unverified

- **O5 — Streaming follows OpenAI event names and ends in a completed or failed response state**
  - *Claim:* The SSE event names match the OpenAI Responses names and the stream terminates in one of the two states.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(openai::responses::streaming)'` — expect an event-name case and both terminal-state cases.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer posts a `store: true` request, decodes the returned `resp_letta_` cursor, then posts a second request with `previous_response_id` set to it and observes a hidden fork continuing the conversation**
  - *Claim:* The cursor and hidden fork behave as specified from outside the process.
  - *Evidence to collect:* Run the two curls; base64url-decode the cursor and confirm the four fields; confirm the follow-up response continues the thread and that the fork is absent from `/v1/models`-visible conversation listings.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/openai/chat.rs` (Task 75) shares the conversation-allocation path; confirm `openai::chat::keying` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-app-server/src/ws/groups/conversations.rs` (Task 71) supplies fork; confirm `ws::conversations::fork` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

The cursor is unsigned by design; treating it as a capability token would be a security change requiring a change spec.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
