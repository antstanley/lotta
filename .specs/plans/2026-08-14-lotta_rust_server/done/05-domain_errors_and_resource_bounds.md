# Task 05 — Domain error taxonomy and resource bounds

**Plan:** [plan.md](../plan.md) · **Certificate:** [05-domain_errors_and_resource_bounds-certificate.md](05-domain_errors_and_resource_bounds-certificate.md)

**Implements:** [01-domain-model.md §Resource bounds](../../../01-domain-model.md#resource-bounds) · [architecture-principles.md §Cross-cutting conventions](../../../architecture-principles.md#cross-cutting-conventions) · [architecture-principles.md §Errors](../../../architecture-principles.md#errors) · [architecture-principles.md §Limits](../../../architecture-principles.md#limits) · [architecture-principles.md §Secrets](../../../architecture-principles.md#secrets) · [development-guidelines.md §Limits and bounds](../../../development-guidelines.md#limits-and-bounds)
**Depends on:** 02, 03, 04
**Produces:** one typed error enum per crate boundary plus every `01-domain-model.md` resource bound as a units-last named constant that is observable when reached
**Pointers:** `crates/lotta-domain/src/errors.rs`, `crates/lotta-domain/src/bounds.rs`, `crates/lotta-domain/src/secret.rs`; reference: `../letta-code/src/websocket/listener/listener-constants.ts`, `../letta-code/src/websocket/listener/constants.ts`

## Steps

- [x] Define the domain error enum with `thiserror` and a stable string code per variant that survives refactors
- [x] Define the nine `01-domain-model.md` §Resource bounds constants with their exact names and defaults (`CONNECTIONS_MAX` 1024, `RUNTIMES_MAX` 4096, `RUNTIME_SUBSCRIPTIONS_PER_CONNECTION_MAX` 256, `QUEUE_ITEMS_SOFT_MAX` 100, `QUEUE_ITEMS_HARD_MAX` 300, `PENDING_APPROVALS_PER_RUNTIME_MAX` 128, `EXTERNAL_TOOLS_PER_RUNTIME_MAX` 256, `SCHEDULE_RUN_LOG_KEEP_LINES` 2000, `SCHEDULE_RUN_LOG_BYTES_MAX` 2000000)
- [x] Record each bound's at-limit behavior as data on the constant so a structured event and counter can name it, per `development-guidelines.md` §Limits and bounds
- [x] Define `Secret<T>` with redacting `Debug` and `Display` and no accessor that can reach a formatter
- [x] Add a lint test asserting no production bound declaration or limit check embeds a numeric bound literal outside `bounds.rs` (ordinary IDs, fixture values, and non-bound arithmetic are not false positives)
- [x] Add units-last naming assertions over the exported constant list

## Definition of done

- [x] All nine `01-domain-model.md` §Resource bounds constants exist with the spec's exact names and default values
- [x] Every bound name puts units last and no numeric bound literal appears outside `bounds.rs`
- [x] Reaching a bound produces a structured event and counter identifying the bound by name, and no bound silently drops input
- [x] `Secret<T>` redacts through `Debug` and `Display`, and the error enum exposes a stable code per variant
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-domain -E 'test(bounds::) + test(errors::) + test(secret::)'` and sees the spec-table match, units-last naming, at-limit observability, and secret redaction pass
