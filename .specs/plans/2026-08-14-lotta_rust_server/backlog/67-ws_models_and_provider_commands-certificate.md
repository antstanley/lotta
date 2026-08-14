# Done Certificate — Task 67: WebSocket models and provider command group

**Task:** [67-ws_models_and_provider_commands.md](67-ws_models_and_provider_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 67. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 67) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** list models, list/connect/disconnect providers, usage read, and update model/toolset commands over the Task 47 and 52 surfaces.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not expose credentials: `06-model-providers.md` §Model handle and settings states `list_models` does not expose credentials.

## Obligations

- **O1 — All six commands decode, route, and respond, with fixture round-trip coverage**
  - *Claim:* Each command in the Models/providers row has a passing case.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::models::commands)'` — expect six cases derived from the fixture group listing.
  - *Status:* ☐ unverified

- **O2 — No response in this group contains a credential, and `list_models` reports readiness**
  - *Claim:* A captured response set for all six commands contains no secret marker and the model listing carries readiness.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::models::redaction)'` — expect PASS asserting a planted secret marker is absent from every response body.
  - *Status:* ☐ unverified

- **O3 — Disconnect refuses during an active turn unless forced, and the forced path cancels affected turns first**
  - *Claim:* The wire command inherits the Task 52 behavior exactly.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::models::disconnect)'` — expect `refuses_while_active` and `force_cancels_then_disconnects`.
  - *Checks:* Resolve the disconnect implementation — confirm it calls the Task 52 unit rather than removing the connection directly, so the active-turn guard cannot be bypassed over the wire.
  - *Status:* ☐ unverified

- **O4 — Update model validates availability before persistence and update toolset resolves through the Task 32 registry**
  - *Claim:* An unavailable model leaves the stored model unchanged, and a toolset update resolves one of the six IDs.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::models::updates)'` — expect `model_update_validates_first` and `toolset_update_resolves_valid_id`, the latter rejecting an unknown toolset name.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::models::)'` and sees six commands, credential-free responses, guarded disconnect, and validated updates pass**
  - *Claim:* The models and providers group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and six command cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-providers/src/connections/` (Task 52) is driven over the wire; confirm `connections::disconnect` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

OAuth callback handling stays in the provider host (Task 51); this group initiates and reports.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
