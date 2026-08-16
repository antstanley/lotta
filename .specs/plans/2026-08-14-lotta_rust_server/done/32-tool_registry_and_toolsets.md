# Task 32 — Tool registry, toolset resolution, and atomic swap

**Plan:** [plan.md](../plan.md) · **Certificate:** [32-tool_registry_and_toolsets-certificate.md](32-tool_registry_and_toolsets-certificate.md)

**Implements:** [05-tools-and-extensions.md §Tool registry](../../../05-tools-and-extensions.md#tool-registry)
**Depends on:** 08, 09
**Produces:** the six toolset IDs with per-toolset model-facing names, `auto` resolution, allowlist filtering, and side-built registries swapped atomically
**Pointers:** `crates/lotta-tools/src/registry.rs`, `src/toolset.rs`, `src/names.rs`, `src/allowlist.rs`; reference: `../letta-code/src/tools/toolset.ts`, `../letta-code/src/tools/manager.ts`, `../letta-code/src/tools/filter.ts`, `../letta-code/src/tools/model-facing-tool.ts`

## Steps

- [x] Define the toolset ID set as exactly `default`, `codex`, `codex_snake`, `gemini`, `gemini_snake`, and `none`, with `auto` as a preference resolving to one of them
- [x] Map each internal stable tool name to its per-toolset model-facing name, keeping the Anthropic-oriented names in `default` and the snake/Pascal variants in the Codex and Gemini sets
- [x] Expose internal `Task` globally as `Agent`
- [x] Implement the allowlist filtering both built-ins and external tools, with an empty allowlist exposing none
- [x] Build registry updates off to the side, validate them, then swap atomically so a model never observes a partial toolset
- [x] Enforce `TOOLS_LOADED_MAX` and reject a registration that would leave a half-registered toolset

## Definition of done

- [x] The toolset ID set is exactly the six spec IDs, `auto` resolves to one of them, and per-toolset model-facing names match the baseline
- [x] Internal `Task` is exposed globally as `Agent`, and every alias calls one internal implementation
- [x] An allowlist filters both built-ins and external tools, and an empty allowlist exposes none
- [x] Registry updates are built aside, validated, then swapped atomically; a failed update leaves the previous registry intact and `TOOLS_LOADED_MAX` rejects at the limit
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(toolset::) + test(names::) + test(allowlist::) + test(registry::)'` and sees six IDs, `Task`→`Agent`, allowlist filtering, and atomic swap pass
