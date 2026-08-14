# Done Certificate — Task 89: Security suite

**Task:** [89-security_suite.md](89-security_suite.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 89. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 89) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** proof of non-loopback authentication, Origin rejection, path confinement, secret redaction, and sandbox policy against the assembled binary.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not exempt loopback from Origin checking: `02-app-server-api.md` §Responsibilities item 2 requires authenticating Origin-bearing clients, and the server exposes shell and filesystem capabilities on its host.

## Obligations

- **O1 — Non-loopback binding without authentication fails before listening, and both auth modes accept valid and reject invalid credentials**
  - *Claim:* The failure occurs before the socket opens, and each mode has an accept and a reject case.
  - *Evidence to collect:* Run `cargo nextest run --test security -E 'test(auth::)'` — expect `non_loopback_without_auth_fails_before_bind` plus accept/reject cases for capability-token and signed-bearer modes.
  - *Status:* ☐ unverified

- **O2 — An unauthenticated Origin-bearing client is rejected even on loopback, and every signed-bearer rejection class is covered**
  - *Claim:* The Origin case and the five signed-bearer rejection classes all fail closed.
  - *Evidence to collect:* Run `cargo nextest run --test security -E 'test(origin::) + test(bearer::)'` — expect the Origin case and five rejection cases (missing `exp`, wrong issuer, wrong audience, non-HS256, excessive skew).
  - *Checks:* Resolve the Origin check's position — confirm it runs before the upgrade completes, so a rejected client never reaches the protocol layer.
  - *Status:* ☐ unverified

- **O3 — Path confinement holds across file tools, memory tools, and the files command group for all four escape classes**
  - *Claim:* `..`, symlinks, alternate separators, and absolute out-of-root paths are rejected on all three surfaces.
  - *Evidence to collect:* Run `cargo nextest run --test security -E 'test(confinement::)'` — expect twelve cases (three surfaces × four escape classes).
  - *Status:* ☐ unverified

- **O4 — Planted secret markers are absent from logs, errors, App Server snapshots, and channel config responses**
  - *Claim:* A run that plants a marker in the provider credential, a hook environment, and a channel config value shows the marker in none of the four outputs.
  - *Evidence to collect:* Run `cargo nextest run --test security -E 'test(redaction::)'` — expect four absence assertions over captured tracing output, a rendered error, a snapshot body, and a channel config response.
  - *Status:* ☐ unverified

- **O5 — The sandbox blocks out-of-root reads and an unsupported platform errors rather than running unsandboxed**
  - *Claim:* A shell child cannot read outside the root, and the unsupported path errors before execution.
  - *Evidence to collect:* Run `cargo nextest run --test security -E 'test(sandbox::)'` — expect the blocking case on a supported platform and `unsupported_errors_before_exec` with the executor never invoked.
  - *Status:* ☐ unverified

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O7 — Reviewable: a reviewer runs `cargo nextest run --test security` against the assembled binary and sees auth, Origin, signed-bearer, confinement, redaction, and sandbox cases pass**
  - *Claim:* The security suite is green with all five classes covered.
  - *Evidence to collect:* Run the command and confirm zero failures and at least twenty-five cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/auth/` (Task 14) is exercised here; confirm `auth::origin_bearing_loopback_rejected` still passes : ☐ (PRESERVED / REGRESSION)
- `crates/lotta-tools/src/permissions/` (Task 34) and `sandbox/` (Task 35); confirm `permissions::analyzer` and `sandbox::workspace` still pass : ☐ (PRESERVED / REGRESSION)

## Residue

This suite satisfies `00-overview.md` §Implementation acceptance criterion 6. TLS termination remains an open question and is not exercised here.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
