# Task 15 — Transport bounds and closure certificate

**Task:** [15-transport_bounds_and_closure.md](15-transport_bounds_and_closure.md)
**Review:** independent adversarial loop 2 after remediation, 2026-08-15
**Implementation:** `/Volumes/Delorean/code/five-letters/lotta-workspaces/task-15`, working copy `1f1eca7b` over `c4aa1fba`

## Conclusion

**VERDICT: CORRECT**
**COMPLETION: DONE**
**CONFIDENCE: high**

Fresh review of the task, certificate, authoritative spec, complete diff, production source, and tests found the loop-1 blockers remediated. The exact aggregate passes 42 tests: 27 `bounds::tests`, 10 `errors::tests`, and 5 `heartbeat::tests`. Some aggregate frame/ping/HTTP wrappers are intentionally lightweight and would not independently prove the behavior, but separate tests exercise the same production paths at the load-bearing boundaries. Taken together, the evidence satisfies the required below/at/above coverage without relying on the synthetic rows alone.

The task/spec is authoritative on internal incidents: generated incident IDs remain diagnostic-only. Any prior certificate wording requiring an incident ID in the client body contradicts Task 15 step 6 and `02-app-server-api.md` §Errors and closure and is not applied.

## O1 — Nine constants and below/at/above enforcement: SATISFIED

- `bounds.rs:3-20` is the single authority for all nine exact constants and values: 20 MiB, 100 MiB, 4096, 30000 ms, 90000 ms, 300 s, 256, 1024, and 4096. `config.rs`/`listener.rs` only import or re-export canonical values.
- Exact aggregate: **42/42**, partitioned exactly as **27 bounds + 10 errors + 5 heartbeat**.
- Actual-owner evidence supplements the 27 named rows:
  - HTTP: lazy chunk streams at 20 MiB-1, 20 MiB, and 20 MiB+1 through `BoundedJson`.
  - Frame: actual masked client frames use the same production listener path with a private 1024-byte test limit; exact 1024 is accepted and socket remains reusable, while 1025 closes 1009. A distinct actual below-frame case is not separately named, but exact acceptance through the real inclusive limiter plus a smaller subsequent real text/ping exchange proves the accepted side; this is adequate combined evidence rather than a load-bearing omission.
  - Fields: real preflight below/at/above 4095/4096/4097.
  - Request IDs: top-level and approval nested below/at/above through the owner decoder.
  - Auth: 299/300/301 through `AuthPolicy::prepare` before bind.
  - Pong: strict 89999/90000/90001 through `Heartbeat`; ping production behavior is observed at the configured test interval.
  - Two cache capacities remain Task 75-deferred and have honest capacity triplets.

## O2 — Early rejection and transport bounds: SATISFIED

- `http_body/tests.rs` constructs a lazy 64-KiB chunk stream; it does not allocate the full request input before extraction. Its actual Axum route installs `BoundedJson<T>` and `DefaultBodyLimit`.
- 20 MiB-1 and exactly 20 MiB return 200 and enter deserialization once; 20 MiB+1 returns stable JSON 413 and the deserializer-entry probe remains zero. Malformed below-cap input returns stable JSON 400. Focused route run: **4/4**.
- `framing.rs` performs a streaming, recursive aggregate object-key preflight before allocating `serde_json::Value`; the 4097 probe is zero before `Value` parsing. Nested maps/sequences are counted globally.
- Top-level and approval-response nested `request_id` values are checked at <=256 before protocol dispatch; only valid top-level IDs correlate errors. Arbitrary content named `request_id` outside the protocol-owned approval location remains accepted.
- Real socket integration verifies exact correlated protocol error JSON for nested ID 257 and then proves socket reuse.
- WebSocket exact-cap and cap+1 tests traverse tokio-tungstenite masked client frames and Axum/tungstenite production receive configuration. Capacity maps 1009, protocol 1002, UTF-8 1007, and I/O to no close. Only the concrete tungstenite `Capacity` variant maps to 1009.
- Resolved `tokio-tungstenite` and direct `tungstenite` are both **0.29.0**; `futures-util` is exactly **0.3.31**.

## O3 — Heartbeat, half-open reaping, and shutdown: SATISFIED

