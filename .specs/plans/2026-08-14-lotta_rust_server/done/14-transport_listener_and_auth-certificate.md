# Done Certificate — Task 14: Transport listener, capability-token and signed-bearer authentication# Task 14 verification certificate — review loop 1

**Task:** [14-transport_listener_and_auth.md](14-transport_listener_and_auth.md) · **Plan:** [plan.md](../plan.md)
**Reviewed:** 2026-08-14 against working copy `lupxmonx` / parent `pkpvwwqr`

## Scope and method

I read the full task, certificate, plan baseline, `02-app-server-api.md`, development rules,
the exact Jujutsu diff (26 files, 2,899 insertions/12 deletions), pinned
`app-server-auth.ts` and `app-server.ts`, every changed production file, tests, manifests,
lockfile change, and `deny.toml`. I did not edit implementation, task text, or VCS state.

All commands below used
`--manifest-path /Volumes/Delorean/code/five-letters/lotta-workspaces/task-14/Cargo.toml`
where needed.

## Independent executions

- Exact selectors: `auth::modes` **16/16 twice**; `auth::signed_bearer` **17/17 twice**;
  `auth::skew_ceiling` **2/2 twice**; `auth::non_loopback_requires_auth` **1/1**;
  `auth::origin_bearing_loopback_rejected` **1/1**; `auth::secrets_never_logged` **1/1**;
  `listener::url_resolution` **15/15**.
- External binary suite: `cargo nextest run --test task14_listener` **4/4 twice**. It used a
  real child and TCP: capability-file upgrade `/ws` and `/` = **101**, missing Origin-bearing
  auth = **401**, wrong token = **401**, unrelated path = **404**, health = **200**; digest
  mode = **101**; invalid/non-loopback startup exit = **2** and immediate rebind succeeds;
  stdout exactly advertises actual base/WS/OpenAI URLs and stdout/stderr exclude the token.
- Full app-server: **65/65 twice**.
- Workspace all features: **423/423**, including prior Task 09–13 suites. No nextest LEAK
  marker occurred. The formerly suspect capability secret test passed in exact, app-server,
  and high-parallel workspace runs; its files are explicitly removed and no residual child
  process was found. The earlier isolated marker is not reproduced as a resource leak.
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: clean.
- `cargo deny check all`: advisories, bans, licenses, sources all clean.
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps`: clean.
- `cargo audit --file Cargo.lock`: unavailable (`cargo-audit` is not installed).
- Brace-aware source scan: no changed production function over 70 lines; no line over 100
  columns. Changed production scan found no `panic!`, `unwrap`, `expect`, unsafe block,
  `#[allow]`, or `#[expect]`; tests contain ordinary assertion helpers but no suppression.

## Obligations

### O1 — exact CLI/auth modes and capability verification — SATISFIED

`config.rs:56-170` accepts the specified flags, rejects unknown, duplicate and missing values,
and requires `--listen`; `AuthPolicy::prepare` rejects auth flags without mode, missing sources,
both capability sources, and all cross-mode combinations. The only mode strings are exactly the
pinned TypeScript union (`app-server-auth.ts:11`). Bare `--listen` and omitted URL port resolve to
loopback port 0; only `ws` is accepted; URL auth/query/fragment are rejected; `/` maps to `/ws`,
while a non-root path is exact. Extra manual duplicate prefix probes all exited 2 safely.

Capability file input is absolute, read once with a metadata check plus bounded
`take(65_537)`, edge-trimmed once, hashed to SHA-256, then discarded. Digest input is exactly 64
hex characters. Candidate tokens are SHA-256 hashed before `subtle::ConstantTimeEq` compares two
fixed `[u8;32]` values (`capability_token.rs:39-46`); there is no `Eq` secret comparison and no
HMAC name-shadowing path.

### O2 — signed bearer verification — SATISFIED

`signed_bearer.rs` enforces token 16 KiB, exactly three non-empty components, canonical unpadded
base64url, decoded header/claims bounds of 8 KiB, exact `alg == "HS256"`, and HMAC
`verify_slice`. `exp` is mandatory; `exp`/`nbf` are JS-safe integral `i64`; issuer is typed;
audience is always validated even without configured audience and is a string or at most 64
all-string entries. Checked add/subtract handles skew overflow. Boundaries are correct:
`now == exp + skew` and `now == nbf - skew` pass; beyond fails. Default skew is 30 and maximum
300; 301 fails during prepare before bind. Verification obtains time only from injected `Clock`.
The root `SystemClock` is a non-panicking Chrono adapter; Chrono `clock` is needed there.

