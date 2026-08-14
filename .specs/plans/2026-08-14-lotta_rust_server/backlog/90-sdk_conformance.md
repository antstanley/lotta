# Task 90 — Agent SDK conformance

**Plan:** [plan.md](../plan.md) · **Certificate:** [90-sdk_conformance-certificate.md](90-sdk_conformance-certificate.md)

**Implements:** [00-overview.md §Compatibility definition](../../../00-overview.md#compatibility-definition) · [00-overview.md §Implementation acceptance](../../../00-overview.md#implementation-acceptance)
**Depends on:** 70, 71, 73, 85
**Produces:** the remote-backend conformance suite creating, resuming, streaming, compacting, forking, and deleting local agents and conversations without client patches
**Pointers:** `tests/conformance/sdk.rs`, `tests/conformance/harness/sdk_driver.mjs`; reference: `../letta-code/src/app-server-client.ts`, `../letta-code/src/types/app-server-protocol.ts`

## Steps

- [ ] Drive the assembled binary with the unmodified Agent SDK from the pinned checkout
- [ ] Exercise create, resume, stream, compact, fork, and delete for local agents and conversations
- [ ] Assert no client patch, shim, or protocol translation is present in the harness
- [ ] Compare the observed event stream against the ordering invariants
- [ ] Cover both a fresh backend root and one populated from `fixtures/persistence/`
- [ ] Fail the suite if the SDK emits a command the server drops as unknown

## Definition of done

- [ ] All six SDK operations succeed against the assembled binary with an unmodified client
- [ ] The observed event stream satisfies the six ordering invariants
- [ ] The suite runs against both a fresh root and a corpus-populated root
- [ ] The suite fails if the SDK sends a command the server drops as unknown
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run --test sdk` against the built binary and watches the unmodified Agent SDK create, resume, stream, compact, fork, and delete a local agent
