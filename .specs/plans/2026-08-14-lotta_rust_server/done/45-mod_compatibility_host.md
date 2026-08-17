# Task 45 — Mod compatibility host and capability scoping

**Plan:** [plan.md](../plan.md) · **Certificate:** [45-mod_compatibility_host-certificate.md](45-mod_compatibility_host-certificate.md)

**Implements:** [05-tools-and-extensions.md §Hooks and mods](../../../05-tools-and-extensions.md#hooks-and-mods)
**Depends on:** 32, 42, 44
**Produces:** a versioned JSON-RPC mod host over the shared sidecar framing, with declared-capability scoping, reload, dispose, generation invalidation, diagnostics, safe mode, and `--no-mods`
**Pointers:** `crates/lotta-extensions/src/mods/host.rs`, `mods/capabilities.rs`, `mods/registrations.rs`, `mods/safe_mode.rs`; reference: `../letta-code/src/mods/mod-engine.ts`, `capabilities.ts`, `conversation-handle.ts`, `../letta-code/src/websocket/listener/mod-adapter.ts`, `mod-command-registry.ts`

## Steps

- [x] Run existing TypeScript mods in a separate host speaking versioned JSON-RPC over the Task 42 framing
- [x] Give the host only its declared capabilities and a scoped conversation handle, with no access to Rust memory
- [x] Let mods register tools, commands, providers, permissions, lifecycle events, and UI metadata
- [x] Support reload, dispose, generation invalidation, diagnostics, and safe mode
- [x] Implement `--no-mods` starting without the host and removing every mod-owned registration
- [x] Bound host frames at `MOD_HOST_MESSAGE_BYTES_MAX` and attribute a mod failure to its owner

## Definition of done

- [x] Mods register all six registration kinds through the host, and every registration is attributed to its owning mod
- [x] The host receives only declared capabilities and a scoped conversation handle; an undeclared capability call is refused
- [x] Reload, dispose, generation invalidation, diagnostics, and safe mode all work, and a stale generation's registrations are invalidated
- [x] `--no-mods` starts without the host and removes every mod-owned registration
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(mods::)'` and sees six registration kinds, capability refusal, the five lifecycle operations, and `--no-mods` pass
