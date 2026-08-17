# Task 43 — MCP client, transports, and bounded discovery

**Plan:** [plan.md](../plan.md) · **Certificate:** [43-mcp_client-certificate.md](43-mcp_client-certificate.md)

**Implements:** [05-tools-and-extensions.md §External tools and MCP](../../../05-tools-and-extensions.md#external-tools-and-mcp)
**Depends on:** 32, 33, 42
**Produces:** a Rust MCP client supporting `stdio`, `sse`, and `http` with namespaced bounded discovery and atomic per-server refresh
**Pointers:** `crates/lotta-extensions/src/mcp/client.rs`, `mcp/transport.rs`, `mcp/oauth.rs`, `mcp/discovery.rs`; reference: `../letta-code/src/mcp-client.ts`, `../letta-code/src/mcp-runtime.ts`, `../letta-code/src/mcp-oauth.ts`

## Steps

- [x] Implement the three config discriminants `stdio` (also the omitted default), `sse`, and `http` for streamable HTTP
- [x] Reuse the Task 42 framing for the `stdio` transport's child boundary validation
- [x] Namespace discovered tools per server and bound discovery at `MCP_TOOLS_PER_SERVER_MAX`
- [x] Enforce `MCP_SERVERS_PER_AGENT_MAX` and replace a server's tool group atomically on refresh
- [x] Store OAuth tokens and server credentials in the credential store, never in diagnostics
- [x] Add negative tests for an unreachable server, a malformed discovery response, and a refresh failure leaving the previous group intact

## Definition of done

- [x] All three transports work and an omitted discriminant defaults to `stdio`
- [x] Discovered tools are namespaced per server and bounded by `MCP_TOOLS_PER_SERVER_MAX` and `MCP_SERVERS_PER_AGENT_MAX`
- [x] A server refresh replaces its tool group atomically, and a failed refresh leaves the previous group intact
- [x] OAuth tokens and server credentials live in the credential store and never reach diagnostics
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(mcp::)'` and sees three transports, stdio defaulting, namespaced bounded discovery, atomic refresh, and credential containment pass
