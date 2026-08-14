# Task 19 — Minimal turn loop against fake provider and tool ports

**Plan:** [plan.md](../plan.md) · **Certificate:** [19-minimal_turn_loop_fake_ports-certificate.md](19-minimal_turn_loop_fake_ports-certificate.md)

**Implements:** [03-runtime-and-turns.md §Provider and tool loop](../../../03-runtime-and-turns.md#provider-and-tool-loop) · [03-runtime-and-turns.md §Responsibilities](../../../03-runtime-and-turns.md#responsibilities)
**Depends on:** 09, 18
**Produces:** a turn that streams provider events, persists text and reasoning projections, executes one sequential local tool, and completes exactly once — all against fake ports
**Pointers:** `crates/lotta-runtime/src/turn/loop.rs`, `src/turn/step.rs`, `src/turn/projection.rs`; reference: `../letta-code/src/backend/dev/provider-turn-executor.ts`, `../letta-code/src/websocket/listener/turn.ts`, `../letta-code/src/websocket/listener/turn-send.ts`, `../letta-code/src/websocket/listener/turn-completion.ts`

## Steps

- [ ] Send a `ProviderRequest` through the fake `ProviderPort` and consume the normalized event stream
- [ ] Persist text and reasoning projections and emit `stream_delta` for each
- [ ] Assemble tool calls from `ToolCallStart`/`ToolCallArgumentsDelta`/`ToolCallEnd` and validate arguments only at tool-call end
- [ ] Execute a local tool call through the fake `ToolPort` sequentially, then append the tool result and continue the loop
- [ ] Complete exactly once on a terminal stop, emitting the terminal event after the final stream delta
- [ ] Keep every effect lease-guarded so a superseded turn emits nothing

## Definition of done

- [ ] Text and reasoning deltas are both projected and streamed; a stream that carries `ReasoningDelta` or `RedactedReasoning` does not lose them
- [ ] Tool-call arguments are buffered under a declared bound and validated only at `ToolCallEnd`, with tool-call IDs stable across deltas
- [ ] Tool execution is sequential: two tool calls in one step execute one after the other, in stream order
- [ ] A terminal stop completes the turn exactly once, after its final stream delta, and a superseded turn emits nothing
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(turn::)'` and sees reasoning projections, end-only argument validation, sequential tool execution, and exactly-once terminal completion pass against the fake ports
