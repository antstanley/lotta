# Task 10 — Protocol wire enums and the discriminant fixture gate

**Plan:** [plan.md](../plan.md) · **Certificate:** [10-protocol_wire_enums_and_fixtures-certificate.md](10-protocol_wire_enums_and_fixtures-certificate.md)

**Implements:** [02-app-server-api.md §WebSocket command groups](../../../02-app-server-api.md#websocket-command-groups) · [02-app-server-api.md §Outbound message groups](../../../02-app-server-api.md#outbound-message-groups) · [architecture-principles.md §Compatibility architecture](../../../architecture-principles.md#compatibility-architecture) · [development-guidelines.md §Repository hygiene](../../../development-guidelines.md#repository-hygiene)
**Depends on:** 04, 05
**Produces:** one tagged command enum and one tagged message enum whose variant lists are proven equal to a checked-in fixture extracted from the pinned TypeScript unions
**Pointers:** `crates/lotta-protocol/src/command.rs`, `src/message.rs`, `tools/extract-protocol-fixture.mjs`, `fixtures/protocol/discriminants.json`, `crates/lotta-protocol/tests/manifest.rs`; reference: `../letta-code/src/types/protocol_v2.ts`, `../letta-code/src/types/app-server-protocol.ts`, `../letta-code/src/types/app-server-info.ts`

## Steps

- [ ] Write an extractor that reads `../letta-code/src/types/protocol_v2.ts` and `app-server-protocol.ts` at the pinned commit and emits `fixtures/protocol/discriminants.json`
- [ ] Define `#[serde(tag = "type")] enum WsProtocolCommand` and `WsProtocolMessage` with one variant per extracted discriminant
- [ ] Pin `APP_SERVER_PROTOCOL_VERSION = 1` from `../letta-code/src/types/app-server-info.ts:1`
- [ ] Implement baseline-compatible decode policy: drop unknown top-level `type` without response or state mutation, tolerate extra object fields where the baseline structural guards do, keep required fields and enum values strict, and map a malformed known `input` frame to the non-terminal loop-error notice
- [ ] Add the manifest test comparing the Rust variant lists against the fixture in both directions
- [ ] Add a regeneration-is-clean CI step that re-runs the extractor and fails on any diff, per `development-guidelines.md` §Repository hygiene

## Definition of done

- [ ] Every baseline command and message discriminant has exactly one Rust variant, proven by a bidirectional manifest test against `fixtures/protocol/discriminants.json`
- [ ] Re-running the extractor against the pinned checkout produces no diff, and CI enforces it
- [ ] Unknown top-level `type` values are dropped with no response and no state mutation; extra fields are tolerated where the baseline tolerates them; required fields and enum values stay strict
- [ ] A malformed known `input` frame produces the baseline non-terminal loop-error notice rather than a terminal failure
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-protocol` and then the extractor in check mode, and sees the manifest equality, decode-policy cases, and a clean regeneration diff
