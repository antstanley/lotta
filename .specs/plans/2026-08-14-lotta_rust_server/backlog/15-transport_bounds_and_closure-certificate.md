# Done Certificate — Task 15: Transport bounds, error envelopes, and closure

**Task:** [15-transport_bounds_and_closure.md](15-transport_bounds_and_closure.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 15. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 15) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the nine transport bounds enforced before proportional work, plus stable HTTP/WebSocket error envelopes and half-open reaping.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not narrow the baseline's accepted range: `02-app-server-api.md` §Assumptions *Transport hardening* states the byte caps make implicit dependency bounds explicit rather than tightening them.

## Obligations

- **O1 — All nine §Transport bounds constants exist with the spec's names and defaults and have below/at/above tests**
  - *Claim:* `HTTP_BODY_BYTES_MAX`, `WS_FRAME_BYTES_MAX`, `WS_MESSAGE_FIELDS_MAX`, `WS_PING_INTERVAL_MS`, `WS_PONG_TIMEOUT_MS`, `AUTH_CLOCK_SKEW_SECONDS_MAX`, `REQUEST_ID_BYTES_MAX`, `CHAT_IDEMPOTENCY_OUTCOMES_MAX`, and `OPENAI_CHAT_KEYS_MAX` are defined with the table's defaults.
  - *Evidence to collect:* Read `crates/lotta-app-server/src/bounds.rs` and compare each name and value against the `02-app-server-api.md` §Transport bounds table. Run `cargo nextest run -p lotta-app-server -E 'test(bounds::)'` — expect three cases (below, at, above) per bound, per `development-guidelines.md` §Testing.
  - *Status:* ☐ unverified

- **O2 — Oversized input is rejected before proportional work: the body cap fires before JSON decode, the frame cap closes with 1009, and the field cap fires before field-proportional work**
  - *Claim:* A 21 MiB body is rejected without being parsed; a 101 MiB frame closes the socket with 1009; a 4,097-field message is rejected before per-field processing.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(bounds::reject_before_work)'` — expect three passing cases. Read the body case and confirm the assertion is on the absence of a parse attempt (for example a counter or a spy), not merely on the response status.
  - *Checks:* Resolve the size check's position relative to `serde_json::from_slice` in the request path — confirm the check precedes the call. `architecture-principles.md` §Limits requires boundary validation before allocation proportional to untrusted input.
  - *Status:* ☐ unverified

- **O3 — Half-open sockets are reaped: a client that stops answering pings is terminated after `WS_PONG_TIMEOUT_MS`**
  - *Claim:* With a fake clock, a socket that never pongs is closed once the pong timeout elapses and not before.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(heartbeat::)'` — expect `closes_after_pong_timeout` and `stays_open_before_timeout` to pass. Confirm the test advances the Task 06 `Clock` port rather than sleeping.
  - *Status:* ☐ unverified

- **O4 — Errors use stable envelopes: HTTP status plus JSON body, pre-upgrade WebSocket failures as HTTP 400/401/403/503, protocol errors correlated to `request_id`, and internal errors scrubbed with an incident ID retained only in diagnostics**
  - *Claim:* Each error class produces its documented shape and no internal detail leaks to the client.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(errors::)'` — expect one case per class named in `02-app-server-api.md` §Errors and closure. Read the scrubbing case and confirm the client-visible body contains the incident ID but no file path, no stack, and no secret.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(bounds::) + test(errors::) + test(heartbeat::)'` and sees below/at/above coverage for all nine bounds, the four error classes, and half-open reaping pass**
  - *Claim:* The bounds, errors, and heartbeat modules pass with 27 boundary cases.
  - *Evidence to collect:* Run the filter and confirm zero failures and at least 27 bound cases in the summary.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/listener.rs` (Task 14) now routes through the bound checks; confirm `auth::non_loopback_requires_auth` and `listener::url_resolution` still pass : ☐ (PRESERVED / REGRESSION)

## Residue

`CHAT_IDEMPOTENCY_OUTCOMES_MAX` and `OPENAI_CHAT_KEYS_MAX` are defined here but exercised by Task 75, which owns the chat-key and idempotency caches.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
