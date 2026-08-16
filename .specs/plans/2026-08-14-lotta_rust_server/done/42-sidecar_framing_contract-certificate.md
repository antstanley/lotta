# Done Certificate — Task 42: Shared bounded sidecar framing contract

**Task:** [42-sidecar_framing_contract.md](42-sidecar_framing_contract.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-16 — DONE

> Verification protocol for Task 42. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 42) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** one versioned length-prefixed JSON framing used by the provider host, the mod host, and the subagent host, validated at the child-to-host boundary.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must be the only framing implementation: three independent sidecar protocols would be mutually incompatible by construction and would each need their own boundary validation.

## Obligations

- **O1 — Framing is length-prefixed JSON with a declared maximum frame length enforced before the payload is read**
  - *Claim:* A frame declaring a length above the bound is rejected without allocating the payload.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(sidecar::framing)'` — expect `round_trips_frame`, `rejects_oversized_declared_length`, and `does_not_allocate_before_check` to pass, the last asserting peak allocation.
  - *Checks:* Resolve the read call sequence — confirm the length check precedes the payload read. `architecture-principles.md` §Limits requires boundary validation before allocation proportional to untrusted input.
  - *Status:* ☒ SATISFIED — exact framing selector proves round-trip, oversized declared-length rejection before payload read/allocation, zero peak on rejection, exact at-limit peak, and the authoritative four-byte big-endian golden frame.

- **O2 — All five §Where to validate child-to-host properties are checked on every inbound frame: frame length, protocol version, owner identity, capability, and timeout**
  - *Claim:* Each property has a rejection test and a passing test.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(sidecar::validation)'` — expect five rejection cases and five acceptance cases.
  - *Status:* ☒ SATISFIED — 12/12 validation cases accept and reject frame length, version, exact owner, capability, and timeout in order on every frame; failures are terminal.

- **O3 — The version handshake rejects an unsupported protocol version before any payload is processed**
  - *Claim:* A child announcing an unsupported version is terminated and no payload frame is dispatched.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(sidecar::handshake)'` — expect `rejects_unsupported_version` asserting zero dispatched payloads.
  - *Status:* ☒ SATISFIED — unsupported Hello followed by payload terminates the production host session with zero payload dispatch and exact child stop/join.

- **O4 — The supervisor bounds restarts at `SIDECAR_RESTARTS_PER_HOUR_MAX` and a crash leaves host state uncorrupted**
  - *Claim:* The eleventh restart within an hour is refused, and after a crash the host's registrations are consistent.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(sidecar::supervisor)'` — expect `bounds_restarts_per_hour` (naming the `07-channels-and-operations.md` §Operational bounds constant) and `crash_leaves_host_consistent`.
  - *Status:* ☒ SATISFIED — the initial launch is free, exactly ten rolling-hour restarts are admitted, the eleventh is refused before spawn, and every failure preserves prior host state while stopping/joining the exact child.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☒ SATISFIED — fmt, strict workspace Clippy, workspace nextest 1,322/1,322, private Rustdoc, and `cargo deny check` pass; touched Rust meets file/function/column limits.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(sidecar::)'` and sees framing bounds, the five validation properties, version rejection, and bounded restarts pass**
  - *Claim:* The sidecar module passes as a single shared contract.
  - *Evidence to collect:* Run the filter and confirm zero failures, then grep Tasks 45, 46, and 51 modules for a second framing implementation — expect none.
  - *Status:* ☒ SATISFIED — clean reviewer ran all 27 shared sidecar cases and found no second production framing implementation or Task 42 remainder.

## Regression check

- Task 09 fake clock/process harness and Task 05 extension bounds drive framing tests; run `cargo nextest run -p lotta-extensions -E 'test(sidecar::framing)'` plus `cargo nextest run -p lotta-testkit -E 'test(contract::)'` and confirm both pass : ☒ PRESERVED — framing selector and 10/10 contracts pass.

## Residue

`06-model-providers.md` §Provider classes pins the provider host to `@earendil-works/pi-ai` `0.82.1` over length-prefixed JSON pipes; this task provides the transport, Task 51 provides the provider payload contract.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: One authoritative four-byte big-endian bounded JSON envelope now drives provider, mod, and caller-bounded sidecars with ordered per-frame validation, mandatory version handshake, owned dispatch/process lifecycle, and transactional rolling restart supervision; all repository gates pass.
