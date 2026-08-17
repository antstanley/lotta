# Task 47 — Model handle, settings normalization, and resolution order

**Plan:** [plan.md](../plan.md) · **Certificate:** [47-model_handle_and_settings-certificate.md](47-model_handle_and_settings-certificate.md)

**Implements:** [06-model-providers.md §Model handle and settings](../../../06-model-providers.md#model-handle-and-settings)
**Depends on:** 03, 07
**Produces:** stable model handles with the four-level resolution order and a model update that validates availability before persistence
**Pointers:** `crates/lotta-providers/src/model/handle.rs`, `model/settings.rs`, `model/resolve.rs`; reference: `../letta-code/src/backend/dev/pi-model-factory.ts`, `../letta-code/src/backend/dev/pi-models-runtime.ts`, `../letta-code/src/providers/local-pi-provider-catalog.test.ts`

## Steps

- [x] Define the model handle as a stable provider/model identifier stored on agent records with open `model_settings`
- [x] Normalize the baseline-supported settings: context-window limits, reasoning effort/tier, endpoint/base URL, provider type, and provider-specific options
- [x] Implement the four-level resolution order: request-scoped temporary override, conversation model and settings, agent model and settings, configured local default
- [x] Validate availability before persisting a model update and leave the prior model untouched on failure
- [x] Report connection readiness in `list_models` without exposing credentials
- [x] Enforce `MODELS_PER_PROVIDER_MAX`

## Definition of done

- [x] Resolution follows the four-level order, with each level overriding the ones below it
- [x] Settings normalization preserves every baseline-supported setting, including provider-specific options
- [x] A model update validates availability before persistence and leaves the prior model untouched on failure
- [x] `list_models` reports connection readiness and never exposes credentials, and `MODELS_PER_PROVIDER_MAX` bounds the catalog
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(model::)'` and sees the four-level resolution, open settings preservation, validate-before-persist, and credential-free listing pass
