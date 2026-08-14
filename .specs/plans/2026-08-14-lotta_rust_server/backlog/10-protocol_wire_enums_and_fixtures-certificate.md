# Done Certificate — Task 10: Protocol wire enums and the discriminant fixture gate

**Task:** [10-protocol_wire_enums_and_fixtures.md](10-protocol_wire_enums_and_fixtures.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 10. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 10) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** one tagged command enum and one tagged message enum whose variant lists are proven equal to a checked-in fixture extracted from the pinned TypeScript unions.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not hand-author the fixture: `02-app-server-api.md` §Assumptions *Protocol source* requires a pinned discriminant fixture because Rust exhaustiveness alone cannot detect upstream TypeScript additions.

## Obligations

- **O1 — Every baseline command and message discriminant has exactly one Rust variant, proven by a bidirectional manifest test against `fixtures/protocol/discriminants.json`**
  - *Claim:* The Rust variant set and the fixture set are equal; a variant with no fixture entry and a fixture entry with no variant both fail.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-protocol -E 'test(manifest::)'` — expect `every_fixture_has_a_variant` and `every_variant_has_a_fixture` to pass. Read `fixtures/protocol/discriminants.json` and confirm its header records the source commit `300f923f16cc8eee50656d7da732902c1dea2b65`.
  - *Checks:* Resolve the extractor's source of truth for each union — confirm `WsProtocolCommand` is read from `../letta-code/src/types/protocol_v2.ts` and `WsProtocolMessage` from `app-server-protocol.ts`. `NAME SHADOWING` risk: `../letta-code/src/types/protocol.ts` also declares protocol types and reading it instead would produce a plausible but wrong manifest.
  - *Status:* ☐ unverified

- **O2 — Re-running the extractor against the pinned checkout produces no diff, and CI enforces it**
  - *Claim:* The checked-in fixture is exactly what the extractor emits today.
  - *Evidence to collect:* Run `node tools/extract-protocol-fixture.mjs --check` (or the equivalent task command) and confirm exit code 0 with no diff output. Read `.github/workflows/ci.yml` and confirm a step invokes the same check.
  - *Status:* ☐ unverified

- **O3 — Unknown top-level `type` values are dropped with no response and no state mutation; extra fields are tolerated where the baseline tolerates them; required fields and enum values stay strict**
  - *Claim:* Decoding `{"type":"not_a_command"}` yields a drop with no emitted message; decoding a known command with an extra field succeeds; decoding one with a bad enum value fails.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-protocol -E 'test(decode::)'` — expect `unknown_type_is_dropped_silently`, `extra_fields_tolerated`, and `bad_enum_value_rejected` to pass. Read the unknown-type case and confirm it asserts zero outbound messages and zero state mutations, not merely an `Err`.
  - *Status:* ☐ unverified

- **O4 — A malformed known `input` frame produces the baseline non-terminal loop-error notice rather than a terminal failure**
  - *Claim:* A structurally invalid `input` command emits the loop-error notice and leaves the runtime admitting further input.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-protocol -E 'test(decode::malformed_input_is_non_terminal)'` — expect PASS. Compare the emitted notice shape against `../letta-code/src/websocket/listener/recoverable-notices.ts`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-protocol` and then the extractor in check mode, and sees the manifest equality, decode-policy cases, and a clean regeneration diff**
  - *Claim:* The protocol crate's tests pass and the fixture regenerates byte-identically.
  - *Evidence to collect:* Run both commands and confirm zero test failures and zero diff lines from the extractor check.
  - *Status:* ☐ unverified

## Regression check

- Task 04 runtime projections are serialized inside protocol fixtures; run `cargo nextest run -p lotta-protocol -E 'test(runtime_snapshot::) | test(manifest::)'` and confirm both suites pass : ☐ (PRESERVED / REGRESSION)

## Residue

Per-command payload schemas are owned by the command-group tasks (62–73); this task fixes only the discriminant set, the envelope tagging, and the decode policy.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
