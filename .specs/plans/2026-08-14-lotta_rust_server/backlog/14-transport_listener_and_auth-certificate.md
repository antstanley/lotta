# Done Certificate — Task 14: Transport listener, capability-token and signed-bearer authentication

**Task:** [14-transport_listener_and_auth.md](14-transport_listener_and_auth.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 14. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 14) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** an Axum listener that binds the baseline URL forms and enforces both `--ws-auth` modes, rejecting unauthenticated Origin-bearing clients even on loopback.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not add a blanket loopback exemption: `02-app-server-api.md` §Responsibilities item 2 requires authenticating Origin-bearing clients, and the baseline rejects unauthenticated Origin-bearing requests on loopback.

## Obligations

- **O1 — Both `--ws-auth` modes work with the exact baseline flag surface, and capability-token mode rejects a configuration supplying both a token file and a digest**
  - *Claim:* `capability-token` and `signed-bearer-token` are the only accepted mode values; supplying `--ws-token-file` and `--ws-token-sha256` together fails at startup.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(auth::modes)'` — expect `accepts_capability_token`, `accepts_signed_bearer_token`, `rejects_unknown_mode`, and `rejects_token_file_and_digest_together` to pass. Compare the two accepted mode strings against `../letta-code/src/websocket/app-server-auth.ts:11` (`type WebsocketAuthCliMode = "capability-token" | "signed-bearer-token"`).
  - *Checks:* Resolve the token comparison call — confirm it is a constant-time comparison helper, not `==` on the decoded bytes. `NAME SHADOWING` risk: a local `verify` helper must not shadow the HMAC `verify` used by signed-bearer mode.
  - *Status:* ☐ unverified

- **O2 — Signed-bearer verification requires HS256 and `exp`, honours optional `nbf`, issuer, and audience, and rejects a configured skew above `AUTH_CLOCK_SKEW_SECONDS_MAX`**
  - *Claim:* A token with a missing `exp`, a wrong issuer, a wrong audience, or a non-HS256 algorithm is rejected; `--ws-max-clock-skew-seconds 301` fails at startup.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(auth::signed_bearer)'` — expect the four rejection cases plus `accepts_valid_token` to pass. Run `cargo nextest run -p lotta-app-server -E 'test(auth::skew_ceiling)'` — expect the 300 boundary to pass and 301 to fail startup, per the `AUTH_CLOCK_SKEW_SECONDS_MAX` row of `02-app-server-api.md` §Transport bounds.
  - *Checks:* Resolve the clock read used for `exp`/`nbf` — confirm it comes from the `Clock` port (Task 06), not `SystemTime::now`, so the skew tests are deterministic.
  - *Status:* ☐ unverified

- **O3 — A non-loopback bind without authentication fails before the socket listens, and an unauthenticated Origin-bearing request is rejected even on loopback**
  - *Claim:* Starting with `--listen ws://0.0.0.0:4500` and no `--ws-auth` returns an error and never binds; a loopback upgrade carrying an `Origin` header without credentials is rejected.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(auth::non_loopback_requires_auth)'` — expect PASS and read the test to confirm it asserts the port is still closed after the failure. Run `cargo nextest run -p lotta-app-server -E 'test(auth::origin_bearing_loopback_rejected)'` — expect PASS; compare with the baseline behavior at `../letta-code/src/websocket/app-server.ts` where an unauthenticated Origin-bearing request is rejected on loopback.
  - *Status:* ☐ unverified

- **O4 — URL resolution matches the baseline: bare `--listen` binds `ws://127.0.0.1:0`, `/ws` is the default path, a listen-URL path overrides it, and `/` is accepted for upgrade**
  - *Claim:* The four URL cases resolve as specified and startup prints the resolved base, WebSocket, and optional OpenAI URLs.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(listener::url_resolution)'` — expect four passing cases. Trace: `--listen` → bind address `127.0.0.1:0`, ws path `/ws`; `--listen ws://127.0.0.1:9000/socket` → ws path `/socket`; upgrade at `/` → accepted.
  - *Status:* ☐ unverified

- **O5 — Secret files are read once at startup and never appear in a log line, an error message, or a debug rendering**
  - *Claim:* The token and shared-secret values are wrapped in the Task 05 `Secret` type and no formatting path can emit them.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(auth::secrets_never_logged)'` — expect PASS; read the test and confirm it captures the tracing subscriber output across a failed and a successful authentication and asserts the secret literal is absent from both.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer starts the binary with `--listen ws://0.0.0.0:4500` and no `--ws-auth` and sees it exit before binding, then restarts with `--ws-auth capability-token --ws-token-file <path>` and sees an authenticated client connect while an unauthenticated Origin-bearing client is rejected**
  - *Claim:* The three connection outcomes are observable from outside the process.
  - *Evidence to collect:* Run the two startups; for the first confirm a non-zero exit and that `ss -ltn` shows no listener on 4500. For the second, connect with the token (expect upgrade 101) and then connect with an `Origin` header and no token (expect an HTTP 401 before upgrade).
  - *Status:* ☐ unverified

## Regression check

- Task 05 transport errors and bounds are resolved by listener configuration validation; run `cargo nextest run -p lotta-app-server -E 'test(auth::) | test(errors::transport)'` and confirm both suites pass : ☐ (PRESERVED / REGRESSION)

## Residue

`02-app-server-api.md` §Assumptions leaves the accepted browser Origin set and TLS termination open; this task rejects all Origin-bearing clients without credentials and records the question in `plan.md`.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
