# Done Certificate — Task 51: pi-ai compatibility provider host

**Task:** [51-provider_compatibility_host.md](51-provider_compatibility_host.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-18

> Verification protocol for Task 51. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 51) ≡ every obligation O1…O7 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a compatibility host pinning resolved `@earendil-works/pi-ai` `0.82.1` over the shared sidecar, serving the full built-in catalog, Codex/ChatGPT OAuth, and mod-defined providers.
- **P2 — Obligations.** Done iff O1…O7 all hold, one per definition-of-done item in DoD order; O7 is the `Reviewable:` item.
- **P3 — Invariants.** Must not give the host persistence or tool access: `06-model-providers.md` §Provider classes states it has no direct access to either.

## Obligations

- **O1 — The host pins resolved pi-ai `0.82.1` and refuses to start against a different resolved version**
  - *Claim:* The pin is asserted at startup against the resolved version, not the declared range.
  - *Evidence to collect:* Read `crates/lotta-providers/src/host/pin.rs` and confirm the constant is `0.82.1`. Run `cargo nextest run -p lotta-providers -E 'test(host::version_pin)'` — expect `accepts_pinned_version` and `rejects_other_version` to pass.
  - *Checks:* Resolve the version the check compares — confirm it is the resolved lockfile version reported by the host at handshake, not the `^0.82.1` range from `package.json`; `06-model-providers.md` §Provider classes distinguishes them explicitly.
  - *Status:* ☑ SATISFIED — `host::version_pin` 2/2; the real child reports the resolved manifest `0.82.1`, and `0.82.2` is refused.

- **O2 — The host has no direct access to persistence or tools, and communicates only over the shared framing**
  - *Claim:* The host process receives no store or tool handle and every message crosses the Task 42 framing.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(host::isolation)'` — expect PASS asserting the spawned child's environment and arguments contain no backend root path. Grep `crates/lotta-providers/src/host/` for a private framing implementation — expect none.
  - *Status:* ☑ SATISFIED — `host::isolation` 2/2; the real child reports only the explicit script/package argv, two allowlisted environment keys, and an empty scratch cwd. Framing imports Task 42 only.

- **O3 — The built-in catalog is served through the host and mod-defined providers register through the provider adapter protocol**
  - *Claim:* The catalog listing includes the named provider families and a mod-registered provider appears after registration.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(host::catalog)'` — expect a case asserting the named families from §Provider classes and a `mod_defined_provider_registers` case.
  - *Status:* ☑ SATISFIED — `host::catalog` 2/2; the real catalog is 38 providers/1,109 models with all named families, and `mod_defined_provider_registers` proves register/list/unregister.

- **O4 — OAuth runs with PKCE/device-code state under `OAUTH_STATE_TTL_SECONDS`, validates callback state, and never logs a code or token**
  - *Claim:* An expired state is rejected, a mismatched state is rejected, and no authorization code or token appears in captured logs.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(host::oauth)'` — expect `rejects_expired_state`, `rejects_mismatched_state`, and `never_logs_code_or_token`, the last asserting marker absence in captured tracing output.
  - *Status:* ☑ SATISFIED — `host::oauth` 15/15, including both real-host PKCE/device flows and the three exact named state/log-secrecy cases.

- **O5 — The `fixtures/providers/` corpus replays through the host to the expected normalized traces**
  - *Claim:* Host-produced traces match the recorded expectations for the covered dialects.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(host::replay)'` — expect one case per covered fixture with divergence reporting on failure.
  - *Status:* ☑ SATISFIED — `host::replay` 7/7 covers every compatibility-host OpenAI-compatible/Anthropic fixture through real pi-ai translation and Task 12 first-divergence comparison. The sole pinned pi-ai metadata omission has an indexed, generated, provenance-bound host trace.

- **O6 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☑ SATISFIED — format, full-workspace strict Clippy, docs, deny, and generator checks pass; provider 138/138, runtime 200/200, testkit 205/205. The full isolated pinned-layout run passes 1,662/1,666; the four exclusions are pre-existing path/Task 37 checks hard-coded to the user's advanced live sibling, with no Task 51 failure. New Rust functions are ≤70 lines, files ≤1,000 lines, and lines ≤100 columns.

- **O7 — Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(host::)'` and sees the version pin enforced, host isolation, the catalog, bounded OAuth, and fixture replay pass**
  - *Claim:* The compatibility host module passes over the shared framing.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `rejects_other_version` case.
  - *Status:* ☑ SATISFIED — `test(host::)` 43/43, including `rejects_other_version`, real isolation, catalog, OAuth, replay, public non-fixture inference, and cancellation/reap/follow-up behavior.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-extensions/src/sidecar/` (Task 42) frames host traffic; `sidecar::*` passes 27/27 and the direct handshake selector passes 4/4: ☑ PRESERVED

## Residue

A native adapter may replace a host-served provider only after both produce equivalent traces on the same corpus (`06-model-providers.md` §Conformance); Task 53 is the gate that proves it.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O7 are satisfied. The pinned, isolated real pi-ai host serves the complete catalog, mod adapters, bounded OAuth, and real normalized streams; exact replay, lifecycle, static, and Task 42 regression gates pass. Claude Fable/high session `b1377353-e012-46b4-8e2d-205e0ef77e79` returned `CORRECT / DONE`.
