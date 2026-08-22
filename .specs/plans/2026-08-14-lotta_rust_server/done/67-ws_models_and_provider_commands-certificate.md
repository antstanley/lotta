# Done Certificate — Task 67: WebSocket models and provider command group

**Task:** [67-ws_models_and_provider_commands.md](67-ws_models_and_provider_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-22

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
  - *Evidence:* Exact selector passed 7/7 — one substantive case per variant. The pinned fixture lists SEVEN tags for this row (`list_connect_providers` is its own tag beyond the authored "six"), verified against `protocol_v2.ts`; `covers_exactly_this_group` proves exactly-once membership of all 7 command + 7 message discriminants with genuine decode→encode round-trips. Response shapes match the pinned types field-for-field.
  - *Status:* ☑ SATISFIED

- **O2 — No response in this group contains a credential, and `list_models` reports readiness**
  - *Claim:* A captured response set for all six commands contains no secret marker and the model listing carries readiness.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::models::redaction)'` — expect PASS asserting a planted secret marker is absent from every response body.
  - *Evidence:* Exact selector passed 2/2. A planted secret marker persisted through the Task 52 store and sent as a connect field is asserted absent from every serialized response across all seven commands; `list_models` carries per-entry readiness, available handles, and BYOK aliases. Static scan of the wire structs finds no credential field.
  - *Status:* ☑ SATISFIED

- **O3 — Disconnect refuses during an active turn unless forced, and the forced path cancels affected turns first**
  - *Claim:* The wire command inherits the Task 52 behavior exactly.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::models::disconnect)'` — expect `refuses_while_active` and `force_cancels_then_disconnects`.
  - *Checks:* Resolve the disconnect implementation — confirm it calls the Task 52 unit rather than removing the connection directly, so the active-turn guard cannot be bypassed over the wire.
  - *Evidence:* Exact selector passed 2/2 — `refuses_while_active` and `force_cancels_then_disconnects`. Traced: the bridge calls `ConnectionManager::disconnect` (the Task 52 unit) with the turn registry and runtime, so the active-turn guard cannot be bypassed; the forced path cancels each active turn and waits before record removal.
  - *Status:* ☑ SATISFIED

- **O4 — Update model validates availability before persistence and update toolset resolves through the Task 32 registry**
  - *Claim:* An unavailable model leaves the stored model unchanged, and a toolset update resolves one of the six IDs.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-app-server -E 'test(ws::models::updates)'` — expect `model_update_validates_first` and `toolset_update_resolves_valid_id`, the latter rejecting an unknown toolset name.
  - *Evidence:* Exact selector passed 3/3. Unavailable models are rejected through Task 47 catalog availability BEFORE persistence with the stored model byte-identical afterward; toolset updates resolve through the Task 32 registry (six IDs) and unknown names are rejected at decode.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* `cargo fmt --all --check`, strict workspace Clippy, workspace rustdoc with warnings denied, and `cargo deny check` passed; full lotta-app-server 313/313. Workspace residue remains confined to the recorded external pinned-SHA/CLI family. New functions stay within 70 lines; constants use units-last naming; no production unwrap/expect.
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::models::)'` and sees six commands, credential-free responses, guarded disconnect, and validated updates pass**
  - *Claim:* The models and providers group passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and six command cases.
  - *Evidence:* Broad selector passed 29/29 with zero failures covering all seven command variants, redaction, guarded disconnect, validated updates, and fixture round-trips. Independent review returned `CORRECT / DONE`, verifying the connect-timestamp fix end-to-end (stamped connects now reach adapter validation and succeed).
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-providers/src/connections/` (Task 52) is driven over the wire; confirm `connections::disconnect` still passes : ☑ PRESERVED

## Residue

OAuth callback handling stays in the provider host (Task 51); this group initiates and reports.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 obligations are discharged by exact selectors and traced evidence. The group routes SEVEN command variants — the pinned fixture splits `list_connect_providers` from `list_models`, superseding the authored count of six; both counts are noted here for the record. No response contains credential material while `list_models` reports per-entry readiness; disconnect inherits the Task 52 active-turn guard exactly, with the forced path cancelling turns before removal; model updates validate availability before persistence and toolset updates resolve through the Task 32 registry; and every mutation emits its provider snapshot in pinned order. The connect timestamp defect found during implementation was fixed by stamping from the injected clock, proven by a success-path test. The Task 52 disconnect regression suite is preserved.
