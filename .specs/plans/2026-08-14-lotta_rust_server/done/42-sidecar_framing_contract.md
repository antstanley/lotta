# Task 42 — Shared bounded sidecar framing contract

**Plan:** [plan.md](../plan.md) · **Certificate:** [42-sidecar_framing_contract-certificate.md](42-sidecar_framing_contract-certificate.md)

**Implements:** [architecture-principles.md §Compatibility architecture](../../../architecture-principles.md#compatibility-architecture) · [06-model-providers.md §Provider classes](../../../06-model-providers.md#provider-classes) · [05-tools-and-extensions.md §Hooks and mods](../../../05-tools-and-extensions.md#hooks-and-mods) · [development-guidelines.md §Where to validate](../../../development-guidelines.md#where-to-validate)
**Depends on:** 05, 09
**Produces:** one versioned length-prefixed JSON framing used by the provider host, the mod host, and the subagent host, validated at the child-to-host boundary
**Pointers:** `crates/lotta-extensions/src/sidecar/framing.rs`, `sidecar/handshake.rs`, `sidecar/supervisor.rs`; reference: `../letta-code/src/backend/dev/pi-provider-registry.ts`, `../letta-code/src/mods/mod-engine.ts`, `../letta-code/src/websocket/listener/mod-adapter.ts`

## Steps

- [x] Implement length-prefixed JSON framing over local pipes with a declared maximum frame length
- [x] Implement a version handshake that rejects an unsupported protocol version before any payload is processed
- [x] Validate frame length, protocol version, owner identity, declared capability, and timeout on every inbound frame, per `development-guidelines.md` §Where to validate
- [x] Implement a supervisor with bounded restarts (`SIDECAR_RESTARTS_PER_HOUR_MAX`) and a crash outcome that never corrupts host state
- [x] Bound host-side buffering at `MOD_HOST_MESSAGE_BYTES_MAX` for mod frames and its provider equivalent
- [x] Expose the contract so Tasks 45, 46, and 51 each use it rather than defining their own

## Definition of done

- [x] Framing is length-prefixed JSON with a declared maximum frame length enforced before the payload is read
- [x] All five §Where to validate child-to-host properties are checked on every inbound frame: frame length, protocol version, owner identity, capability, and timeout
- [x] The version handshake rejects an unsupported protocol version before any payload is processed
- [x] The supervisor bounds restarts at `SIDECAR_RESTARTS_PER_HOUR_MAX` and a crash leaves host state uncorrupted
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(sidecar::)'` and sees framing bounds, the five validation properties, version rejection, and bounded restarts pass
