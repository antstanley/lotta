# Task 41 — Controller-owned external tools

**Plan:** [plan.md](../plan.md) · **Certificate:** [41-external_tools-certificate.md](41-external_tools-certificate.md)

**Implements:** [05-tools-and-extensions.md §External tools and MCP](../../../05-tools-and-extensions.md#external-tools-and-mcp) · [01-domain-model.md §External tool registration](../../../01-domain-model.md#external-tool-registration)
**Depends on:** 32, 33
**Produces:** controller-owned tool registration at `runtime_start` and by atomic update, with scoped calls, the fixed five-minute timeout, and typed owner-disconnect rejection
**Pointers:** `crates/lotta-tools/src/external/registry.rs`, `external/call.rs`; reference: `../letta-code/src/websocket/listener/external-tools.ts`, `../letta-code/src/websocket/listener/external-tool-protocol.ts`

## Steps

- [ ] Register controller-owned tools at `runtime_start` and through atomic update groups with an optional `scope_id`
- [ ] Correlate calls by runtime, request ID, tool call ID, name, validated arguments, and optional scope ID
- [ ] Own the fixed `EXTERNAL_TOOL_CALL_TIMEOUT_MS` five-minute timeout on the server side
- [ ] Reject pending calls with a typed owner-disconnected result when the originating connection drops
- [ ] Resolve each call through the originating connection, never a different one
- [ ] Enforce `EXTERNAL_TOOLS_PER_RUNTIME_MAX` and reject a registration group atomically

## Definition of done

- [ ] Registration accepts unscoped and `scope_id`-selected tools at `runtime_start` and by atomic update, rejecting a whole group atomically on any invalid member
- [ ] The server owns a fixed five-minute call timeout, and a timed-out call yields a typed timeout result rather than hanging
- [ ] A pending call is rejected with a typed owner-disconnected result when its originating connection drops, and calls never resolve through another connection
- [ ] `EXTERNAL_TOOLS_PER_RUNTIME_MAX` rejects registration at the limit with the 257th tool
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(external::)'` and sees scoped registration, atomic group rejection, the five-minute timeout, owner-disconnect rejection, and the per-runtime cap pass
