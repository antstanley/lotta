# Task 09 — Testkit fakes and the shared port-contract suite

**Plan:** [plan.md](../plan.md) · **Certificate:** [09-testkit_and_port_contract_suite-certificate.md](09-testkit_and_port_contract_suite-certificate.md)

**Implements:** [architecture-principles.md §Testing architecture](../../../architecture-principles.md#testing-architecture) · [development-guidelines.md §Testing](../../../development-guidelines.md#testing)
**Depends on:** 07, 08
**Produces:** deterministic clocks and IDs, in-memory port fakes, and one shared contract suite that every concrete adapter and its fake must pass
**Pointers:** `crates/lotta-testkit/src/clock.rs`, `src/ids.rs`, `src/fakes/`, `src/contract/mod.rs`, `src/roots.rs`; reference: `../letta-code/src/backend/dev/fake-headless-backend.ts`, `../letta-code/src/backend/dev/headless-backend.ts`

## Steps

- [ ] Implement `FakeClock` with explicit advance and no wall-clock read, and deterministic ID generators for each ID newtype
- [ ] Implement in-memory fakes for every port defined in Tasks 06–08
- [ ] Write the shared port-contract suite as a reusable function generic over an implementation, per the `architecture-principles.md` §Testing architecture *Port contract* tier
- [ ] Run the suite against every fake so the fakes are proven equivalent before any adapter exists
- [ ] Implement temporary-root and golden-fixture helpers that read from `fixtures/` without network access
- [ ] Add a test asserting no wall-clock sleep or network call is reachable from the testkit

## Definition of done

- [ ] One shared contract suite exists per port and is invoked generically, so a concrete adapter reuses it without copying assertions
- [ ] Every port from Tasks 06–08 has an in-memory fake that passes the shared suite
- [ ] `FakeClock` and the deterministic ID generators make tests reproducible: no wall-clock read, no sleep, no network
- [ ] The golden-fixture loader reads `fixtures/` deterministically and fails loudly on a missing or malformed fixture
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-testkit` and sees the shared contract suite pass against every in-memory fake with no wall-clock or network dependency
