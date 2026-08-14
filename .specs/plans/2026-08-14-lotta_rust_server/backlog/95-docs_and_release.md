# Task 95 — Documentation and release readiness

**Plan:** [plan.md](../plan.md) · **Certificate:** [95-docs_and_release-certificate.md](95-docs_and_release-certificate.md)

**Implements:** [00-overview.md §Scope summary](../../../00-overview.md#scope-summary) · [architecture-principles.md §Compatibility architecture](../../../architecture-principles.md#compatibility-architecture) · [07-channels-and-operations.md §Observability and security](../../../07-channels-and-operations.md#observability-and-security)
**Depends on:** 86, 94
**Produces:** rustdoc without warnings, an architecture and operations guide, a compatibility statement, and a release checklist whose items are executable commands
**Pointers:** `README.md`, `docs/architecture.md`, `docs/operations.md`, `docs/compatibility.md`, `RELEASE-CHECKLIST.md`, `CHANGELOG.md`; reference: `.specs/00-overview.md`, `.specs/architecture-principles.md`, `../letta-app-server-deployment/`

## Steps

- [ ] Document each crate's responsibility, ports, dependencies, and forbidden dependencies in its crate root rustdoc
- [ ] Write `docs/architecture.md` describing the ports-and-adapters layout and the dependency direction
- [ ] Write `docs/operations.md` covering deployment, health, backup and restore, shutdown, and upgrade
- [ ] Write `docs/compatibility.md` stating the pinned baseline, the parity surfaces, and every labelled Lotta hardening
- [ ] Write `RELEASE-CHECKLIST.md` where every item is an executable command with an expected result, including the Task 94 gate
- [ ] Record the license and attribution position for fixtures derived from Apache-2.0 Letta Code

## Definition of done

- [ ] `cargo doc --workspace --no-deps` is clean with denied warnings and every crate root documents its responsibility, ports, dependencies, and forbidden dependencies
- [ ] `docs/compatibility.md` names the pinned baseline, every §Compatibility definition surface, and every labelled Lotta hardening
- [ ] Every `RELEASE-CHECKLIST.md` item is an executable command with an expected result, including the acceptance gate
- [ ] The license and attribution position for fixtures derived from Apache-2.0 Letta Code is stated
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs every command in `RELEASE-CHECKLIST.md` top to bottom and sees each produce its stated expected result, ending with a green acceptance gate
