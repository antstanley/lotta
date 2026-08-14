# Task 09 — Testkit fakes and the shared port-contract suite

**Plan:** [plan.md](../plan.md) · **Certificate:** [09-testkit_and_port_contract_suite-certificate.md](09-testkit_and_port_contract_suite-certificate.md)

**Implements:** [architecture-principles.md §Testing architecture](../../../architecture-principles.md#testing-architecture) · [development-guidelines.md §Testing](../../../development-guidelines.md#testing)
**Depends on:** 07, 08
**Produces:** deterministic clocks and IDs, in-memory port fakes, and one shared contract suite that every concrete adapter and its fake must pass
**Pointers:** `crates/lotta-testkit/src/clock.rs`, `src/ids.rs`, `src/fakes/`, `src/contract/mod.rs`, `src/roots.rs`; reference: `../letta-code/src/backend/dev/fake-headless-backend.ts`, `../letta-code/src/backend/dev/headless-backend.ts`

## Steps

- [x] Implement `FakeClock` with explicit advance and no wall-clock read, and deterministic ID generators for each ID newtype
- [x] Implement in-memory fakes for every port defined in Tasks 06–08
- [x] Write the shared port-contract suite as a reusable function generic over an implementation, per the `architecture-principles.md` §Testing architecture *Port contract* tier
- [x] Run the suite against every fake so the fakes are proven equivalent before any adapter exists
- [x] Implement temporary-root and golden-fixture helpers that read from `fixtures/` without network access
- [x] Add a test asserting no wall-clock sleep or network call is reachable from the testkit

## Definition of done

- [x] One shared contract suite exists per port and is invoked generically, so a concrete adapter reuses it without copying assertions
- [x] Every port from Tasks 06–08 has an in-memory fake that passes the shared suite
- [x] `FakeClock` and the deterministic ID generators make tests reproducible: no wall-clock read, no sleep, no network
- [x] The golden-fixture loader reads `fixtures/` deterministically and fails loudly on a missing or malformed fixture
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-testkit` and sees the shared contract suite pass against every in-memory fake with no wall-clock or network dependency
