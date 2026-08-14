# Task 94 — Implementation acceptance gate

**Plan:** [plan.md](../plan.md) · **Certificate:** [94-implementation_acceptance_gate-certificate.md](94-implementation_acceptance_gate-certificate.md)

**Implements:** [00-overview.md §Implementation acceptance](../../../00-overview.md#implementation-acceptance) · [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition) · [architecture-principles.md §Testing architecture](../../../architecture-principles.md#testing-architecture)
**Depends on:** 10, 31, 53, 77, 87, 88, 89, 90, 91, 92, 93
**Produces:** one executable gate that fails unless every `00-overview.md` §Implementation acceptance criterion and every §Compatibility definition surface is proven green
**Pointers:** `tests/conformance/acceptance.rs`, `.github/workflows/acceptance.yml`; reference: `.specs/00-overview.md`

## Steps

- [ ] Map each of the six §Implementation acceptance criteria to the suite that proves it
- [ ] Map each of the nine §Compatibility definition surfaces to its suite
- [ ] Implement the gate as a single command that runs every mapped suite and fails on any red or missing suite
- [ ] Fail the gate when a criterion has no mapped suite, so a future spec addition cannot pass silently
- [ ] Run the gate in CI as a required check
- [ ] Emit a machine-readable report naming each criterion, its suite, and its result

## Definition of done

- [ ] Every one of the six §Implementation acceptance criteria maps to a named suite and the mapping is asserted rather than documented
- [ ] Every one of the nine §Compatibility definition surfaces maps to a named suite and passes
- [ ] The gate is one command that fails on any red or missing suite and emits a machine-readable report
- [ ] The gate runs in CI as a required check
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs the acceptance gate command and sees a report listing all six implementation-acceptance criteria and all nine compatibility surfaces with their suites green