### O3 — pre-bind validation and Origin/non-loopback security — SATISFIED

`ServerArgs::prepare` performs URL, auth mode, path, digest and secret validation before any
`TcpListener::bind`; `start_listener` independently rechecks non-loopback/auth, resisting a forged
public `PreparedServer` (its fields are crate-private). IPv4 127/8, IPv6 `::1`, and case-insensitive
`localhost` are loopback; all other IP/host forms require auth. Real child evidence proves
`0.0.0.0` unauthenticated failure precedes bind. Auth runs before upgrade; loopback Origin without
policy is 401, while valid configured auth with Origin upgrades 101. No 101 precedes these checks.

### O4 — listener routes, URLs, bounds and lifecycle — SATISFIED

The Axum router registers configured WS path and `/`, `/healthz`, `/readyz`, and a 404 fallback.
Actual OS-selected ports are used in base, WS and optional HTTP `/v1` output, including bracketed
IPv6. Both frame and message maxima are explicitly 100 MiB. `ListenerHandle` owns the shutdown
sender and join handle. `wait(self)` deliberately sends graceful shutdown before joining; the root
waits for Ctrl-C, calls shutdown, then joins. Tests call `wait` as an owned stop operation. No
production detached process exists; the sole Tokio serve task has cancellation and a join path.

### O5 — secret custody and leakage — SATISFIED

Both files are read once at startup, bounded to 64 KiB before/while reading, edge-trimmed once,
and converted into `Secret`-wrapped digest/key material. `Secret::expose_secret` uses an HRTB;
`Debug` and `Display` are unconditional `[REDACTED]`. Errors and tracing use fixed messages, mode
names and paths, not contents. `secrets_never_logged` deletes and replaces both capability and
shared-secret files, proves the in-memory originals still authenticate while replacements fail,
and scans original/replacement literals across tracing, policy Debug, and error Debug/Display.
External tests separately scan real child stdout and stderr. One residual caveat is that configured
secret **paths** may appear in read errors by design; contents do not.

### O6 — repository definition of done and dependencies — SATISFIED

All required tests and gates above pass. New bounds are named constants with units last. Source
hard-limit and mutation tests also pass. The diff contains no Task 15+ closure/runtime command
implementation. New direct path dependencies specify `version = "0.1.0"`; cryptographic and URL
crate versions are explicit workspace minimums. Axum default features are broader than a hand-cut
minimal set, but are the normal Axum server surface and no accidental duplicate failed deny/tree
gates. Chrono clock is justified by the root system adapter.

`deny.toml` remains narrow: only Apache-2.0, MIT, BSD-3-Clause and Unicode-3.0 are allowed; the
only duplicate exception is specifically `syn@2.0.119`, with the accurate reason that source audit
uses Syn 2 while Tokio macros resolve Syn 3. `cargo deny check all` is clean.

### O7 — externally reviewable behavior — SATISFIED

The real-child suite twice demonstrated all required outcomes from outside the process: invalid
non-loopback unauthenticated startup exits 2 before bind/rebind; capability-file authenticated
connections return 101; unauthenticated Origin-bearing connection returns 401. It additionally
proved wrong-token 401, route 404, health 200, digest authentication 101, exact actual URL lines,
and secret scrubbing. Child Drop always kills/waits and removes token files; explicit successful
paths also kill/wait before pipe capture.

## Regression check

**PRESERVED.** The complete app-server suite passed 65/65 twice and workspace passed 423/423,
covering Task 05 bounds/errors plus all Task 09–13 tests. No leaked listener, child, or secret file
was observed in this loop.

## Residuals

- `cargo-audit` could not be run because the executable is unavailable; `cargo deny check all`
  performed the configured advisory check successfully.
- TLS termination and accepted browser Origin sets remain explicitly deferred spec questions.
- Process signal support in this task is Ctrl-C only; full SIGTERM/eight-step shutdown belongs to
  Task 85, not Task 14.

## Conclusion

**VERDICT: CORRECT**
**COMPLETION: DONE**
**CONFIDENCE: high**

All O1–O7 are load-bearing and SATISFIED, with regression PRESERVED. The implementation matches
the Task 14 contract and pinned baseline behavior on the reviewed surface.