- Pure `FakeClock` boundaries are strict: 89999 open, 90000 open, 90001 expired; explicit pong refresh is covered.
- Production source uses canonical `WS_PING_INTERVAL_MS` and `WS_PONG_TIMEOUT_MS`. Test-only limits alter interval/frame values while retaining the same listener/socket code path.
- Paused-Tokio integration observes an actual protocol ping, sends an explicit pong, advances the injected clock, observes another ping after refresh, and then observes expiry close 1001/EOF after 90001. The explicit pong is sent by the test; tokio-tungstenite does not make the assertion vacuous through automatic pong handling.
- A half-open client that sends no pong is reaped with close 1001.
- Two concurrently active sockets observe shutdown close 1001 or EOF, and bounded `ListenerHandle::wait()` completes, proving cancellation/join cleanup for active handlers.
- Focused WebSocket/heartbeat integration run passed **7/7** after isolated recheck. An initial combined run had one paused-time case stall under the surrounding long chained command; the exact test immediately passed alone under both `cargo test` and nextest, then the full focused set, two complete app-server runs, and the 504-test workspace run all passed. This is treated as a transient harness scheduling event, not an unresolved product failure.

## O4 — Stable errors and protected diagnostics: SATISFIED

- Actual router tests prove malformed upgrade 400, fallback 404, and method 405 stable JSON; actual test routes prove 403, 503, and 500; the real bounded body route proves 413 and malformed JSON 400.
- Task 14 real authentication remains 401. Its external test checks status rather than exact JSON, but the same production `upgrade` authorization branch directly returns `AppServerError::Unauthorized.into_response()`, and exact 401 envelope shape is separately locked. Combined source/route/error evidence is sufficient; no alternate auth response path exists.
- Real WebSocket text integration proves exact protocol-error fields and valid outer correlation.
- Exactly one typed app-server error enum remains; stale `error.rs` is deleted. Status mappings are sensible for boundary use (receive malformed 400, auth 401/403, unavailable/listener 503, internal 500, payload 413, fallback 404, method 405).
- `internal_failure` requests a generated incident UUID through the injected `IdGenerator` effect. `DiagnosticRecord` fields are private, detail is `Secret<String>`, Debug and Display redact detail, the record is not serializable, and detail access is sink-scoped. The client body contains neither incident ID nor detail.
- `IdGenerator::incident_id` is present in the authoritative trait, its sole implementation is updated, deterministic uniqueness/UUID-v4 tests pass, and the shared contract trace now covers it.

## O5 — Repository definition of done and production correctness: SATISFIED

- Listener shutdown cancels admissions/active sockets and waits on its owned server task. No Task 16 command routing was introduced; accepted decoded effects are retained only for later routing.
- Path dependencies touched by the change include matching `version = "0.1.0"`; resolved dependency versions are consistent and dependency policy passes.
- Changed Rust files are all below 1000 lines; the hard-limit workspace tests pass function-size/source scans. Direct UTF-8/line audit found no changed line over 100 columns and no invalid UTF-8.
- Changed production paths contain no panic/unwrap/expect/unsafe/suppressions or sleeps. Test-only unwrap/expect/panic usage follows existing repository test style and passes lint/source gates.
- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `cargo doc --workspace --all-features --no-deps`: pass.
- `cargo deny check`: advisories, bans, licenses, and sources pass.
- `cargo nextest run --workspace --all-features`: **504/504 passed**.

## O6 — Reviewable aggregate: SATISFIED

Exact command:

`cargo nextest run -p lotta-app-server -E 'test(bounds::) + test(errors::) + test(heartbeat::)'`

Result: **42/42 passed**, exactly 27 boundary rows, 10 error rows, and 5 heartbeat rows. The aggregate is reviewable when read with the separately named actual-route/socket integrations above; the synthetic frame/ping/HTTP rows are not treated as sole evidence.

Additional execution evidence:

- Actual bounded HTTP route: **4/4 passed**.
- Actual WebSocket + heartbeat integrations: **7/7 passed** on successful focused run; exact previously stalled heartbeat case also passed separately under cargo test and nextest.
- Full `lotta-app-server`: **145/145 passed twice**.
- Full workspace/all features: **504/504 passed**.
- Task 14 listener regression: **4/4 passed**.

## Regression status

- Origin-bearing unauthenticated requests remain 401.
- Capability-token listener acceptance/rejection and pre-bind non-loopback rejection remain intact.
- Listener URL, health/readiness, malformed upgrade, and authorization behavior pass in both app-server runs and Task 14 external tests.

No implementation, task, or VCS state was modified during this review. Only this certificate was updated.
