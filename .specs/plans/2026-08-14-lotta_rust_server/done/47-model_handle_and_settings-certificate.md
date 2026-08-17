# Done Certificate — Task 47: Model handle, settings normalization, and resolution order

**Task:** [47-model_handle_and_settings.md](47-model_handle_and_settings.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-17 — DONE

> Verification protocol for Task 47. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 47) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** stable model handles with the four-level resolution order and a model update that validates availability before persistence.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not narrow `model_settings`: `06-model-providers.md` §Model handle and settings describes it as open, so an unrecognized provider option must survive.

## Obligations

- **O1 — Resolution follows the four-level order, with each level overriding the ones below it**
  - *Claim:* A request override beats a conversation model, which beats an agent model, which beats the configured local default.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(model::resolution_order)'` — expect four cases asserting each level wins over the next. Trace: agent `a`, conversation `c`, request `r` → resolved `r`.
  - *Status:* ☒ SATISFIED — exact selector passes 5/5 and proves request, conversation, agent, and local-default handle precedence with low-to-high settings overlays.

- **O2 — Settings normalization preserves every baseline-supported setting, including provider-specific options**
  - *Claim:* Round-tripping a settings map with context-window limit, reasoning effort, endpoint, provider type, and an unrecognized provider-specific key loses nothing.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(model::settings_normalization)'` — expect PASS asserting the unrecognized key survives, since `model_settings` is open.
  - *Status:* ☒ SATISFIED — exact selector passes 5/5; baseline keys normalize deterministically while unknown nested/null provider settings survive.

- **O3 — A model update validates availability before persistence and leaves the prior model untouched on failure**
  - *Claim:* Setting an unavailable model returns an error and the persisted model is unchanged.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(model::update_validates_first)'` — expect PASS asserting zero writes reached the store fake.
  - *Checks:* Resolve the availability check's position relative to the store write — confirm the check precedes the write; `06-model-providers.md` §Model handle and settings requires validation before persistence.
  - *Status:* ☒ SATISFIED — exact selector passes 4/4 with availability/check/revision failures producing zero writes and unchanged prior state.

- **O4 — `list_models` reports connection readiness and never exposes credentials, and `MODELS_PER_PROVIDER_MAX` bounds the catalog**
  - *Claim:* The listing carries a readiness flag per model, contains no secret, and rejects above 10,000 models per provider.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(model::listing)'` — expect `reports_readiness`, `contains_no_credentials`, and below/at/above cases for `MODELS_PER_PROVIDER_MAX`.
  - *Status:* ☒ SATISFIED — exact selector passes 7/7 for readiness, credential containment, duplicates/provider mismatch, and actual 9,999/10,000/10,001 limits.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☒ SATISFIED — workspace 1,525/1,525 plus fmt, strict all-target/all-feature Clippy, private docs, deny, units-last constants, and source-shape checks pass.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(model::)'` and sees the four-level resolution, open settings preservation, validate-before-persist, and credential-free listing pass**
  - *Claim:* The model module passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and four `resolution_order` cases.
  - *Status:* ☒ SATISFIED — broad `model::` selector passes 25/25 with all four named evidence modules substantive and nonzero.

## Regression check

- Task 03 `Agent` and `Conversation` model fields and Task 07 request shapes are consumed by handle resolution; run `cargo nextest run -p lotta-domain -E 'test(entities::model_fields)'` and `cargo nextest run -p lotta-runtime -E 'test(model::resolution_order)'` and confirm both pass : ☒ PRESERVED — exact selectors pass 2/2 and 5/5; provider runtime integration consumes actual entity/request fields.

## Residue

The static pi-ai built-in catalog is distinct from native endpoint discovery; `06-model-providers.md` §Conformance states `models.json` is not the local-provider catalog. Discovery is Task 50.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: Stable nested model handles, open baseline settings, four-level handle and settings resolution, validate-before-persist updates, and credential-free bounded readiness listing are implemented and independently certified by Claude Fable.
